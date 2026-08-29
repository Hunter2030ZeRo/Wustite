use std::collections::{BTreeMap, BTreeSet};

use super::{Builder, SsaValue};
use crate::adaptive_v2::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode, VirtualKind,
    VirtualRecipe,
};
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId, Effect, Instruction, InstructionKind, RootLocation, RootMap, SafepointId,
    Terminator, ValueDef, ValueId, ValueType,
};
use crate::bytecode::{Instruction as WvmInstruction, Register};

mod ops;

struct SplicedChild {
    entry: Terminator,
    continuation: BlockId,
    output: Option<ValueDef>,
}

enum NestedListDestination {
    Entry(SsaValue),
    Owned { identity: u32 },
}

struct PreparedNestedChild {
    callee: crate::executable::ExecutableFunction,
    region: crate::structure_map::Region,
    locals: BTreeMap<Register, SsaValue>,
    child: super::LoweredLoop,
}

fn remap_root(root: RootLocation, base: u32, virtuals: &BTreeMap<u32, u32>) -> RootLocation {
    match root {
        RootLocation::Ssa(value) => {
            RootLocation::Ssa(ValueId::new(base.saturating_add(value.get())))
        }
        RootLocation::Virtual(id) => RootLocation::Virtual(virtuals[&id]),
        other => other,
    }
}

fn remap_source(
    source: RegisterSource,
    base: u32,
    virtuals: &BTreeMap<u32, u32>,
) -> RegisterSource {
    match source {
        RegisterSource::Ssa(value) => {
            RegisterSource::Ssa(ValueId::new(base.saturating_add(value.get())))
        }
        RegisterSource::Virtual(id) => RegisterSource::Virtual(virtuals[&id]),
        other => other,
    }
}

fn remap_virtual_kind(kind: &mut VirtualKind, base: u32, virtuals: &BTreeMap<u32, u32>) {
    let sources = match kind {
        VirtualKind::Object { fields, .. } => fields
            .iter_mut()
            .map(|(_, source)| source)
            .collect::<Vec<_>>(),
        VirtualKind::List { items } | VirtualKind::Tuple { items } => {
            items.iter_mut().collect::<Vec<_>>()
        }
    };
    for source in sources {
        *source = remap_source(source.clone(), base, virtuals);
    }
}

fn elide_invariant_handle_phis(child: &mut super::LoweredLoop) {
    let Some(entry) = child.blocks.first().map(|block| block.id) else {
        return;
    };
    let copy_sources = child
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| {
            if matches!(instruction.kind.semantic(), InstructionKind::Copy)
                && instruction.inputs.len() == 1
                && instruction
                    .output
                    .is_some_and(|output| output.ty == ValueType::Handle)
            {
                Some((
                    instruction.output.expect("checked handle copy").id,
                    instruction.inputs[0],
                ))
            } else {
                None
            }
        })
        .collect::<BTreeMap<_, _>>();
    let copy_root = |mut value: ValueId| {
        while let Some(source) = copy_sources.get(&value).copied() {
            value = source;
        }
        value
    };
    let mut aliases = BTreeMap::new();
    let mut removals = BTreeMap::<BlockId, Vec<usize>>::new();
    for block in &child.blocks {
        if block.id == entry {
            continue;
        }
        for (index, parameter) in block.parameters.iter().enumerate() {
            if parameter.ty != ValueType::Handle {
                continue;
            }
            let incoming =
                child
                    .blocks
                    .iter()
                    .filter_map(|predecessor| match &predecessor.terminator {
                        Terminator::Jump { target, arguments } if *target == block.id => {
                            arguments.get(index).copied()
                        }
                        _ => None,
                    });
            let mut initial = None;
            let mut valid = true;
            for value in incoming.map(copy_root) {
                if value == parameter.id {
                    continue;
                }
                if initial.is_some_and(|candidate| candidate != value) {
                    valid = false;
                    break;
                }
                initial = Some(value);
            }
            if valid && let Some(initial) = initial {
                aliases.insert(parameter.id, initial);
                removals.entry(block.id).or_default().push(index);
            }
        }
    }
    if aliases.is_empty() {
        return;
    }
    let resolve = |mut value: ValueId| {
        while let Some(alias) = aliases.get(&value).copied() {
            value = alias;
        }
        value
    };
    for block in &mut child.blocks {
        if let Some(indices) = removals.get(&block.id) {
            block.parameters = block
                .parameters
                .drain(..)
                .enumerate()
                .filter_map(|(index, parameter)| (!indices.contains(&index)).then_some(parameter))
                .collect();
        }
        for instruction in &mut block.instructions {
            instruction
                .inputs
                .iter_mut()
                .for_each(|value| *value = resolve(*value));
        }
        match &mut block.terminator {
            Terminator::Jump { target, arguments } => {
                arguments
                    .iter_mut()
                    .for_each(|value| *value = resolve(*value));
                if let Some(indices) = removals.get(target) {
                    *arguments = arguments
                        .drain(..)
                        .enumerate()
                        .filter_map(|(index, argument)| {
                            (!indices.contains(&index)).then_some(argument)
                        })
                        .collect();
                }
            }
            Terminator::Branch { condition, .. } => *condition = resolve(*condition),
            Terminator::Return { values } | Terminator::SideExit { values, .. } => {
                values.iter_mut().for_each(|value| *value = resolve(*value));
            }
            Terminator::Backedge { .. } | Terminator::IrreducibleBackedge => {}
        }
    }
    for map in &mut child.root_maps {
        map.roots = map
            .roots
            .iter()
            .map(|root| match root {
                RootLocation::Ssa(value) => RootLocation::Ssa(resolve(*value)),
                other => *other,
            })
            .collect();
    }
    for recipe in &mut child.deopts {
        recipe.explicit_roots.iter_mut().for_each(|root| {
            if let RootLocation::Ssa(value) = root {
                *value = resolve(*value);
            }
        });
        for frame in &mut recipe.frames {
            for register in &mut frame.registers {
                if let RegisterSource::Ssa(value) = &mut register.source {
                    *value = resolve(*value);
                }
            }
        }
        for virtual_value in &mut recipe.virtuals {
            let sources = match &mut virtual_value.kind {
                VirtualKind::Object { fields, .. } => fields
                    .iter_mut()
                    .map(|(_, source)| source)
                    .collect::<Vec<_>>(),
                VirtualKind::List { items } | VirtualKind::Tuple { items } => {
                    items.iter_mut().collect::<Vec<_>>()
                }
            };
            for source in sources {
                if let RegisterSource::Ssa(value) = source {
                    *value = resolve(*value);
                }
            }
        }
    }
}

pub(super) fn widen_numeric_phis(
    child: &mut super::LoweredLoop,
    initial_types: &[ValueType],
) -> Result<(), String> {
    let mut types = BTreeMap::new();
    let mut next_value = 0;
    for definition in child
        .blocks
        .iter()
        .flat_map(|block| block.parameters.iter())
        .chain(child.blocks.iter().flat_map(|block| {
            block
                .instructions
                .iter()
                .filter_map(|instruction| instruction.output.as_ref())
        }))
    {
        types.insert(definition.id, definition.ty);
        next_value = next_value.max(definition.id.get().saturating_add(1));
    }
    let mut candidates = Vec::new();
    for block in &child.blocks {
        for (index, parameter) in block.parameters.iter().enumerate() {
            if parameter.ty != ValueType::I64 {
                continue;
            }
            let incoming = child
                .blocks
                .iter()
                .filter_map(|predecessor| match &predecessor.terminator {
                    Terminator::Jump { target, arguments } if *target == block.id => {
                        arguments.get(index).copied()
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let has_initial_integer = if block.id
                == child
                    .blocks
                    .first()
                    .map(|entry| entry.id)
                    .unwrap_or(block.id)
            {
                initial_types.get(index) == Some(&ValueType::I64)
            } else {
                incoming
                    .iter()
                    .any(|value| types.get(value) == Some(&ValueType::I64))
            };
            if !has_initial_integer
                || incoming.is_empty()
                || !incoming
                    .iter()
                    .any(|value| types.get(value) == Some(&ValueType::F64))
                || incoming
                    .iter()
                    .any(|value| !matches!(types.get(value), Some(ValueType::I64 | ValueType::F64)))
            {
                continue;
            }
            let conversions = child
                .blocks
                .iter()
                .flat_map(|candidate| &candidate.instructions)
                .filter(|instruction| {
                    matches!(instruction.kind.semantic(), InstructionKind::IntegerToFloat)
                        && instruction.inputs == [parameter.id]
                })
                .filter_map(|instruction| {
                    instruction
                        .output
                        .map(|output| (output.id, instruction.kind.clone()))
                })
                .collect::<Vec<_>>();
            if conversions.is_empty() {
                return Err(format!(
                    "adaptive-v2 mixed numeric phi lacks an explicit promotion at block {:?} parameter {index} value {:?} incoming {:?}",
                    block.id,
                    parameter.id,
                    incoming
                        .iter()
                        .map(|value| (*value, types.get(value).copied()))
                        .collect::<Vec<_>>()
                ));
            }
            candidates.push((block.id, index, parameter.id, conversions));
        }
    }
    let entry_id = child
        .blocks
        .first()
        .map(|block| block.id)
        .ok_or_else(|| "adaptive-v2 nested child has no entry".to_owned())?;
    let original_entry_parameters = child
        .blocks
        .first()
        .map(|block| block.parameters.clone())
        .ok_or_else(|| "adaptive-v2 nested child has no entry parameters".to_owned())?;
    let mut promotions = Vec::new();
    for (block_id, index, parameter, conversions) in candidates {
        let shadow = ValueId::new(next_value);
        next_value = next_value.saturating_add(1);
        let conversion_values = conversions
            .iter()
            .map(|(value, _)| *value)
            .collect::<BTreeSet<_>>();
        let conversion_kind = conversions[0].1.clone();
        let header = child
            .blocks
            .iter_mut()
            .find(|block| block.id == block_id)
            .ok_or_else(|| "adaptive-v2 promoted phi disappeared".to_owned())?;
        header
            .parameters
            .get_mut(index)
            .ok_or_else(|| "adaptive-v2 promoted phi parameter disappeared".to_owned())?
            .ty = ValueType::F64;
        header
            .parameters
            .push(ValueDef::new(shadow, ValueType::I64));
        for block in &mut child.blocks {
            for instruction in &mut block.instructions {
                for input in &mut instruction.inputs {
                    if conversion_values.contains(input) {
                        *input = parameter;
                    }
                }
            }
            block.instructions.retain(|instruction| {
                !instruction
                    .output
                    .is_some_and(|output| conversion_values.contains(&output.id))
            });
            match &mut block.terminator {
                Terminator::Jump { arguments, .. }
                | Terminator::Return { values: arguments }
                | Terminator::SideExit {
                    values: arguments, ..
                } => {
                    for value in arguments {
                        if conversion_values.contains(value) {
                            *value = parameter;
                        }
                    }
                }
                Terminator::Branch { condition, .. } => {
                    if conversion_values.contains(condition) {
                        *condition = parameter;
                    }
                }
                Terminator::Backedge { .. } | Terminator::IrreducibleBackedge => {}
            }
        }
        for recipe in &mut child.deopts {
            for frame in &mut recipe.frames {
                for register in &mut frame.registers {
                    if matches!(register.source, RegisterSource::Ssa(value) if value == parameter || conversion_values.contains(&value))
                    {
                        if register.ty == ValueType::I64 {
                            register.source = RegisterSource::Ssa(shadow);
                        } else {
                            register.source = RegisterSource::Ssa(parameter);
                        }
                    }
                }
            }
            for virtual_value in &mut recipe.virtuals {
                if let VirtualKind::Tuple { items } = &mut virtual_value.kind {
                    for item in items {
                        if matches!(item, RegisterSource::Ssa(value) if conversion_values.contains(value))
                        {
                            *item = RegisterSource::Ssa(parameter);
                        }
                    }
                }
            }
        }
        for predecessor in &mut child.blocks {
            let Terminator::Jump { target, arguments } = &mut predecessor.terminator else {
                continue;
            };
            if *target != block_id {
                continue;
            }
            let Some(original) = arguments.get(index).copied() else {
                continue;
            };
            if types.get(&original) == Some(&ValueType::I64) {
                arguments.push(original);
                let output = ValueId::new(next_value);
                next_value = next_value.saturating_add(1);
                predecessor.instructions.push(Instruction::new(
                    conversion_kind.clone(),
                    vec![original],
                    Some(ValueDef::new(output, ValueType::F64)),
                    Effect::Pure,
                ));
                arguments[index] = output;
            } else if types.get(&original) == Some(&ValueType::F64) {
                arguments.push(shadow);
            } else {
                continue;
            }
        }
        if block_id == entry_id {
            promotions.push((index, conversion_kind, shadow));
        }
    }
    if !promotions.is_empty() {
        let preheader_id = BlockId::new(
            child
                .blocks
                .iter()
                .map(|block| block.id.get())
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        let mut preheader_parameters = Vec::new();
        for parameter in original_entry_parameters {
            preheader_parameters.push(ValueDef::new(ValueId::new(next_value), parameter.ty));
            next_value = next_value.saturating_add(1);
        }
        let mut arguments = preheader_parameters
            .iter()
            .map(|parameter| parameter.id)
            .collect::<Vec<_>>();
        let mut instructions = Vec::new();
        for (index, conversion_kind, _) in &promotions {
            let input = preheader_parameters[*index].id;
            let output = ValueId::new(next_value);
            next_value = next_value.saturating_add(1);
            instructions.push(Instruction::new(
                conversion_kind.clone(),
                vec![input],
                Some(ValueDef::new(output, ValueType::F64)),
                Effect::Pure,
            ));
            arguments[*index] = output;
        }
        arguments.extend(
            promotions
                .iter()
                .map(|(index, _, _)| preheader_parameters[*index].id),
        );
        child.blocks.insert(
            0,
            Block::new(
                preheader_id,
                preheader_parameters,
                instructions,
                Terminator::Jump {
                    target: entry_id,
                    arguments,
                },
            ),
        );
    }
    Ok(())
}

pub(super) fn elide_dead_numeric_phis(
    child: &mut super::LoweredLoop,
    executable: &crate::executable::ExecutableFunction,
    storage_registers: &[Register],
) {
    let mut used = BTreeSet::new();
    for block in &child.blocks {
        for instruction in &block.instructions {
            used.extend(instruction.inputs.iter().copied());
        }
        match &block.terminator {
            Terminator::Jump { arguments, .. }
            | Terminator::Return { values: arguments }
            | Terminator::SideExit {
                values: arguments, ..
            } => used.extend(arguments.iter().copied()),
            Terminator::Branch { condition, .. } => {
                used.insert(*condition);
            }
            Terminator::Backedge { .. } | Terminator::IrreducibleBackedge => {}
        }
    }
    for recipe in &child.deopts {
        used.extend(recipe.explicit_roots.iter().filter_map(|root| match root {
            RootLocation::Ssa(value) => Some(*value),
            _ => None,
        }));
        for virtual_recipe in &recipe.virtuals {
            match &virtual_recipe.kind {
                VirtualKind::Object { fields, .. } => {
                    used.extend(fields.iter().filter_map(|(_, source)| match source {
                        RegisterSource::Ssa(value) => Some(*value),
                        _ => None,
                    }))
                }
                VirtualKind::List { items } | VirtualKind::Tuple { items } => {
                    used.extend(items.iter().filter_map(|source| match source {
                        RegisterSource::Ssa(value) => Some(*value),
                        _ => None,
                    }));
                }
            }
        }
    }
    let provisional_dead = child
        .blocks
        .iter()
        .flat_map(|block| &block.parameters)
        .filter(|parameter| parameter.ty != ValueType::Handle && !used.contains(&parameter.id))
        .map(|parameter| parameter.id)
        .collect::<BTreeSet<_>>();
    for recipe in &mut child.deopts {
        for frame in &mut recipe.frames {
            for register in &mut frame.registers {
                let RegisterSource::Ssa(value) = register.source else {
                    continue;
                };
                if !provisional_dead.contains(&value) {
                    used.insert(value);
                    continue;
                }
                let resume = usize::try_from(frame.resume_pc).unwrap_or(usize::MAX);
                if frame.function != executable.id().as_u64()
                    || storage_registers.contains(&register.register)
                    || replay_value_is_live(
                        executable.bytecode().code.as_slice(),
                        resume,
                        register.register,
                    )
                {
                    used.insert(value);
                } else {
                    register.source = RegisterSource::UndefinedDead;
                    frame.dead_registers.insert(register.register);
                }
            }
        }
    }
    let dead = child
        .blocks
        .iter()
        .flat_map(|block| {
            block
                .parameters
                .iter()
                .enumerate()
                .filter(|(_, parameter)| {
                    parameter.ty != ValueType::Handle && !used.contains(&parameter.id)
                })
                .map(|(index, _)| (block.id, index))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for (target, index) in dead.into_iter().rev() {
        if let Some(block) = child.blocks.iter_mut().find(|block| block.id == target) {
            block.parameters.remove(index);
        }
        for predecessor in &mut child.blocks {
            if let Terminator::Jump {
                target: destination,
                arguments,
            } = &mut predecessor.terminator
                && *destination == target
            {
                arguments.remove(index);
            }
        }
    }
}

impl Builder<'_> {
    pub(super) fn try_lower_reverse_prefix(
        &mut self,
        pc: usize,
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
    ) -> Result<bool, String> {
        let code = &self.executable.bytecode().code;
        let Some(pattern) = crate::wvm::match_reverse_prefix(code, pc) else {
            return Ok(false);
        };
        let object = self.read(values, pattern.object)?;
        let start = self.read(values, pattern.start)?;
        let step = self.read(values, pattern.step)?;
        let stop = self.read(values, pattern.stop)?;
        if object.ty != ValueType::Handle
            || start.ty != ValueType::I64
            || step.ty != ValueType::I64
            || stop.ty != ValueType::I64
            || self
                .element_types
                .get(&pattern.object)
                .copied()
                .or_else(|| self.copied_list_element_types.get(&object.id).copied())
                != Some(ValueType::I64)
        {
            return Ok(false);
        }
        let definition = |value: ValueId| {
            lowered
                .iter()
                .rev()
                .find(|instruction| instruction.output.is_some_and(|output| output.id == value))
        };
        if !definition(step.id).is_some_and(|instruction| {
            matches!(
                instruction.kind.semantic(),
                InstructionKind::Constant(crate::adaptive_v2::wxir_v2::ir::Constant::Integer(-1))
            )
        }) {
            return Ok(false);
        }
        let Some(stop_definition) = definition(stop.id) else {
            return Ok(false);
        };
        if !matches!(stop_definition.kind.semantic(), InstructionKind::IntegerAdd)
            || stop_definition.inputs.len() != 2
        {
            return Ok(false);
        }
        let one = if stop_definition.inputs[0] == start.id {
            stop_definition.inputs[1]
        } else if stop_definition.inputs[1] == start.id {
            stop_definition.inputs[0]
        } else {
            return Ok(false);
        };
        if !definition(one).is_some_and(|instruction| {
            matches!(
                instruction.kind.semantic(),
                InstructionKind::Constant(crate::adaptive_v2::wxir_v2::ir::Constant::Integer(1))
            )
        }) {
            return Ok(false);
        }
        let order = lowered
            .iter()
            .filter(|instruction| instruction.effect.is_ordered())
            .count() as u32;
        lowered.push(
            Instruction::new(
                InstructionKind::ListReversePrefix {
                    element_type: ValueType::I64,
                }
                .at_pc(pc_u32(pc)?),
                vec![object.id, stop.id],
                None,
                Effect::Write,
            )
            .ordered(order),
        );
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_splice_nested_loop_call(
        &mut self,
        pc: usize,
        instruction: &WvmInstruction,
        terminal_pc: usize,
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
        active: &mut Vec<usize>,
    ) -> Result<Option<Terminator>, String> {
        let WvmInstruction::Call {
            dst,
            callable,
            args,
        } = instruction
        else {
            return Ok(None);
        };
        let Some(callee) = ops::constant_callee(self.executable, pc, *callable).or_else(|| {
            self.call_targets
                .get(callable)
                .map(|target| &target.function)
        }) else {
            return Ok(None);
        };
        let target_metadata = self.call_targets.get(callable).or_else(|| {
            self.constant_call_targets
                .get(&(self.executable.id().as_u64(), *callable))
        });
        crate::verifier::verify(callee)?;
        let loops = callee.structure_map().loop_regions().collect::<Vec<_>>();
        let Some(&(mut callee_region_id, mut callee_region)) = loops.first() else {
            return self.try_splice_loopless_wrapper_call(
                pc,
                terminal_pc,
                *dst,
                args,
                callee,
                values,
                lowered,
                active,
            );
        };
        if loops.len() > 1 {
            if !target_metadata.is_some_and(|target| {
                target
                    .argument_element_paths
                    .iter()
                    .any(|path| path.len() > 1)
            }) {
                return Ok(None);
            }
            let mut maximal = loops.iter().copied().filter(|(_, candidate)| {
                loops.iter().all(|(_, nested)| {
                    nested
                        .blocks
                        .iter()
                        .all(|block| candidate.blocks.contains(block))
                })
            });
            let Some(selected) = maximal.next() else {
                return Ok(None);
            };
            if maximal.next().is_some() {
                return Ok(None);
            }
            (callee_region_id, callee_region) = selected;
        }
        let crate::structure_map::RegionKind::Loop { backedge } = callee_region.kind else {
            return Err("adaptive-v2 nested callee region is not a loop".to_owned());
        };
        let tuple_items = match args.as_slice() {
            [tuple] => self
                .virtual_tuples
                .get(tuple)
                .cloned()
                .map(|items| (*tuple, items)),
            _ => None,
        };
        let mut locals = BTreeMap::new();
        let mut constants = BTreeMap::<Register, i64>::new();
        let mut nested_element_types = BTreeMap::new();
        let mut nested_call_targets = BTreeMap::new();
        let reused_result = if let Some((_, tuple_items)) = &tuple_items {
            let [parameter] = callee.parameters() else {
                return Err("adaptive-v2 nested callee tuple ABI is unsupported".to_owned());
            };
            let tuple_handle = tuple_items
                .iter()
                .map(|(_, value)| *value)
                .find(|value| value.ty == ValueType::Handle)
                .ok_or_else(|| "adaptive-v2 nested tuple has no rooted Handle".to_owned())?;
            locals.insert(parameter.register, tuple_handle);
            None
        } else {
            if callee.parameters().len() != args.len() {
                return Ok(None);
            }
            for (parameter, argument) in callee.parameters().iter().zip(args) {
                let value = self.read(values, *argument)?;
                locals.insert(parameter.register, value);
                if value.ty == ValueType::Handle {
                    if let Some(element_type) = self.element_types.get(argument).copied() {
                        nested_element_types.insert(parameter.register, element_type);
                    }
                    if let Some(target) = self.call_targets.get(argument).cloned() {
                        nested_call_targets.insert(parameter.register, target);
                    }
                }
            }
            let destination = self
                .executable
                .bytecode()
                .code
                .get(pc.saturating_add(1))
                .and_then(|next| match next {
                    WvmInstruction::Move { dst: target, src } if *src == *dst => {
                        values.get(target).copied().map(|value| (*target, value))
                    }
                    _ => None,
                });
            match destination {
                Some((register, value)) if value.ty == ValueType::Handle => {
                    if let Some(element_type) = self.element_types.get(&register).copied() {
                        nested_element_types.insert(register, element_type);
                    }
                    Some((register, value))
                }
                _ => None,
            }
        };
        let mut element_paths = BTreeMap::new();
        if let Some(target) = target_metadata {
            for (parameter, path) in callee
                .parameters()
                .iter()
                .zip(&target.argument_element_paths)
            {
                if !path.is_empty() {
                    element_paths.insert(parameter.register, path.clone());
                }
            }
        }
        for operation in &callee.bytecode().code {
            match operation {
                WvmInstruction::Move { dst, src } => {
                    if let Some(path) = element_paths.get(src).cloned() {
                        element_paths.insert(*dst, path);
                    }
                }
                WvmInstruction::GetItem { dst, object, .. } => {
                    let Some(path) = element_paths.get(object).cloned() else {
                        continue;
                    };
                    if let Some(element_type) = path.first().copied() {
                        nested_element_types.insert(*object, element_type);
                    }
                    if path.first() == Some(&ValueType::Handle) && path.len() > 1 {
                        element_paths.insert(*dst, path[1..].to_vec());
                    }
                }
                _ => {}
            }
        }
        if let Some(target) = target_metadata {
            let index_constants = callee
                .bytecode()
                .code
                .iter()
                .filter_map(|operation| match operation {
                    WvmInstruction::ConstSmallInt { dst, value }
                    | WvmInstruction::ConstI64 { dst, value } => {
                        usize::try_from(*value).ok().map(|value| (*dst, value))
                    }
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>();
            let mut indexed_paths = callee
                .parameters()
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    target
                        .argument_indexed_element_types
                        .get(*index)
                        .is_some_and(|types| !types.is_empty())
                })
                .map(|(index, parameter)| (parameter.register, (index, Vec::new())))
                .collect::<BTreeMap<_, (usize, Vec<Option<usize>>)>>();
            for operation in &callee.bytecode().code {
                match operation {
                    WvmInstruction::Move { dst, src } => {
                        if let Some(path) = indexed_paths.get(src).cloned() {
                            indexed_paths.insert(*dst, path);
                        }
                    }
                    WvmInstruction::GetItem { dst, object, key } => {
                        let Some((argument, mut path)) = indexed_paths.get(object).cloned() else {
                            continue;
                        };
                        path.push(index_constants.get(key).copied());
                        let Some(types) = target.argument_indexed_element_types.get(argument)
                        else {
                            continue;
                        };
                        if let Some(ty) = indexed_element_type(types, &path) {
                            nested_element_types.insert(*dst, ty);
                            if ty == ValueType::Handle {
                                indexed_paths.insert(*dst, (argument, path));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for (callee_pc, operation) in callee.bytecode().code[..callee_region.entry]
            .iter()
            .enumerate()
        {
            match operation {
                WvmInstruction::ConstSmallInt { dst, value }
                | WvmInstruction::ConstI64 { dst, value } => {
                    let operation =
                        ops::lower(callee, callee_pc, operation, &locals, &nested_element_types)?
                            .ok_or_else(|| {
                            "adaptive-v2 nested integer constant is unsupported".to_owned()
                        })?;
                    self.emit_operation(pc, operation, &mut locals, lowered)?;
                    constants.insert(*dst, *value);
                }
                WvmInstruction::ConstFloat { .. } => {
                    let operation =
                        ops::lower(callee, callee_pc, operation, &locals, &nested_element_types)?
                            .ok_or_else(|| {
                            "adaptive-v2 nested float constant is unsupported".to_owned()
                        })?;
                    self.emit_operation(pc, operation, &mut locals, lowered)?;
                }
                WvmInstruction::GetItem { dst, object, key }
                    if tuple_items.is_some()
                        && callee
                            .parameters()
                            .first()
                            .is_some_and(|parameter| *object == parameter.register) =>
                {
                    let index = usize::try_from(*constants.get(key).ok_or_else(|| {
                        "adaptive-v2 nested tuple index is not constant".to_owned()
                    })?)
                    .map_err(|_| "adaptive-v2 nested tuple index is negative".to_owned())?;
                    let (_, tuple_items) = tuple_items
                        .as_ref()
                        .ok_or_else(|| "adaptive-v2 nested tuple disappeared".to_owned())?;
                    let (outer_register, item) = *tuple_items.get(index).ok_or_else(|| {
                        "adaptive-v2 nested tuple index is out of bounds".to_owned()
                    })?;
                    self.emit_operation(
                        pc,
                        ops::LoweredOp {
                            kind: InstructionKind::Copy,
                            inputs: vec![item.id],
                            dst: *dst,
                            ty: item.ty,
                        },
                        &mut locals,
                        lowered,
                    )?;
                    if item.ty == ValueType::Handle
                        && let Some(element_type) = self.element_types.get(&outer_register)
                    {
                        nested_element_types.insert(*dst, *element_type);
                    }
                }
                WvmInstruction::Length { .. } => {
                    let operation =
                        ops::lower(callee, callee_pc, operation, &locals, &nested_element_types)?
                            .ok_or_else(|| {
                            "adaptive-v2 nested list length is unsupported".to_owned()
                        })?;
                    self.emit_operation(pc, operation, &mut locals, lowered)?;
                }
                WvmInstruction::Move { dst, src } => {
                    let operation =
                        ops::lower(callee, callee_pc, operation, &locals, &nested_element_types)?
                            .ok_or_else(|| "adaptive-v2 nested move is unsupported".to_owned())?;
                    self.emit_operation(pc, operation, &mut locals, lowered)?;
                    if let Some(element_type) = nested_element_types.get(src).copied() {
                        nested_element_types.insert(*dst, element_type);
                    }
                }
                WvmInstruction::BuildList { dst, items }
                    if items.is_empty() && reused_result.is_some() =>
                {
                    let (outer_register, destination) = reused_result
                        .ok_or_else(|| "adaptive-v2 reused result disappeared".to_owned())?;
                    self.emit_operation(
                        pc,
                        ops::LoweredOp {
                            kind: InstructionKind::ListClear,
                            inputs: vec![destination.id],
                            dst: *dst,
                            ty: ValueType::Handle,
                        },
                        &mut locals,
                        lowered,
                    )?;
                    if let Some(element_type) = self.element_types.get(&outer_register).copied() {
                        nested_element_types.insert(*dst, element_type);
                    }
                }
                WvmInstruction::Jump { target }
                    if *target == callee_region.entry
                        || callee_region.blocks.iter().any(|block| {
                            callee
                                .structure_map()
                                .block(*block)
                                .is_some_and(|candidate| candidate.start_pc == *target)
                        }) => {}
                WvmInstruction::LoadConstant { .. } => {
                    return Err(
                        "adaptive-v2 nested prologue constant is not rematerializable".to_owned(),
                    );
                }
                _ => {
                    return Err(format!(
                        "adaptive-v2 nested prologue pc {callee_pc} is not pure/rematerializable: {operation:?}"
                    ));
                }
            }
        }
        let input_types = callee_region
            .entry_summary
            .iter()
            .map(|slot| self.read(&locals, slot.register).map(|value| value.ty))
            .collect::<Result<Vec<_>, _>>()?;
        let mut child = super::lower_typed(
            callee,
            callee_region,
            backedge,
            &input_types,
            &nested_element_types,
            &nested_call_targets,
            self.constant_call_targets,
            crate::adaptive_v2::trace::ExecutableIdentity::new(
                callee.id().as_u64(),
                callee.id().as_u64(),
            ),
            self.dependencies,
            &[],
            &[],
            None,
        )
        .map_err(|error| format!("adaptive-v2 wrapper child lowering failed: {error}"))?;
        let nested_storage = super::super::storage_live_destinations(callee, callee_region_id);
        elide_dead_numeric_phis(&mut child, callee, &nested_storage);
        widen_numeric_phis(&mut child, &input_types)?;
        elide_invariant_handle_phis(&mut child);
        self.splice_child_loop(
            pc,
            terminal_pc,
            *dst,
            tuple_items.as_ref(),
            callee,
            callee_region_id,
            callee_region,
            &locals,
            child,
            values,
            active,
        )
        .map(Some)
    }

    #[allow(clippy::too_many_arguments)]
    fn try_splice_loopless_wrapper_call(
        &mut self,
        pc: usize,
        terminal_pc: usize,
        outer_dst: Register,
        args: &[Register],
        wrapper: &crate::executable::ExecutableFunction,
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
        active: &mut Vec<usize>,
    ) -> Result<Option<Terminator>, String> {
        if wrapper.parameters().len() != args.len() {
            return Ok(None);
        }
        let calls = wrapper
            .bytecode()
            .code
            .iter()
            .enumerate()
            .filter(|(_, instruction)| matches!(instruction, WvmInstruction::Call { .. }))
            .collect::<Vec<_>>();
        if calls.len() != 2
            || !matches!(
                wrapper.bytecode().code.last(),
                Some(WvmInstruction::Return { .. })
            )
        {
            return Ok(None);
        }
        let destination_register = self
            .executable
            .bytecode()
            .code
            .get(pc.saturating_add(1))
            .and_then(|next| match next {
                WvmInstruction::Move { dst: target, src } if *src == outer_dst => Some(*target),
                _ => None,
            });
        let Some(destination_register) = destination_register else {
            return Ok(None);
        };
        let destination = if let Some(destination) = values
            .get(&destination_register)
            .copied()
            .filter(|value| value.ty == ValueType::Handle)
        {
            destination
        } else {
            let [source] = args else {
                return Ok(None);
            };
            let source = self.read(values, *source)?;
            if source.ty != ValueType::Handle {
                return Ok(None);
            }
            let capacity = self.define(values, destination_register, ValueType::I64);
            lowered.push(Instruction::new(
                InstructionKind::ListLength.at_pc(pc_u32(pc)?),
                vec![source.id],
                Some(capacity),
                Effect::Read,
            ));
            let destination = self.define(values, destination_register, ValueType::Handle);
            lowered.push(Instruction::new(
                InstructionKind::OwnedList {
                    identity: 2,
                    element_type: ValueType::F64,
                    reset_on_definition: false,
                    copy_from_source: false,
                },
                vec![capacity.id],
                Some(destination),
                Effect::Pure,
            ));
            let destination = self.read(values, destination_register)?;
            self.owned_lists.insert(2, destination);
            destination
        };
        let mut wrapper_values = wrapper
            .parameters()
            .iter()
            .zip(args)
            .map(|(parameter, argument)| {
                self.read(values, *argument)
                    .map(|value| (parameter.register, value))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let mut wrapper_element_types = BTreeMap::new();
        let mut wrapper_call_targets = BTreeMap::new();
        for (parameter, argument) in wrapper.parameters().iter().zip(args) {
            if let Some(element_type) = self.element_types.get(argument).copied() {
                wrapper_element_types.insert(parameter.register, element_type);
            }
            if let Some(target) = self.call_targets.get(argument).cloned() {
                wrapper_call_targets.insert(parameter.register, target);
            }
        }
        for instruction in &wrapper.bytecode().code {
            let WvmInstruction::LoadConstant { dst, .. } = instruction else {
                continue;
            };
            let Some(target) = self
                .constant_call_targets
                .get(&(wrapper.id().as_u64(), *dst))
                .cloned()
            else {
                continue;
            };
            let output = self.define(&mut wrapper_values, *dst, ValueType::Handle);
            lowered.push(Instruction::new(
                InstructionKind::Constant(crate::adaptive_v2::wxir_v2::ir::Constant::HandleBits(
                    u64::from(target.handle),
                ))
                .at_pc(pc_u32(pc)?),
                Vec::new(),
                Some(output),
                Effect::Pure,
            ));
            wrapper_call_targets.insert(*dst, target);
        }
        let (first_pc, first_instruction) = calls[0];
        let mut first_instructions = Vec::new();
        let first = self
            .prepare_wrapper_child(
                wrapper,
                first_pc,
                first_instruction,
                &mut wrapper_values,
                &mut wrapper_element_types,
                &wrapper_call_targets,
                NestedListDestination::Owned { identity: 1 },
                &mut first_instructions,
            )
            .map_err(|error| format!("adaptive-v2 first wrapper preparation failed: {error}"))?;
        lowered.extend(first_instructions);
        let first_spliced = self
            .append_child_loop(
                pc,
                match first_instruction {
                    WvmInstruction::Call { dst, .. } => *dst,
                    _ => return Ok(None),
                },
                None,
                &first.callee,
                &first.region,
                &first.locals,
                first.child,
                &mut wrapper_values,
            )
            .map_err(|error| format!("adaptive-v2 first wrapper append failed: {error}"))?;
        let (second_pc, second_instruction) = calls[1];
        for operation in &wrapper.bytecode().code[first_pc + 1..second_pc] {
            if let WvmInstruction::Move { dst, src } = operation {
                let value = self.read(&wrapper_values, *src)?;
                wrapper_values.insert(*dst, value);
                if let Some(element_type) = wrapper_element_types.get(src).copied() {
                    wrapper_element_types.insert(*dst, element_type);
                }
            }
        }
        let mut second_instructions = Vec::new();
        let second = self
            .prepare_wrapper_child(
                wrapper,
                second_pc,
                second_instruction,
                &mut wrapper_values,
                &mut wrapper_element_types,
                &wrapper_call_targets,
                NestedListDestination::Entry(destination),
                &mut second_instructions,
            )
            .map_err(|error| format!("adaptive-v2 second wrapper preparation failed: {error}"))?;
        let second_dst = match second_instruction {
            WvmInstruction::Call { dst, .. } => *dst,
            _ => return Ok(None),
        };
        let second_spliced = self
            .append_child_loop(
                pc,
                second_dst,
                None,
                &second.callee,
                &second.region,
                &second.locals,
                second.child,
                &mut wrapper_values,
            )
            .map_err(|error| format!("adaptive-v2 second wrapper append failed: {error}"))?;
        let first_output = first_spliced
            .output
            .ok_or_else(|| "adaptive-v2 first wrapper result is not loop-carried".to_owned())?;
        let second_output = second_spliced
            .output
            .ok_or_else(|| "adaptive-v2 second wrapper result is not loop-carried".to_owned())?;
        self.blocks.push(Block::new(
            first_spliced.continuation,
            vec![first_output],
            second_instructions,
            second_spliced.entry,
        ));
        values.insert(
            outer_dst,
            SsaValue {
                id: second_output.id,
                ty: second_output.ty,
            },
        );
        let mut continuation_instructions = Vec::new();
        for (offset, operation) in self.executable.bytecode().code[pc + 1..terminal_pc]
            .iter()
            .enumerate()
        {
            if let Some(terminator) = self.try_splice_nested_loop_call(
                pc + 1 + offset,
                operation,
                terminal_pc,
                values,
                &mut continuation_instructions,
                active,
            )? {
                self.blocks.push(Block::new(
                    second_spliced.continuation,
                    vec![second_output],
                    continuation_instructions,
                    terminator,
                ));
                return Ok(Some(first_spliced.entry));
            }
            self.lower_instruction(
                pc + 1 + offset,
                operation,
                values,
                &mut continuation_instructions,
            )?;
        }
        let terminal = self
            .executable
            .bytecode()
            .code
            .get(terminal_pc)
            .ok_or_else(|| "adaptive-v2 wrapper caller terminal is missing".to_owned())?;
        let continuation_terminator = self.lower_terminator(
            terminal_pc,
            terminal,
            values,
            &mut continuation_instructions,
            true,
            false,
            active,
        )?;
        self.blocks.push(Block::new(
            second_spliced.continuation,
            vec![second_output],
            continuation_instructions,
            continuation_terminator,
        ));
        Ok(Some(first_spliced.entry))
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_wrapper_child(
        &mut self,
        caller: &crate::executable::ExecutableFunction,
        call_pc: usize,
        instruction: &WvmInstruction,
        values: &mut BTreeMap<Register, SsaValue>,
        element_types: &mut BTreeMap<Register, ValueType>,
        call_targets: &BTreeMap<Register, super::super::CallTarget>,
        destination: NestedListDestination,
        lowered: &mut Vec<Instruction>,
    ) -> Result<PreparedNestedChild, String> {
        let WvmInstruction::Call {
            dst,
            callable,
            args,
        } = instruction
        else {
            return Err("adaptive-v2 wrapper child is not a call".to_owned());
        };
        let callee = ops::constant_callee(caller, call_pc, *callable)
            .or_else(|| call_targets.get(callable).map(|target| &target.function))
            .ok_or_else(|| "adaptive-v2 wrapper child target is missing".to_owned())?
            .clone();
        crate::verifier::verify(&callee)?;
        let mut loops = callee.structure_map().loop_regions();
        let (_, region) = loops
            .next()
            .ok_or_else(|| "adaptive-v2 wrapper child has no loop".to_owned())?;
        if loops.next().is_some() {
            return Err("adaptive-v2 wrapper child has multiple loops".to_owned());
        }
        let crate::structure_map::RegionKind::Loop { backedge } = region.kind else {
            return Err("adaptive-v2 wrapper child region is not a loop".to_owned());
        };
        if callee.parameters().len() != args.len() {
            return Err("adaptive-v2 wrapper child arity is unsupported".to_owned());
        }
        let mut locals = BTreeMap::new();
        let mut nested_element_types = BTreeMap::new();
        let mut nested_call_targets = BTreeMap::new();
        for (parameter, argument) in callee.parameters().iter().zip(args) {
            let value = self.read(values, *argument)?;
            locals.insert(parameter.register, value);
            if let Some(element_type) = element_types.get(argument).copied() {
                nested_element_types.insert(parameter.register, element_type);
            }
            if let Some(target) = call_targets.get(argument).cloned() {
                nested_call_targets.insert(parameter.register, target);
            }
        }
        let mut capacity = None;
        let mut pending_owned: Option<(Vec<Register>, u32)> = None;
        for (callee_pc, operation) in callee.bytecode().code[..region.entry].iter().enumerate() {
            match operation {
                WvmInstruction::ConstSmallInt { .. }
                | WvmInstruction::ConstI64 { .. }
                | WvmInstruction::ConstFloat { .. }
                | WvmInstruction::Length { .. } => {
                    let lowered_operation = ops::lower(
                        &callee,
                        callee_pc,
                        operation,
                        &locals,
                        &nested_element_types,
                    )?
                    .ok_or_else(|| {
                        "adaptive-v2 wrapper child prologue is unsupported".to_owned()
                    })?;
                    let result_register = lowered_operation.dst;
                    self.emit_operation(call_pc, lowered_operation, &mut locals, lowered)?;
                    if matches!(operation, WvmInstruction::Length { .. }) {
                        capacity = Some(self.read(&locals, result_register)?);
                        if let Some((lists, identity)) = pending_owned.take() {
                            let list = lists[0];
                            let output = self.define(&mut locals, list, ValueType::Handle);
                            lowered.push(Instruction::new(
                                InstructionKind::OwnedList {
                                    identity,
                                    element_type: ValueType::F64,
                                    reset_on_definition: true,
                                    copy_from_source: false,
                                },
                                vec![self.read(&locals, result_register)?.id],
                                Some(output),
                                Effect::Pure,
                            ));
                            let target = self.read(&locals, list)?;
                            self.owned_lists.insert(identity, target);
                            nested_element_types.insert(list, ValueType::F64);
                            for alias in lists.into_iter().skip(1) {
                                self.emit_operation(
                                    call_pc,
                                    ops::LoweredOp {
                                        kind: InstructionKind::Copy,
                                        inputs: vec![self.read(&locals, list)?.id],
                                        dst: alias,
                                        ty: ValueType::Handle,
                                    },
                                    &mut locals,
                                    lowered,
                                )?;
                                nested_element_types.insert(alias, ValueType::F64);
                            }
                        }
                    }
                }
                WvmInstruction::Move { dst, src }
                    if pending_owned
                        .as_ref()
                        .is_some_and(|(lists, _)| lists.contains(src)) =>
                {
                    if let Some((lists, _)) = pending_owned.as_mut() {
                        lists.push(*dst);
                    }
                }
                WvmInstruction::Move { .. } => {
                    let lowered_operation = ops::lower(
                        &callee,
                        callee_pc,
                        operation,
                        &locals,
                        &nested_element_types,
                    )?
                    .ok_or_else(|| "adaptive-v2 wrapper child move is unsupported".to_owned())?;
                    self.emit_operation(call_pc, lowered_operation, &mut locals, lowered)?;
                }
                WvmInstruction::BuildList { dst: list, items } if items.is_empty() => {
                    let target = match destination {
                        NestedListDestination::Entry(target) => target,
                        NestedListDestination::Owned { identity } => {
                            if let Some(target) = self.owned_lists.get(&identity).copied() {
                                target
                            } else {
                                pending_owned = Some((vec![*list], identity));
                                continue;
                            }
                        }
                    };
                    self.emit_operation(
                        call_pc,
                        ops::LoweredOp {
                            kind: InstructionKind::ListClear,
                            inputs: vec![target.id],
                            dst: *list,
                            ty: ValueType::Handle,
                        },
                        &mut locals,
                        lowered,
                    )?;
                    nested_element_types.insert(*list, ValueType::F64);
                }
                WvmInstruction::Jump { target } if *target == region.entry => {}
                _ => {
                    return Err(format!(
                        "adaptive-v2 wrapper child prologue pc {callee_pc} is unsupported: {operation:?}"
                    ));
                }
            }
        }
        if pending_owned.is_some() || capacity.is_none() {
            return Err("adaptive-v2 owned list has no extent".to_owned());
        }
        let input_types = region
            .entry_summary
            .iter()
            .map(|slot| self.read(&locals, slot.register).map(|value| value.ty))
            .collect::<Result<Vec<_>, _>>()?;
        let mut child = super::lower_typed(
            &callee,
            region,
            backedge,
            &input_types,
            &nested_element_types,
            &nested_call_targets,
            self.constant_call_targets,
            crate::adaptive_v2::trace::ExecutableIdentity::new(
                callee.id().as_u64(),
                callee.id().as_u64(),
            ),
            self.dependencies,
            &[],
            &[],
            None,
        )?;
        widen_numeric_phis(&mut child, &input_types)?;
        elide_invariant_handle_phis(&mut child);
        element_types.insert(*dst, ValueType::F64);
        Ok(PreparedNestedChild {
            callee: callee.clone(),
            region: region.clone(),
            locals,
            child,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn splice_child_loop(
        &mut self,
        pc: usize,
        terminal_pc: usize,
        dst: Register,
        tuple_items: Option<&(Register, Vec<(Register, SsaValue)>)>,
        callee: &crate::executable::ExecutableFunction,
        _callee_region_id: crate::structure_map::RegionId,
        callee_region: &crate::structure_map::Region,
        locals: &BTreeMap<Register, SsaValue>,
        child: super::LoweredLoop,
        values: &mut BTreeMap<Register, SsaValue>,
        active: &mut Vec<usize>,
    ) -> Result<Terminator, String> {
        let spliced = self.append_child_loop(
            pc,
            dst,
            tuple_items,
            callee,
            callee_region,
            locals,
            child,
            values,
        )?;
        let mut continuation_instructions = Vec::new();
        for (offset, operation) in self.executable.bytecode().code[pc + 1..terminal_pc]
            .iter()
            .enumerate()
        {
            if let Some(terminator) = self.try_splice_nested_loop_call(
                pc + 1 + offset,
                operation,
                terminal_pc,
                values,
                &mut continuation_instructions,
                active,
            )? {
                self.blocks.push(Block::new(
                    spliced.continuation,
                    spliced.output.into_iter().collect(),
                    continuation_instructions,
                    terminator,
                ));
                return Ok(spliced.entry);
            }
            self.lower_instruction(
                pc + 1 + offset,
                operation,
                values,
                &mut continuation_instructions,
            )?;
        }
        let terminal = self
            .executable
            .bytecode()
            .code
            .get(terminal_pc)
            .ok_or_else(|| "adaptive-v2 nested caller terminal is missing".to_owned())?;
        let continuation_terminator = self.lower_terminator(
            terminal_pc,
            terminal,
            values,
            &mut continuation_instructions,
            true,
            false,
            active,
        )?;
        self.blocks.push(Block::new(
            spliced.continuation,
            spliced.output.into_iter().collect(),
            continuation_instructions,
            continuation_terminator,
        ));
        Ok(spliced.entry)
    }

    #[allow(clippy::too_many_arguments)]
    fn append_child_loop(
        &mut self,
        pc: usize,
        dst: Register,
        tuple_items: Option<&(Register, Vec<(Register, SsaValue)>)>,
        callee: &crate::executable::ExecutableFunction,
        callee_region: &crate::structure_map::Region,
        locals: &BTreeMap<Register, SsaValue>,
        child: super::LoweredLoop,
        values: &mut BTreeMap<Register, SsaValue>,
    ) -> Result<SplicedChild, String> {
        let return_register = callee
            .bytecode()
            .code
            .iter()
            .rev()
            .find_map(|instruction| match instruction {
                WvmInstruction::Return { src } => Some(*src),
                _ => None,
            })
            .ok_or_else(|| "adaptive-v2 nested callee has no return".to_owned())?;
        let return_position = callee_region
            .entry_summary
            .iter()
            .position(|slot| slot.register == return_register);
        let return_value = return_position.and_then(|position| {
            child
                .blocks
                .iter()
                .find_map(|block| match &block.terminator {
                    Terminator::SideExit { values, .. } => values.get(position).copied(),
                    _ => None,
                })
        });
        let return_type = return_value.and_then(|return_value| {
            child
                .blocks
                .iter()
                .flat_map(|block| {
                    block.parameters.iter().chain(
                        block
                            .instructions
                            .iter()
                            .filter_map(|instruction| instruction.output.as_ref()),
                    )
                })
                .find(|definition| definition.id == return_value)
                .map(|definition| definition.ty)
        });
        let child_entry = child
            .blocks
            .first()
            .map(|block| block.id)
            .ok_or_else(|| "adaptive-v2 nested child entry is missing".to_owned())?;
        let value_base = self.next_value;
        let max_child_value = child
            .blocks
            .iter()
            .flat_map(|block| {
                block.parameters.iter().chain(
                    block
                        .instructions
                        .iter()
                        .filter_map(|instruction| instruction.output.as_ref()),
                )
            })
            .map(|value| value.id.get())
            .max()
            .unwrap_or(0);
        self.next_value = self
            .next_value
            .checked_add(max_child_value.saturating_add(1))
            .ok_or_else(|| "adaptive-v2 nested value id overflow".to_owned())?;
        let remap_value = |value: ValueId| ValueId::new(value_base.saturating_add(value.get()));
        let child_virtual_ids = child
            .deopts
            .iter()
            .flat_map(|recipe| recipe.virtuals.iter().map(|virtual_value| virtual_value.id))
            .collect::<BTreeSet<_>>();
        let mut virtual_map = BTreeMap::new();
        for id in child_virtual_ids {
            let remapped = self.next_virtual;
            self.next_virtual = self
                .next_virtual
                .checked_add(1)
                .ok_or_else(|| "adaptive-v2 nested virtual id overflow".to_owned())?;
            virtual_map.insert(id, remapped);
        }
        let block_base = self.next_block;
        let block_map = child
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                Ok((
                    block.id,
                    BlockId::new(
                        block_base
                            .checked_add(u32::try_from(index).map_err(|_| {
                                "adaptive-v2 nested block index overflow".to_owned()
                            })?)
                            .ok_or_else(|| "adaptive-v2 nested block id overflow".to_owned())?,
                    ),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        self.next_block = block_base
            .checked_add(
                u32::try_from(child.blocks.len())
                    .map_err(|_| "adaptive-v2 nested block count overflow".to_owned())?,
            )
            .ok_or_else(|| "adaptive-v2 nested block id overflow".to_owned())?;
        let continuation = BlockId::new(self.next_block);
        self.next_block = self.next_block.saturating_add(1);
        let safepoint_base = self.next_safepoint;
        let max_child_safepoint = child
            .root_maps
            .iter()
            .map(|map| map.point.get())
            .max()
            .unwrap_or(0);
        self.next_safepoint = self
            .next_safepoint
            .checked_add(max_child_safepoint)
            .ok_or_else(|| "adaptive-v2 nested safepoint overflow".to_owned())?;
        let remap_point = |point: SafepointId| {
            SafepointId::new(safepoint_base.saturating_add(point.get().saturating_sub(1)))
        };
        let caller_values = values.clone();
        let mut remapped_types = child
            .blocks
            .iter()
            .flat_map(|block| {
                block.parameters.iter().chain(
                    block
                        .instructions
                        .iter()
                        .filter_map(|instruction| instruction.output.as_ref()),
                )
            })
            .map(|definition| (remap_value(definition.id), definition.ty))
            .collect::<BTreeMap<_, _>>();
        remapped_types.extend(caller_values.values().map(|value| (value.id, value.ty)));
        let output = return_type.map(|return_type| self.define(values, dst, return_type));
        let virtual_id = if tuple_items.is_some() {
            let id = self.next_virtual;
            self.next_virtual = self
                .next_virtual
                .checked_add(1)
                .ok_or_else(|| "adaptive-v2 nested virtual tuple id overflow".to_owned())?;
            Some(id)
        } else {
            None
        };
        let outer_roots = caller_values
            .values()
            .filter(|value| value.ty == ValueType::Handle)
            .map(|value| RootLocation::Ssa(value.id))
            .collect::<Vec<_>>();
        for map in child.root_maps {
            let mut roots = map
                .roots
                .into_iter()
                .map(|root| remap_root(root, value_base, &virtual_map))
                .collect::<BTreeSet<_>>();
            roots.extend(outer_roots.iter().copied());
            roots.extend(
                self.owned_lists
                    .keys()
                    .copied()
                    .map(RootLocation::OwnedList),
            );
            if let Some(virtual_id) = virtual_id {
                roots.insert(RootLocation::Virtual(virtual_id));
                roots.insert(RootLocation::DeoptWorklist);
            }
            self.root_maps
                .push(RootMap::new(remap_point(map.point), roots));
        }
        for mut recipe in child.deopts {
            recipe.id = remap_point(SafepointId::new(recipe.id)).get();
            recipe.executable = self.identity;
            recipe.resume_pc = pc_u32(pc)?;
            recipe.mode = ResumeMode::ReplayBeforePc;
            recipe.root_point = remap_point(recipe.root_point);
            recipe.explicit_roots = recipe
                .explicit_roots
                .into_iter()
                .map(|root| remap_root(root, value_base, &virtual_map))
                .collect();
            for frame in &mut recipe.frames {
                for register in &mut frame.registers {
                    register.source =
                        remap_source(register.source.clone(), value_base, &virtual_map);
                    if let Some(virtual_id) = virtual_id
                        && register.register == callee.parameters()[0].register
                    {
                        register.source = RegisterSource::Virtual(virtual_id);
                    }
                }
            }
            for virtual_value in &mut recipe.virtuals {
                virtual_value.id = virtual_map[&virtual_value.id];
                remap_virtual_kind(&mut virtual_value.kind, value_base, &virtual_map);
            }
            let caller = self
                .deopt(recipe.root_point, &caller_values, pc_u32(pc)?, recipe.id)?
                .frames
                .into_iter()
                .next()
                .ok_or_else(|| "adaptive-v2 nested caller frame is missing".to_owned())?;
            recipe.frames.insert(0, caller);
            if let (Some(virtual_id), Some((_, tuple_items))) = (virtual_id, tuple_items) {
                recipe.virtuals.push(VirtualRecipe {
                    id: virtual_id,
                    kind: VirtualKind::Tuple {
                        items: tuple_items
                            .iter()
                            .map(|(_, item)| RegisterSource::Ssa(item.id))
                            .collect(),
                    },
                });
            }
            recipe.dependencies = self.dependencies.to_vec();
            let mut exact_roots = recipe
                .explicit_roots
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            exact_roots.extend(
                self.owned_lists
                    .keys()
                    .copied()
                    .map(RootLocation::OwnedList),
            );
            for frame in &recipe.frames {
                for register in &frame.registers {
                    match register.source {
                        RegisterSource::Ssa(value) if register.ty == ValueType::Handle => {
                            exact_roots.insert(RootLocation::Ssa(value));
                        }
                        RegisterSource::Spill {
                            slot,
                            ty: ValueType::Handle,
                        } => {
                            exact_roots.insert(RootLocation::Spill(slot));
                        }
                        RegisterSource::Virtual(id) if register.ty == ValueType::Handle => {
                            exact_roots.insert(RootLocation::Virtual(id));
                        }
                        _ => {}
                    }
                }
            }
            if !recipe.virtuals.is_empty() {
                exact_roots.extend(
                    recipe
                        .virtuals
                        .iter()
                        .map(|virtual_value| RootLocation::Virtual(virtual_value.id)),
                );
                exact_roots.insert(RootLocation::DeoptWorklist);
                for virtual_value in &recipe.virtuals {
                    let sources = match &virtual_value.kind {
                        VirtualKind::Object { fields, .. } => {
                            fields.iter().map(|(_, source)| source).collect::<Vec<_>>()
                        }
                        VirtualKind::List { items } | VirtualKind::Tuple { items } => {
                            items.iter().collect::<Vec<_>>()
                        }
                    };
                    for source in sources {
                        match source {
                            RegisterSource::Ssa(value)
                                if remapped_types.get(value) == Some(&ValueType::Handle) =>
                            {
                                exact_roots.insert(RootLocation::Ssa(*value));
                            }
                            RegisterSource::Spill {
                                slot,
                                ty: ValueType::Handle,
                            } => {
                                exact_roots.insert(RootLocation::Spill(*slot));
                            }
                            RegisterSource::Ssa(_)
                            | RegisterSource::Constant(_)
                            | RegisterSource::Spill { .. }
                            | RegisterSource::Virtual(_)
                            | RegisterSource::UndefinedDead => {}
                        }
                    }
                }
            }
            let map = self
                .root_maps
                .iter_mut()
                .find(|map| map.point == recipe.root_point)
                .ok_or_else(|| "adaptive-v2 nested root map is missing".to_owned())?;
            map.roots = exact_roots;
            self.deopts.push(recipe);
        }
        let mut side_exits = 0;
        for block in child.blocks {
            let parameters = block
                .parameters
                .into_iter()
                .map(|parameter| ValueDef::new(remap_value(parameter.id), parameter.ty))
                .collect();
            let instructions = block
                .instructions
                .into_iter()
                .map(|mut instruction| {
                    instruction.inputs = instruction.inputs.into_iter().map(remap_value).collect();
                    instruction.output = instruction
                        .output
                        .map(|output| ValueDef::new(remap_value(output.id), output.ty));
                    instruction.safepoint = instruction.safepoint.map(remap_point);
                    instruction
                })
                .collect();
            let terminator = match block.terminator {
                Terminator::Jump { target, arguments } => Terminator::Jump {
                    target: block_map[&target],
                    arguments: arguments.into_iter().map(remap_value).collect(),
                },
                Terminator::Branch { condition, yes, no } => Terminator::Branch {
                    condition: remap_value(condition),
                    yes: block_map[&yes],
                    no: block_map[&no],
                },
                Terminator::SideExit { values, .. } => {
                    side_exits += 1;
                    Terminator::Jump {
                        target: continuation,
                        arguments: return_position
                            .map(|position| vec![remap_value(values[position])])
                            .unwrap_or_default(),
                    }
                }
                Terminator::Return { .. }
                | Terminator::Backedge { .. }
                | Terminator::IrreducibleBackedge => {
                    return Err("adaptive-v2 nested child terminator is unsupported".to_owned());
                }
            };
            self.blocks.push(Block::new(
                block_map[&block.id],
                parameters,
                instructions,
                terminator,
            ));
        }
        if side_exits != 1 {
            return Err("adaptive-v2 nested child must have one normal exit".to_owned());
        }
        Ok(SplicedChild {
            entry: Terminator::Jump {
                target: block_map[&child_entry],
                arguments: callee_region
                    .entry_summary
                    .iter()
                    .map(|slot| self.read(locals, slot.register).map(|value| value.id))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            continuation,
            output,
        })
    }

    pub(super) fn lower_instruction(
        &mut self,
        pc: usize,
        instruction: &WvmInstruction,
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
    ) -> Result<(), String> {
        if let WvmInstruction::BinaryOp {
            dst, op, lhs, rhs, ..
        } = instruction
        {
            return self.lower_binary(pc, *dst, *op, *lhs, *rhs, None, values, lowered);
        }
        if let WvmInstruction::Call {
            dst,
            callable,
            args,
        } = instruction
        {
            return self.inline_call(pc, *dst, *callable, args, values, lowered);
        }
        if let WvmInstruction::BuildTuple { dst, items } = instruction {
            let items = items
                .iter()
                .map(|item| self.read(values, *item).map(|value| (*item, value)))
                .collect::<Result<Vec<_>, _>>()?;
            self.virtual_tuples.insert(*dst, items);
            return Ok(());
        }
        if let WvmInstruction::GetSlice {
            dst,
            object,
            start: None,
            stop: None,
            step: None,
        } = instruction
        {
            let source = self.read(values, *object)?;
            if source.ty != ValueType::Handle {
                return Err("adaptive-v2 full list copy requires a handle".to_owned());
            }
            let element_type = self
                .element_types
                .get(object)
                .or_else(|| self.element_types.get(dst))
                .copied()
                .filter(|ty| matches!(ty, ValueType::I64 | ValueType::F64))
                .ok_or_else(|| {
                    "adaptive-v2 full list copy requires a stable numeric strategy".to_owned()
                })?;
            let identity = (1_u32..=2)
                .find(|identity| !self.owned_lists.contains_key(identity))
                .ok_or_else(|| "adaptive-v2 loop supports at most two owned lists".to_owned())?;
            let capacity = self.define(values, *dst, ValueType::I64);
            lowered.push(Instruction::new(
                InstructionKind::ListLength.at_pc(pc_u32(pc)?),
                vec![source.id],
                Some(capacity),
                Effect::Read,
            ));
            let owned = self.define(values, *dst, ValueType::Handle);
            lowered.push(Instruction::new(
                InstructionKind::OwnedList {
                    identity,
                    element_type,
                    reset_on_definition: false,
                    copy_from_source: true,
                }
                .at_pc(pc_u32(pc)?),
                vec![capacity.id, source.id],
                Some(owned),
                Effect::Read,
            ));
            self.owned_lists.insert(
                identity,
                SsaValue {
                    id: owned.id,
                    ty: owned.ty,
                },
            );
            self.copied_list_element_types
                .insert(owned.id, element_type);
            return Ok(());
        }
        if let WvmInstruction::LoadConstant { dst, constant } = instruction {
            if ops::is_inlined_constant(self.executable, pc, *dst, constant.0) {
                return Ok(());
            }
            if let Some(target) = self.call_targets.get(dst)
                && self.executable.bytecode().code[pc.saturating_add(1)..]
                    .iter()
                    .any(|operation| {
                        matches!(operation, WvmInstruction::Call { args, .. } if args.contains(dst))
                    })
            {
                let output = self.define(values, *dst, ValueType::Handle);
                lowered.push(Instruction::new(
                    InstructionKind::Constant(
                        crate::adaptive_v2::wxir_v2::ir::Constant::HandleBits(u64::from(
                            target.handle,
                        )),
                    )
                    .at_pc(pc_u32(pc)?),
                    Vec::new(),
                    Some(output),
                    Effect::Pure,
                ));
                return Ok(());
            }
        }
        let copied_element_type = match instruction {
            WvmInstruction::Move { src, .. } => self
                .copied_list_element_types
                .get(&self.read(values, *src)?.id)
                .copied(),
            _ => None,
        };
        let operation = ops::lower(self.executable, pc, instruction, values, self.element_types)?
            .ok_or_else(|| {
            format!("unsupported WVM instruction at pc {pc} in adaptive-v2 loop")
        })?;
        self.emit_operation(pc, operation, values, lowered)?;
        if let (Some(element_type), WvmInstruction::Move { dst, .. }) =
            (copied_element_type, instruction)
        {
            let moved = self.read(values, *dst)?;
            self.copied_list_element_types
                .insert(moved.id, element_type);
        }
        Ok(())
    }

    fn emit_operation(
        &mut self,
        pc: usize,
        operation: ops::LoweredOp,
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
    ) -> Result<(), String> {
        let effect = match operation.kind.semantic() {
            InstructionKind::ListGet | InstructionKind::ListLength => Effect::Read,
            InstructionKind::ListSet
            | InstructionKind::ListReversePrefix { .. }
            | InstructionKind::ListClear
            | InstructionKind::ListAppend
            | InstructionKind::ListInsert
            | InstructionKind::ListPop => Effect::Write,
            _ => Effect::Pure,
        };
        let output = if matches!(
            operation.kind.semantic(),
            InstructionKind::ListSet | InstructionKind::ListReversePrefix { .. }
        ) {
            None
        } else {
            Some(self.define(values, operation.dst, operation.ty))
        };
        let order = lowered
            .iter()
            .filter(|instruction| instruction.effect.is_ordered())
            .count() as u32;
        let instruction = Instruction::new(
            operation.kind.at_pc(pc_u32(pc)?),
            operation.inputs,
            output,
            effect,
        );
        lowered.push(if effect.is_ordered() {
            instruction.ordered(order)
        } else {
            instruction
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_binary(
        &mut self,
        pc: usize,
        dst: Register,
        op: crate::bytecode::BinaryOperator,
        lhs: Register,
        rhs: Register,
        floor_divisor: Option<i64>,
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
    ) -> Result<(), String> {
        let mut left = self.read(values, lhs)?;
        let mut right = self.read(values, rhs)?;
        let float_result = matches!(
            op,
            crate::bytecode::BinaryOperator::Divide
                | crate::bytecode::BinaryOperator::Add
                | crate::bytecode::BinaryOperator::Subtract
                | crate::bytecode::BinaryOperator::Multiply
                | crate::bytecode::BinaryOperator::Power
        ) && (left.ty == ValueType::F64
            || right.ty == ValueType::F64
            || matches!(op, crate::bytecode::BinaryOperator::Divide));
        if float_result {
            if left.ty == ValueType::I64 {
                let temporary = temporary_register(values)?;
                self.emit_operation(
                    pc,
                    ops::LoweredOp {
                        kind: InstructionKind::IntegerToFloat,
                        inputs: vec![left.id],
                        dst: temporary,
                        ty: ValueType::F64,
                    },
                    values,
                    lowered,
                )?;
                left = self.read(values, temporary)?;
            }
            if right.ty == ValueType::I64 {
                let temporary = temporary_register(values)?;
                self.emit_operation(
                    pc,
                    ops::LoweredOp {
                        kind: InstructionKind::IntegerToFloat,
                        inputs: vec![right.id],
                        dst: temporary,
                        ty: ValueType::F64,
                    },
                    values,
                    lowered,
                )?;
                right = self.read(values, temporary)?;
            }
        }
        let (kind, ty, inputs) = match (op, left.ty, right.ty) {
            (crate::bytecode::BinaryOperator::Add, ValueType::I64, ValueType::I64) => (
                InstructionKind::IntegerAdd,
                ValueType::I64,
                vec![left.id, right.id],
            ),
            (crate::bytecode::BinaryOperator::Subtract, ValueType::I64, ValueType::I64) => (
                InstructionKind::IntegerSubtract,
                ValueType::I64,
                vec![left.id, right.id],
            ),
            (crate::bytecode::BinaryOperator::Multiply, ValueType::I64, ValueType::I64) => (
                InstructionKind::IntegerMultiply,
                ValueType::I64,
                vec![left.id, right.id],
            ),
            (crate::bytecode::BinaryOperator::FloorDivide, ValueType::I64, ValueType::I64)
                if floor_divisor.is_some() =>
            {
                let divisor = floor_divisor.expect("matched a verified floor divisor");
                (
                    InstructionKind::IntegerFloorDivide { divisor },
                    ValueType::I64,
                    vec![left.id],
                )
            }
            (crate::bytecode::BinaryOperator::Add, ValueType::F64, ValueType::F64) => (
                InstructionKind::FloatAdd,
                ValueType::F64,
                vec![left.id, right.id],
            ),
            (crate::bytecode::BinaryOperator::Subtract, ValueType::F64, ValueType::F64) => (
                InstructionKind::FloatSubtract,
                ValueType::F64,
                vec![left.id, right.id],
            ),
            (crate::bytecode::BinaryOperator::Multiply, ValueType::F64, ValueType::F64) => (
                InstructionKind::FloatMultiply,
                ValueType::F64,
                vec![left.id, right.id],
            ),
            (crate::bytecode::BinaryOperator::Divide, ValueType::F64, ValueType::F64) => (
                InstructionKind::FloatDivide,
                ValueType::F64,
                vec![left.id, right.id],
            ),
            (crate::bytecode::BinaryOperator::Power, ValueType::F64, ValueType::F64) => (
                InstructionKind::FloatPower,
                ValueType::F64,
                vec![left.id, right.id],
            ),
            _ => {
                return Err(format!(
                    "adaptive-v2 loop arithmetic operand types are unsupported at pc {pc}: {op:?} {:?} {:?}",
                    left.ty, right.ty
                ));
            }
        };
        self.emit_operation(
            pc,
            ops::LoweredOp {
                kind,
                inputs,
                dst,
                ty,
            },
            values,
            lowered,
        )?;
        Ok(())
    }

    fn inline_call(
        &mut self,
        pc: usize,
        dst: Register,
        callable: Register,
        args: &[Register],
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
    ) -> Result<(), String> {
        let callee = ops::constant_callee(self.executable, pc, callable)
            .or_else(|| {
                self.call_targets
                    .get(&callable)
                    .map(|target| &target.function)
            })
            .ok_or_else(|| "adaptive-v2 loop call target is not a function".to_owned())?;
        crate::verifier::verify(callee)?;
        if callee.parameters().len() != args.len() {
            return Err("adaptive-v2 loop callee arity is unsupported".to_owned());
        }
        let mut locals = if callee.parameters().len() == 1 && args.len() == 1 {
            if let Some(items) = self.virtual_tuples.get(&args[0]) {
                BTreeMap::from([(callee.parameters()[0].register, items[0].1)])
            } else {
                BTreeMap::from([(callee.parameters()[0].register, self.read(values, args[0])?)])
            }
        } else {
            callee
                .parameters()
                .iter()
                .zip(args)
                .map(|(parameter, argument)| {
                    self.read(values, *argument)
                        .map(|value| (parameter.register, value))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?
        };
        let mut constants = BTreeMap::<Register, i64>::new();
        let mut returned = None;
        for (callee_pc, instruction) in callee.bytecode().code.iter().enumerate() {
            match instruction {
                WvmInstruction::BinaryOp {
                    dst, op, lhs, rhs, ..
                } => {
                    let floor_divisor = constants
                        .get(rhs)
                        .copied()
                        .filter(|value| *value != 0 && *value != -1);
                    self.lower_binary(
                        pc,
                        *dst,
                        *op,
                        *lhs,
                        *rhs,
                        floor_divisor,
                        &mut locals,
                        lowered,
                    )?;
                    let computed = match (constants.get(lhs), constants.get(rhs), op) {
                        (Some(left), Some(right), crate::bytecode::BinaryOperator::Add) => {
                            left.checked_add(*right)
                        }
                        (Some(left), Some(right), crate::bytecode::BinaryOperator::Subtract) => {
                            left.checked_sub(*right)
                        }
                        (Some(left), Some(right), crate::bytecode::BinaryOperator::Multiply) => {
                            left.checked_mul(*right)
                        }
                        (Some(left), Some(right), crate::bytecode::BinaryOperator::FloorDivide) => {
                            left.checked_div_euclid(*right)
                        }
                        _ => None,
                    };
                    if let Some(value) = computed {
                        constants.insert(*dst, value);
                    } else {
                        constants.remove(dst);
                    }
                }
                WvmInstruction::Move { dst, src } => {
                    let operation =
                        ops::lower(callee, callee_pc, instruction, &locals, &BTreeMap::new())?
                            .ok_or_else(|| {
                                "adaptive-v2 loop callee move is unsupported".to_owned()
                            })?;
                    self.emit_operation(pc, operation, &mut locals, lowered)?;
                    if let Some(value) = constants.get(src).copied() {
                        constants.insert(*dst, value);
                    }
                }
                WvmInstruction::ConstSmallInt { dst, value }
                | WvmInstruction::ConstI64 { dst, value } => {
                    let operation =
                        ops::lower(callee, callee_pc, instruction, &locals, &BTreeMap::new())?
                            .ok_or_else(|| {
                                "adaptive-v2 loop callee constant is unsupported".to_owned()
                            })?;
                    self.emit_operation(pc, operation, &mut locals, lowered)?;
                    constants.insert(*dst, *value);
                }
                WvmInstruction::ConstFloat { .. } | WvmInstruction::ConstBool { .. } => {
                    let operation =
                        ops::lower(callee, callee_pc, instruction, &locals, &BTreeMap::new())?
                            .ok_or_else(|| {
                                "adaptive-v2 loop callee constant is unsupported".to_owned()
                            })?;
                    self.emit_operation(pc, operation, &mut locals, lowered)?;
                }
                WvmInstruction::Return { src } if callee_pc + 1 == callee.bytecode().code.len() => {
                    returned = Some(self.read(&locals, *src)?);
                }
                _ => {
                    return Err(format!(
                        "adaptive-v2 loop callee {:?} body pc {callee_pc} is unsupported: {instruction:?}",
                        callee.name()
                    ));
                }
            }
        }
        let returned =
            returned.ok_or_else(|| "adaptive-v2 loop callee has no terminal return".to_owned())?;
        self.emit_operation(
            pc,
            ops::LoweredOp {
                kind: InstructionKind::Copy,
                inputs: vec![returned.id],
                dst,
                ty: returned.ty,
            },
            values,
            lowered,
        )?;
        Ok(())
    }

    pub(super) fn add_backedge_safepoint(
        &mut self,
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
        backedge_pc: usize,
        resume_pc: usize,
    ) -> Result<SafepointId, String> {
        let register = self
            .region
            .entry_summary
            .first()
            .map(|slot| slot.register)
            .ok_or_else(|| "adaptive-v2 loop has no live header registers".to_owned())?;
        let source = self.read(values, register)?;
        let output = self.define(values, register, source.ty);
        let point = SafepointId::new(self.next_safepoint);
        self.next_safepoint = self.next_safepoint.saturating_add(1);
        let order = lowered
            .iter()
            .filter(|instruction| instruction.effect.is_ordered())
            .count() as u32;
        lowered.push(
            Instruction::safepoint(
                InstructionKind::Copy.at_pc(pc_u32(backedge_pc)?),
                vec![source.id],
                Some(output),
                Effect::Backedge,
                point,
            )
            .ordered(order),
        );
        let recipe = self.deopt(point, values, pc_u32(resume_pc)?, point.get())?;
        let mut roots: BTreeSet<RootLocation> = recipe
            .frames
            .iter()
            .flat_map(|frame| &frame.registers)
            .filter_map(|register| match register.source {
                RegisterSource::Ssa(value) if register.ty == ValueType::Handle => {
                    Some(RootLocation::Ssa(value))
                }
                RegisterSource::Spill {
                    slot,
                    ty: ValueType::Handle,
                } => Some(RootLocation::Spill(slot)),
                RegisterSource::Virtual(id) if register.ty == ValueType::Handle => {
                    Some(RootLocation::Virtual(id))
                }
                _ => None,
            })
            .collect();
        roots.extend(
            self.owned_lists
                .keys()
                .copied()
                .map(RootLocation::OwnedList),
        );
        self.root_maps.push(RootMap::new(point, roots));
        self.deopts.push(recipe);
        Ok(point)
    }

    pub(super) fn deopt(
        &self,
        point: SafepointId,
        values: &BTreeMap<Register, SsaValue>,
        resume_pc: u32,
        id: u32,
    ) -> Result<DeoptRecipe, String> {
        let mut dead = Vec::new();
        let mut live = self
            .region
            .entry_summary
            .iter()
            .map(|slot| slot.register)
            .collect::<BTreeSet<_>>();
        live.extend(self.storage_registers.iter().copied());
        let resume = usize::try_from(resume_pc)
            .map_err(|_| "adaptive-v2 deopt resume pc overflow".to_owned())?;
        live.extend(values.keys().copied().filter(|register| {
            replay_value_is_live(
                self.executable.bytecode().code.as_slice(),
                resume,
                *register,
            )
        }));
        let registers = (0..self.executable.bytecode().register_count)
            .map(|index| {
                let register = u16::try_from(index)
                    .map_err(|_| "adaptive-v2 loop register index overflow".to_owned())?;
                Ok(
                    match values
                        .get(&register)
                        .copied()
                        .filter(|_| live.contains(&register))
                    {
                        Some(value) => {
                            RegisterRecipe::new(register, RegisterSource::Ssa(value.id), value.ty)
                        }
                        None => {
                            dead.push(register);
                            RegisterRecipe::new(
                                register,
                                RegisterSource::UndefinedDead,
                                ValueType::I64,
                            )
                        }
                    },
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        let frame =
            FrameRecipe::new(self.identity.id, resume_pc, registers).with_dead_registers(dead);
        Ok(DeoptRecipe::new(
            id,
            self.identity,
            resume_pc,
            ResumeMode::ReplayBeforePc,
            vec![frame],
            point,
        )
        .with_dependencies(self.dependencies.to_vec()))
    }

    pub(super) fn header_arguments(
        &self,
        values: &BTreeMap<Register, SsaValue>,
    ) -> Result<Vec<ValueId>, String> {
        self.region
            .entry_summary
            .iter()
            .map(|slot| slot.register)
            .chain(self.storage_registers.iter().copied())
            .map(|register| self.read(values, register).map(|value| value.id))
            .collect()
    }

    pub(super) fn define(
        &mut self,
        values: &mut BTreeMap<Register, SsaValue>,
        register: Register,
        ty: ValueType,
    ) -> ValueDef {
        let value = SsaValue {
            id: ValueId::new(self.next_value),
            ty,
        };
        self.next_value = self.next_value.saturating_add(1);
        values.insert(register, value);
        ValueDef::new(value.id, value.ty)
    }

    pub(super) fn read(
        &self,
        values: &BTreeMap<Register, SsaValue>,
        register: Register,
    ) -> Result<SsaValue, String> {
        values
            .get(&register)
            .copied()
            .ok_or_else(|| format!("adaptive-v2 loop reads undefined r{register}"))
    }
}

fn indexed_element_type(
    types: &BTreeMap<Vec<usize>, ValueType>,
    path: &[Option<usize>],
) -> Option<ValueType> {
    let mut matches = types.iter().filter_map(|(candidate, ty)| {
        (candidate.len() == path.len()
            && candidate.iter().zip(path).all(|(candidate, expected)| {
                expected.is_none_or(|expected| *candidate == expected)
            }))
        .then_some(*ty)
    });
    let first = matches.next()?;
    matches.all(|ty| ty == first).then_some(first)
}

pub(super) fn pc_u32(pc: usize) -> Result<u32, String> {
    u32::try_from(pc).map_err(|_| "adaptive-v2 loop pc overflow".to_owned())
}

fn temporary_register(values: &BTreeMap<Register, SsaValue>) -> Result<Register, String> {
    values
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| "adaptive-v2 loop temporary register overflow".to_owned())
}

pub(super) fn replay_value_is_live(
    code: &[WvmInstruction],
    start: usize,
    register: Register,
) -> bool {
    for instruction in code.iter().skip(start) {
        if instruction_reads(instruction, register) {
            return true;
        }
        if instruction_writes(instruction, register) {
            return false;
        }
    }
    false
}

pub(super) fn replay_value_is_read_after(
    code: &[WvmInstruction],
    start: usize,
    register: Register,
) -> bool {
    code.iter()
        .skip(start)
        .any(|instruction| instruction_reads(instruction, register))
}

fn instruction_reads(instruction: &WvmInstruction, register: Register) -> bool {
    let contains = |registers: &[Register]| registers.contains(&register);
    match instruction {
        WvmInstruction::BinaryOp { lhs, rhs, .. }
        | WvmInstruction::CompareOp { lhs, rhs, .. }
        | WvmInstruction::BooleanOp { lhs, rhs, .. }
        | WvmInstruction::AddI64 { lhs, rhs, .. }
        | WvmInstruction::LtI64 { lhs, rhs, .. } => [*lhs, *rhs].contains(&register),
        WvmInstruction::UnaryOp { src, .. } | WvmInstruction::Move { src, .. } => *src == register,
        WvmInstruction::BuildTuple { items, .. } | WvmInstruction::BuildList { items, .. } => {
            contains(items)
        }
        WvmInstruction::BuildDict { entries, .. } => entries
            .iter()
            .any(|(key, value)| *key == register || *value == register),
        WvmInstruction::GetItem { object, key, .. } => [*object, *key].contains(&register),
        WvmInstruction::GetAttr { object, .. } => *object == register,
        WvmInstruction::GetSlice {
            object,
            start,
            stop,
            step,
            ..
        }
        | WvmInstruction::SetSlice {
            object,
            start,
            stop,
            step,
            ..
        } => {
            *object == register
                || [*start, *stop, *step]
                    .into_iter()
                    .flatten()
                    .any(|candidate| candidate == register)
                || matches!(instruction, WvmInstruction::SetSlice { value, .. } if *value == register)
        }
        WvmInstruction::SetItem {
            object, key, value, ..
        }
        | WvmInstruction::ListInsert {
            list: object,
            index: key,
            value,
        } => [*object, *key, *value].contains(&register),
        WvmInstruction::SetAttr { object, value, .. } => [*object, *value].contains(&register),
        WvmInstruction::ListAppend { list, value } => [*list, *value].contains(&register),
        WvmInstruction::ListPop { list, index, .. } => [*list, *index].contains(&register),
        WvmInstruction::Length { object, .. } => *object == register,
        WvmInstruction::Call { callable, args, .. } => *callable == register || contains(args),
        WvmInstruction::CallMethod { receiver, args, .. } => {
            *receiver == register || contains(args)
        }
        WvmInstruction::Branch { cond, .. } => *cond == register,
        WvmInstruction::Return { src } => *src == register,
        WvmInstruction::ConstSmallInt { .. }
        | WvmInstruction::ConstFloat { .. }
        | WvmInstruction::ConstBool { .. }
        | WvmInstruction::ConstNone { .. }
        | WvmInstruction::LoadConstant { .. }
        | WvmInstruction::ConstI64 { .. }
        | WvmInstruction::LoadCurrentFunction { .. }
        | WvmInstruction::Jump { .. } => false,
    }
}

fn instruction_writes(instruction: &WvmInstruction, register: Register) -> bool {
    match instruction {
        WvmInstruction::ConstSmallInt { dst, .. }
        | WvmInstruction::ConstFloat { dst, .. }
        | WvmInstruction::ConstBool { dst, .. }
        | WvmInstruction::ConstNone { dst }
        | WvmInstruction::LoadConstant { dst, .. }
        | WvmInstruction::ConstI64 { dst, .. }
        | WvmInstruction::BinaryOp { dst, .. }
        | WvmInstruction::CompareOp { dst, .. }
        | WvmInstruction::UnaryOp { dst, .. }
        | WvmInstruction::BooleanOp { dst, .. }
        | WvmInstruction::BuildTuple { dst, .. }
        | WvmInstruction::BuildList { dst, .. }
        | WvmInstruction::BuildDict { dst, .. }
        | WvmInstruction::GetItem { dst, .. }
        | WvmInstruction::GetAttr { dst, .. }
        | WvmInstruction::GetSlice { dst, .. }
        | WvmInstruction::ListPop { dst, .. }
        | WvmInstruction::Length { dst, .. }
        | WvmInstruction::LoadCurrentFunction { dst }
        | WvmInstruction::Call { dst, .. }
        | WvmInstruction::CallMethod { dst, .. }
        | WvmInstruction::AddI64 { dst, .. }
        | WvmInstruction::LtI64 { dst, .. }
        | WvmInstruction::Move { dst, .. } => *dst == register,
        WvmInstruction::SetItem { .. }
        | WvmInstruction::SetAttr { .. }
        | WvmInstruction::SetSlice { .. }
        | WvmInstruction::ListAppend { .. }
        | WvmInstruction::ListInsert { .. }
        | WvmInstruction::Jump { .. }
        | WvmInstruction::Branch { .. }
        | WvmInstruction::Return { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_v2::trace::ExecutableIdentity;
    use crate::adaptive_v2::wxir_v2::deopt::DeoptRecipe;
    use crate::bytecode::{Function, Instruction as WvmInstruction};
    use crate::executable::ExecutableFunction;
    use crate::structure_map::StructureMap;

    #[test]
    fn dead_phi_elision_preserves_a_native_unused_value_required_by_replay() {
        let executable = ExecutableFunction::new(
            Function {
                code: vec![WvmInstruction::Return { src: 0 }],
                register_count: 1,
            },
            StructureMap::default(),
        );
        let identity = ExecutableIdentity::new(executable.id().as_u64(), executable.id().as_u64());
        let value = ValueId::new(0);
        let frame = FrameRecipe::new(
            executable.id().as_u64(),
            0,
            vec![RegisterRecipe::new(
                0,
                RegisterSource::Ssa(value),
                ValueType::I64,
            )],
        );
        let mut lowered = super::super::LoweredLoop {
            blocks: vec![Block::new(
                BlockId::new(0),
                vec![ValueDef::new(value, ValueType::I64)],
                Vec::new(),
                Terminator::Return { values: Vec::new() },
            )],
            root_maps: Vec::new(),
            deopts: vec![DeoptRecipe::new(
                1,
                identity,
                0,
                ResumeMode::ReplayBeforePc,
                vec![frame],
                SafepointId::new(1),
            )],
        };

        elide_dead_numeric_phis(&mut lowered, &executable, &[]);

        assert_eq!(lowered.blocks[0].parameters.len(), 1);
        assert_eq!(
            lowered.deopts[0].frames[0].registers[0].source,
            RegisterSource::Ssa(value)
        );
        assert!(lowered.deopts[0].frames[0].dead_registers.is_empty());
    }
}

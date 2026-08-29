use std::collections::{BTreeMap, BTreeSet};

use crate::adaptive_v2::trace::ExecutableIdentity;
use crate::adaptive_v2::wxir_v2::deopt::DeoptRecipe;
use crate::adaptive_v2::wxir_v2::dependency::Dependency;
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId as WxBlockId, RootLocation, RootMap, SafepointId, Terminator, ValueDef, ValueId,
    ValueType,
};
use crate::bytecode::{Instruction as WvmInstruction, Register};
use crate::executable::ExecutableFunction;
use crate::structure_map::{Region, SlotType};
use crate::value::Value;

mod instruction;

pub(super) fn prepared_value_is_live(
    code: &[WvmInstruction],
    start: usize,
    register: Register,
) -> bool {
    instruction::replay_value_is_live(code, start, register)
}

const MAX_EMITTED_BLOCKS: u32 = 256;

#[derive(Clone, Copy)]
struct SsaValue {
    id: ValueId,
    ty: ValueType,
}

pub(super) struct LoweredLoop {
    pub(super) blocks: Vec<Block>,
    pub(super) root_maps: Vec<RootMap>,
    pub(super) deopts: Vec<DeoptRecipe>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn lower(
    executable: &ExecutableFunction,
    region_id: crate::structure_map::RegionId,
    region: &Region,
    backedge: usize,
    inputs: &[Value],
    prepared: Option<&super::PreparedLoop>,
    element_types: &BTreeMap<Register, ValueType>,
    call_targets: &BTreeMap<Register, super::CallTarget>,
    constant_call_targets: &super::ConstantCallTargets,
    identity: ExecutableIdentity,
    dependencies: &[Dependency],
) -> Result<LoweredLoop, String> {
    let storage_destinations = super::storage_live_destinations(executable, region_id);
    let mut input_types = region
        .entry_summary
        .iter()
        .zip(inputs)
        .map(|(slot, value)| value_type(slot.ty, *value))
        .collect::<Result<Vec<_>, _>>()?;
    input_types.extend(
        prepared
            .into_iter()
            .flat_map(|prepared| &prepared.values)
            .map(|(_, value)| observed_value_type(*value))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let storage_registers = prepared
        .map(|prepared| {
            prepared
                .values
                .iter()
                .map(|(register, _)| *register)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut lowered = lower_typed(
        executable,
        region,
        backedge,
        &input_types,
        element_types,
        call_targets,
        constant_call_targets,
        identity,
        dependencies,
        &storage_registers,
        &storage_destinations,
        prepared.map(|prepared| prepared.prefix),
    )?;
    instruction::elide_dead_numeric_phis(&mut lowered, executable, &storage_registers);
    instruction::widen_numeric_phis(&mut lowered, &input_types)?;
    Ok(lowered)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn lower_typed(
    executable: &ExecutableFunction,
    region: &Region,
    backedge: usize,
    input_types: &[ValueType],
    element_types: &BTreeMap<Register, ValueType>,
    call_targets: &BTreeMap<Register, super::CallTarget>,
    constant_call_targets: &super::ConstantCallTargets,
    identity: ExecutableIdentity,
    dependencies: &[Dependency],
    storage_registers: &[Register],
    storage_destinations: &[Register],
    prepared_prefix: Option<(usize, usize)>,
) -> Result<LoweredLoop, String> {
    let mut builder = Builder {
        executable,
        region,
        backedge,
        identity,
        dependencies,
        element_types,
        call_targets,
        constant_call_targets,
        region_blocks: region.blocks.iter().map(|block| block.0).collect(),
        exit_starts: region.exits.iter().map(|exit| exit.target).collect(),
        next_value: 0,
        next_block: 1,
        next_safepoint: 1,
        next_virtual: 1,
        blocks: Vec::new(),
        root_maps: Vec::new(),
        deopts: Vec::new(),
        virtual_tuples: BTreeMap::new(),
        secondary_loops: BTreeMap::new(),
        owned_lists: BTreeMap::new(),
        copied_list_element_types: BTreeMap::new(),
        storage_registers: storage_registers.to_vec(),
        storage_destinations: storage_destinations.to_vec(),
        prepared_prefix,
    };
    let mut values = BTreeMap::new();
    let mut parameters = region
        .entry_summary
        .iter()
        .zip(input_types)
        .map(|(slot, ty)| builder.define(&mut values, slot.register, *ty))
        .collect::<Vec<_>>();
    parameters.extend(
        storage_registers
            .iter()
            .zip(&input_types[region.entry_summary.len()..])
            .map(|(register, ty)| builder.define(&mut values, *register, *ty)),
    );
    builder.emit(
        WxBlockId::new(0),
        region.entry,
        values,
        parameters,
        false,
        &mut Vec::new(),
    )?;
    let owned_roots = builder
        .owned_lists
        .keys()
        .copied()
        .map(RootLocation::OwnedList)
        .collect::<Vec<_>>();
    for map in &mut builder.root_maps {
        map.roots.extend(owned_roots.iter().copied());
    }
    builder.blocks.sort_by_key(|block| block.id.get());
    Ok(LoweredLoop {
        blocks: builder.blocks,
        root_maps: builder.root_maps,
        deopts: builder.deopts,
    })
}

fn value_type(slot: SlotType, value: Value) -> Result<ValueType, String> {
    match (slot, value) {
        (SlotType::SmallInt | SlotType::Any, Value::SmallInt(_)) => Ok(ValueType::I64),
        (SlotType::Float | SlotType::Any, Value::Float(_)) => Ok(ValueType::F64),
        (SlotType::Bool | SlotType::Any, Value::Bool(_)) => Ok(ValueType::Bool),
        (SlotType::Object(_) | SlotType::Any, Value::Object(_)) => Ok(ValueType::Handle),
        _ => Err("adaptive-v2 loop header value does not match its observed type".to_owned()),
    }
}

fn observed_value_type(value: Value) -> Result<ValueType, String> {
    match value {
        Value::SmallInt(_) => Ok(ValueType::I64),
        Value::Float(_) => Ok(ValueType::F64),
        Value::Bool(_) => Ok(ValueType::Bool),
        Value::Object(_) => Ok(ValueType::Handle),
        Value::None | Value::Uninitialized => {
            Err("adaptive-v2 prepared loop value is not typed".to_owned())
        }
    }
}

struct Builder<'a> {
    executable: &'a ExecutableFunction,
    region: &'a Region,
    backedge: usize,
    identity: ExecutableIdentity,
    dependencies: &'a [Dependency],
    element_types: &'a BTreeMap<Register, ValueType>,
    call_targets: &'a BTreeMap<Register, super::CallTarget>,
    constant_call_targets: &'a super::ConstantCallTargets,
    region_blocks: BTreeSet<u32>,
    exit_starts: BTreeSet<usize>,
    next_value: u32,
    next_block: u32,
    next_safepoint: u32,
    next_virtual: u32,
    blocks: Vec<Block>,
    root_maps: Vec<RootMap>,
    deopts: Vec<DeoptRecipe>,
    virtual_tuples: BTreeMap<Register, Vec<(Register, SsaValue)>>,
    secondary_loops: BTreeMap<usize, (WxBlockId, Vec<Register>)>,
    owned_lists: BTreeMap<u32, SsaValue>,
    copied_list_element_types: BTreeMap<ValueId, ValueType>,
    storage_registers: Vec<Register>,
    storage_destinations: Vec<Register>,
    prepared_prefix: Option<(usize, usize)>,
}

impl Builder<'_> {
    fn emit(
        &mut self,
        id: WxBlockId,
        start: usize,
        mut values: BTreeMap<Register, SsaValue>,
        parameters: Vec<ValueDef>,
        exit_path: bool,
        active: &mut Vec<usize>,
    ) -> Result<(), String> {
        if active.contains(&start) {
            return Err("adaptive-v2 loop contains a secondary cycle".to_owned());
        }
        active.push(start);
        let block = self
            .executable
            .structure_map()
            .block_by_pc(start)
            .filter(|block| block.start_pc == start)
            .ok_or_else(|| format!("adaptive-v2 loop target {start} is not a block entry"))?;
        let internal = self.region_blocks.contains(&block.id.0);
        if !internal && !exit_path && !self.exit_starts.contains(&start) {
            return Err(format!(
                "adaptive-v2 loop escapes through unrecorded pc {start}"
            ));
        }
        let mut instructions = Vec::new();
        if start == self.region.entry {
            for destination in self.storage_destinations.clone() {
                if values.contains_key(&destination) {
                    continue;
                }
                let source = self
                    .region
                    .entry_summary
                    .iter()
                    .find_map(|slot| {
                        values
                            .get(&slot.register)
                            .copied()
                            .filter(|value| value.ty == ValueType::Handle)
                    })
                    .ok_or_else(|| "adaptive-v2 owned destination has no source list".to_owned())?;
                let capacity = self.define(&mut values, destination, ValueType::I64);
                instructions.push(crate::adaptive_v2::wxir_v2::ir::Instruction::new(
                    crate::adaptive_v2::wxir_v2::ir::InstructionKind::ListLength,
                    vec![source.id],
                    Some(capacity),
                    crate::adaptive_v2::wxir_v2::ir::Effect::Read,
                ));
                let owned = self.define(&mut values, destination, ValueType::Handle);
                instructions.push(crate::adaptive_v2::wxir_v2::ir::Instruction::new(
                    crate::adaptive_v2::wxir_v2::ir::InstructionKind::OwnedList {
                        identity: 2,
                        element_type: ValueType::F64,
                        reset_on_definition: false,
                        copy_from_source: false,
                    },
                    vec![capacity.id],
                    Some(owned),
                    crate::adaptive_v2::wxir_v2::ir::Effect::Pure,
                ));
                self.owned_lists.insert(
                    2,
                    SsaValue {
                        id: owned.id,
                        ty: owned.ty,
                    },
                );
            }
        }
        let code = &self.executable.bytecode().code;
        let terminal_pc = block.end_pc.saturating_sub(1);
        let lowered_start = self
            .prepared_prefix
            .filter(|(prefix_start, _)| *prefix_start == start)
            .map_or(start, |(_, prefix_end)| prefix_end);
        let mut pc = lowered_start;
        while pc < terminal_pc {
            let instruction = &code[pc];
            if let Some(terminator) = self.try_splice_nested_loop_call(
                pc,
                instruction,
                terminal_pc,
                &mut values,
                &mut instructions,
                active,
            )? {
                self.blocks
                    .push(Block::new(id, parameters, instructions, terminator));
                active.pop();
                return Ok(());
            }
            if pc.saturating_add(1) < terminal_pc
                && self.try_lower_reverse_prefix(pc, &mut values, &mut instructions)?
            {
                pc = pc.saturating_add(2);
                continue;
            }
            self.lower_instruction(pc, instruction, &mut values, &mut instructions)?;
            pc = pc.saturating_add(1);
        }
        let terminal = code
            .get(terminal_pc)
            .ok_or_else(|| "adaptive-v2 loop block is empty".to_owned())?;
        let terminator = self.lower_terminator(
            terminal_pc,
            terminal,
            &mut values,
            &mut instructions,
            internal,
            exit_path,
            active,
        )?;
        self.blocks
            .push(Block::new(id, parameters, instructions, terminator));
        active.pop();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_terminator(
        &mut self,
        terminal_pc: usize,
        terminal: &WvmInstruction,
        values: &mut BTreeMap<Register, SsaValue>,
        instructions: &mut Vec<crate::adaptive_v2::wxir_v2::ir::Instruction>,
        internal: bool,
        exit_path: bool,
        active: &mut Vec<usize>,
    ) -> Result<Terminator, String> {
        Ok(match terminal {
            WvmInstruction::Return { src } => Terminator::Return {
                values: vec![self.read(values, *src)?.id],
            },
            WvmInstruction::Jump { target } if *target == self.region.entry => {
                if terminal_pc != self.backedge || !internal {
                    return Err("adaptive-v2 loop has an unrecognized backedge".to_owned());
                }
                let point = self.add_backedge_safepoint(
                    values,
                    instructions,
                    terminal_pc,
                    self.region.entry,
                )?;
                if self.prepared_prefix.is_some() {
                    Terminator::SideExit {
                        id: point.get(),
                        values: self
                            .region
                            .entry_summary
                            .iter()
                            .map(|slot| self.read(values, slot.register).map(|value| value.id))
                            .collect::<Result<Vec<_>, _>>()?,
                    }
                } else {
                    Terminator::Jump {
                        target: WxBlockId::new(0),
                        arguments: self.header_arguments(values)?,
                    }
                }
            }
            WvmInstruction::Jump { target } => {
                if let Some((header, registers)) = self.secondary_loops.get(target).cloned() {
                    self.add_backedge_safepoint(values, instructions, terminal_pc, *target)?;
                    let arguments = registers
                        .iter()
                        .map(|register| self.read(values, *register).map(|value| value.id))
                        .collect::<Result<Vec<_>, _>>()?;
                    Terminator::Jump {
                        target: header,
                        arguments,
                    }
                } else {
                    let child = self.child(*target, values.clone(), exit_path, active)?;
                    let arguments = self
                        .secondary_loops
                        .get(target)
                        .map(|(_, registers)| {
                            registers
                                .iter()
                                .map(|register| self.read(values, *register).map(|value| value.id))
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .transpose()?
                        .unwrap_or_default();
                    Terminator::Jump {
                        target: child,
                        arguments,
                    }
                }
            }
            WvmInstruction::Branch { cond, yes, no } => {
                let condition = self.read(values, *cond)?;
                if condition.ty != ValueType::Bool {
                    return Err("adaptive-v2 loop branch condition is not boolean".to_owned());
                }
                let yes = self.branch_child(
                    terminal_pc,
                    *yes,
                    values,
                    instructions,
                    internal,
                    exit_path,
                    active,
                )?;
                let no = self.branch_child(
                    terminal_pc,
                    *no,
                    values,
                    instructions,
                    internal,
                    exit_path,
                    active,
                )?;
                Terminator::Branch {
                    condition: condition.id,
                    yes,
                    no,
                }
            }
            other => {
                self.lower_instruction(terminal_pc, other, values, instructions)?;
                let target_pc = terminal_pc + 1;
                let target = self.child(target_pc, values.clone(), exit_path, active)?;
                let arguments = self
                    .secondary_loops
                    .get(&target_pc)
                    .map(|(_, registers)| {
                        registers
                            .iter()
                            .map(|register| self.read(values, *register).map(|value| value.id))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                Terminator::Jump { target, arguments }
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn branch_child(
        &mut self,
        terminal_pc: usize,
        target: usize,
        values: &mut BTreeMap<Register, SsaValue>,
        instructions: &mut Vec<crate::adaptive_v2::wxir_v2::ir::Instruction>,
        internal: bool,
        exit_path: bool,
        active: &mut Vec<usize>,
    ) -> Result<WxBlockId, String> {
        if target == self.region.entry {
            if terminal_pc != self.backedge || !internal {
                return Err("adaptive-v2 loop has an unrecognized backedge".to_owned());
            }
            let point =
                self.add_backedge_safepoint(values, instructions, terminal_pc, self.region.entry)?;
            let id = self.fresh_block()?;
            let terminator = if self.prepared_prefix.is_some() {
                Terminator::SideExit {
                    id: point.get(),
                    values: self
                        .region
                        .entry_summary
                        .iter()
                        .map(|slot| self.read(values, slot.register).map(|value| value.id))
                        .collect::<Result<Vec<_>, _>>()?,
                }
            } else {
                Terminator::Jump {
                    target: WxBlockId::new(0),
                    arguments: self.header_arguments(values)?,
                }
            };
            self.blocks
                .push(Block::new(id, Vec::new(), Vec::new(), terminator));
            return Ok(id);
        }

        let child = if let Some((header, _)) = self.secondary_loops.get(&target) {
            *header
        } else {
            self.child(target, values.clone(), exit_path, active)?
        };
        let Some((header, registers)) = self.secondary_loops.get(&target).cloned() else {
            return Ok(child);
        };
        self.add_backedge_safepoint(values, instructions, terminal_pc, target)?;
        let arguments = registers
            .iter()
            .map(|register| self.read(values, *register).map(|value| value.id))
            .collect::<Result<Vec<_>, _>>()?;
        let id = self.fresh_block()?;
        self.blocks.push(Block::new(
            id,
            Vec::new(),
            Vec::new(),
            Terminator::Jump {
                target: header,
                arguments,
            },
        ));
        Ok(id)
    }

    fn fresh_block(&mut self) -> Result<WxBlockId, String> {
        if self.next_block >= MAX_EMITTED_BLOCKS {
            return Err("adaptive-v2 loop CFG exceeds the conservative block limit".to_owned());
        }
        let id = WxBlockId::new(self.next_block);
        self.next_block = self.next_block.saturating_add(1);
        Ok(id)
    }

    fn child(
        &mut self,
        target: usize,
        values: BTreeMap<Register, SsaValue>,
        exit_path: bool,
        active: &mut Vec<usize>,
    ) -> Result<WxBlockId, String> {
        let id = self.fresh_block()?;
        if self.exit_starts.contains(&target)
            && !self
                .executable
                .structure_map()
                .loop_regions()
                .any(|(_, region)| region.entry > target)
        {
            let point = SafepointId::new(self.next_safepoint);
            self.next_safepoint = self.next_safepoint.saturating_add(1);
            let roots = values
                .values()
                .filter(|value| value.ty == ValueType::Handle)
                .map(|value| RootLocation::Ssa(value.id))
                .collect();
            self.root_maps.push(RootMap::new(point, roots));
            let target_pc = instruction::pc_u32(target)?;
            self.deopts
                .push(self.deopt(point, &values, target_pc, point.get())?);
            let outputs = self
                .region
                .entry_summary
                .iter()
                .map(|slot| self.read(&values, slot.register).map(|value| value.id))
                .collect::<Result<Vec<_>, _>>()?;
            self.blocks.push(Block::new(
                id,
                Vec::new(),
                Vec::new(),
                Terminator::SideExit {
                    id: point.get(),
                    values: outputs,
                },
            ));
            return Ok(id);
        }
        if let Some((_, following)) =
            self.executable
                .structure_map()
                .loop_regions()
                .find(|(_, candidate)| {
                    candidate.entry == target && candidate.entry != self.region.entry
                })
        {
            let mut header_values = values;
            let mut parameters = Vec::new();
            let mut registers = Vec::new();
            for slot in &following.entry_summary {
                if !instruction::replay_value_is_read_after(
                    &self.executable.bytecode().code,
                    target,
                    slot.register,
                ) || !header_values.contains_key(&slot.register)
                {
                    continue;
                }
                let source = self.read(&header_values, slot.register)?;
                let parameter = self.define(&mut header_values, slot.register, source.ty);
                if let Some(element_type) = self.copied_list_element_types.get(&source.id).copied()
                {
                    self.copied_list_element_types
                        .insert(parameter.id, element_type);
                }
                parameters.push(parameter);
                registers.push(slot.register);
            }
            self.secondary_loops.insert(target, (id, registers));
            self.emit(id, target, header_values, parameters, true, active)?;
            return Ok(id);
        }
        let next_exit = exit_path || self.exit_starts.contains(&target);
        self.emit(id, target, values, Vec::new(), next_exit, active)?;
        Ok(id)
    }
}

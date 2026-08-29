use std::collections::{BTreeMap, BTreeSet};

use super::FusedTraceRequest;
use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity};
use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, NumericComparison,
    SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
};
use crate::bytecode::{
    BinaryOperator, BooleanOperator, CompareOperator, Instruction as WvmInstruction, Register,
    UnaryOperator,
};
use crate::executable::{ExecutableConstant, ExecutableFunction};
use crate::object::ObjectKind;
use crate::structure_map::{EscapeState, Fact, SlotType, ValueOrigin};

pub(super) fn recognizes(executable: &ExecutableFunction) -> bool {
    executable.parameters().is_empty()
        && executable
            .bytecode()
            .code
            .iter()
            .filter(|instruction| matches!(instruction, WvmInstruction::Jump { .. }))
            .count()
            > 1
        && infer_types(executable).is_some()
}

pub(super) fn lower(request: &FusedTraceRequest<'_>) -> Result<Option<SnapshotDraft>, String> {
    if !request.arguments.is_empty() || !recognizes(request.executable) {
        return Ok(None);
    }
    let executable = request.executable;
    let types = infer_types(executable).ok_or_else(|| "unsupported scalar CFG".to_owned())?;
    let registers = types
        .iter()
        .enumerate()
        .filter_map(|(register, ty)| ty.map(|ty| (register as Register, ty)))
        .collect::<Vec<_>>();
    let leaders = leaders(executable.bytecode().code.as_slice())?;
    let rotations = rotation_loops(executable.bytecode().code.as_slice(), &leaders);
    let rotation_bodies = rotations
        .values()
        .map(|rotation| rotation.body)
        .collect::<BTreeSet<_>>();
    let leader_ids = leaders
        .iter()
        .enumerate()
        .map(|(id, pc)| Ok((*pc, block_id(id)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let edge_base = leaders.len();
    let mut next_value = 0_u32;
    let mut parameters = Vec::with_capacity(leaders.len());
    for index in 0..leaders.len() {
        let mut block_parameters = Vec::new();
        if index != 0 {
            for (_, ty) in &registers {
                block_parameters.push(next(&mut next_value, *ty)?);
            }
        }
        parameters.push(block_parameters);
    }
    let mut blocks = Vec::new();
    let mut edge_blocks = Vec::new();
    for (index, start) in leaders.iter().copied().enumerate() {
        if rotation_bodies.contains(&start) {
            continue;
        }
        let end = leaders
            .get(index + 1)
            .copied()
            .unwrap_or(executable.bytecode().code.len());
        let mut values = BTreeMap::new();
        let mut instructions = Vec::new();
        if index == 0 {
            for (register, ty) in &registers {
                if *ty == ValueType::Handle {
                    continue;
                }
                let value = next(&mut next_value, *ty)?;
                let dead = match ty {
                    ValueType::I64 => Constant::Integer(0),
                    ValueType::Bool => Constant::Boolean(false),
                    ValueType::F64 | ValueType::Handle | ValueType::BorrowedView => {
                        return Err("unsupported scalar CFG register type".to_owned());
                    }
                };
                instructions.push(Instruction::new(
                    InstructionKind::Constant(dead),
                    Vec::new(),
                    Some(value),
                    Effect::Pure,
                ));
                values.insert(*register, value);
            }
        } else {
            for ((register, _), value) in registers.iter().zip(&parameters[index]) {
                values.insert(*register, *value);
            }
        }
        let mut loaded_functions = BTreeMap::new();
        for pc in start..end.saturating_sub(1) {
            lower_instruction(
                executable,
                &executable.bytecode().code[pc],
                pc,
                &mut values,
                &mut loaded_functions,
                &mut instructions,
                &mut next_value,
            )?;
        }
        let last_pc = end.saturating_sub(1);
        let last = executable
            .bytecode()
            .code
            .get(last_pc)
            .ok_or_else(|| "empty scalar CFG block".to_owned())?;
        let terminator = if let Some(rotation) = rotations.get(&start) {
            lower_rotation(rotation, &mut values, &mut instructions, &mut next_value)?;
            Terminator::Jump {
                target: target_id(&leader_ids, rotation.exit)?,
                arguments: arguments(&registers, &values)?,
            }
        } else {
            match last {
                WvmInstruction::Jump { target } => Terminator::Jump {
                    target: target_id(&leader_ids, *target)?,
                    arguments: arguments(&registers, &values)?,
                },
                WvmInstruction::Branch { cond, yes, no } => {
                    let condition = value(&values, *cond, ValueType::Bool)?;
                    let edge_index = edge_blocks.len();
                    let yes_edge = block_id(edge_base + edge_index)?;
                    let no_edge = block_id(edge_base + edge_index + 1)?;
                    let incoming = arguments(&registers, &values)?;
                    edge_blocks.push(Block::new(
                        yes_edge,
                        Vec::new(),
                        Vec::new(),
                        Terminator::Jump {
                            target: target_id(&leader_ids, *yes)?,
                            arguments: incoming.clone(),
                        },
                    ));
                    edge_blocks.push(Block::new(
                        no_edge,
                        Vec::new(),
                        Vec::new(),
                        Terminator::Jump {
                            target: target_id(&leader_ids, *no)?,
                            arguments: incoming,
                        },
                    ));
                    Terminator::Branch {
                        condition: condition.id,
                        yes: yes_edge,
                        no: no_edge,
                    }
                }
                WvmInstruction::Return { src } => Terminator::Return {
                    values: vec![
                        values
                            .get(src)
                            .ok_or_else(|| format!("missing scalar return r{src}"))?
                            .id,
                    ],
                },
                instruction => {
                    lower_instruction(
                        executable,
                        instruction,
                        last_pc,
                        &mut values,
                        &mut loaded_functions,
                        &mut instructions,
                        &mut next_value,
                    )?;
                    Terminator::Jump {
                        target: target_id(&leader_ids, end)?,
                        arguments: arguments(&registers, &values)?,
                    }
                }
            }
        };
        blocks.push(Block::new(
            block_id(index)?,
            parameters[index].clone(),
            instructions,
            terminator,
        ));
    }
    blocks.extend(edge_blocks);
    let id = executable.id().as_u64();
    let mut dependencies = vec![
        Dependency::current(DependencyKind::Executable, id, id),
        Dependency::current(DependencyKind::Schema, id, request.facts.schema_epoch),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
    ];
    if registers.iter().any(|(_, ty)| *ty == ValueType::Handle) {
        dependencies.push(Dependency::current(DependencyKind::ListLayout, id, id));
    }
    for constant in executable.constants() {
        if let ExecutableConstant::Function(callee) = constant {
            dependencies.push(Dependency::current(
                DependencyKind::Callee,
                callee.id().as_u64(),
                callee.id().as_u64(),
            ));
        }
    }
    Ok(Some(
        SnapshotDraft::new(
            ExecutableIdentity::new(id, id),
            EntryKind::FunctionEntry,
            BlockId::new(0),
            blocks,
            Vec::new(),
            Vec::new(),
            dependencies,
        )
        .with_schema_epoch(request.permit.schema_epoch()),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RotationLoop {
    header: usize,
    body: usize,
    exit: usize,
    condition: Register,
    list: Register,
    index: Register,
    bound: Register,
}

fn rotation_loops(code: &[WvmInstruction], leaders: &[usize]) -> BTreeMap<usize, RotationLoop> {
    leaders
        .windows(3)
        .filter_map(|window| match_rotation_loop(code, window[0], window[1], window[2]))
        .map(|rotation| (rotation.header, rotation))
        .collect()
}

fn match_rotation_loop(
    code: &[WvmInstruction],
    header: usize,
    body: usize,
    exit: usize,
) -> Option<RotationLoop> {
    let [.., compare, branch] = code.get(header..body)? else {
        return None;
    };
    let WvmInstruction::CompareOp {
        dst: condition,
        op: CompareOperator::Lt,
        lhs: index,
        rhs: bound,
        ..
    } = compare
    else {
        return None;
    };
    if !matches!(branch, WvmInstruction::Branch { cond, yes, no } if cond == condition && *yes == body && *no == exit)
        || incoming_target_count(code, body) != 1
    {
        return None;
    }
    let [
        zero_instruction,
        negative_instruction,
        pop_instruction,
        insert_instruction,
        one_instruction,
        add_instruction,
        move_instruction,
        jump_instruction,
    ] = code.get(body..exit)?
    else {
        return None;
    };
    let (WvmInstruction::ConstSmallInt {
        dst: zero,
        value: 0,
    }
    | WvmInstruction::ConstI64 {
        dst: zero,
        value: 0,
    }) = zero_instruction
    else {
        return None;
    };
    let (WvmInstruction::ConstSmallInt {
        dst: negative,
        value: -1,
    }
    | WvmInstruction::ConstI64 {
        dst: negative,
        value: -1,
    }) = negative_instruction
    else {
        return None;
    };
    let WvmInstruction::ListPop {
        dst: popped,
        list,
        index: pop_index,
    } = pop_instruction
    else {
        return None;
    };
    let WvmInstruction::ListInsert {
        list: inserted_list,
        index: insert_index,
        value: inserted,
    } = insert_instruction
    else {
        return None;
    };
    let (WvmInstruction::ConstSmallInt { dst: one, value: 1 }
    | WvmInstruction::ConstI64 { dst: one, value: 1 }) = one_instruction
    else {
        return None;
    };
    let WvmInstruction::BinaryOp {
        dst: next_index,
        op: BinaryOperator::Add,
        lhs: add_left,
        rhs: add_right,
        ..
    } = add_instruction
    else {
        return None;
    };
    if pop_index != negative
        || inserted_list != list
        || insert_index != zero
        || inserted != popped
        || !((*add_left == *index && add_right == one) || (add_left == one && *add_right == *index))
        || !matches!(move_instruction, WvmInstruction::Move { dst, src } if dst == index && src == next_index)
        || !matches!(jump_instruction, WvmInstruction::Jump { target } if *target == header)
    {
        return None;
    }
    let distinct = [
        *condition,
        *list,
        *index,
        *bound,
        *zero,
        *negative,
        *popped,
        *one,
        *next_index,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if distinct.len() != 9
        || [*zero, *negative, *popped, *one, *next_index]
            .into_iter()
            .any(|register| !crate::wvm::temporary_is_dead(code, exit, register))
    {
        return None;
    }
    Some(RotationLoop {
        header,
        body,
        exit,
        condition: *condition,
        list: *list,
        index: *index,
        bound: *bound,
    })
}

fn incoming_target_count(code: &[WvmInstruction], target: usize) -> usize {
    code.iter()
        .map(|instruction| match instruction {
            WvmInstruction::Jump { target: candidate } => usize::from(*candidate == target),
            WvmInstruction::Branch { yes, no, .. } => {
                usize::from(*yes == target) + usize::from(*no == target)
            }
            _ => 0,
        })
        .sum()
}

fn lower_rotation(
    rotation: &RotationLoop,
    values: &mut BTreeMap<Register, ValueDef>,
    instructions: &mut Vec<Instruction>,
    next_value: &mut u32,
) -> Result<(), String> {
    let pc = u32::try_from(rotation.header).map_err(|_| "rotation pc overflow".to_owned())?;
    let list = value(values, rotation.list, ValueType::Handle)?;
    let index = value(values, rotation.index, ValueType::I64)?;
    let bound = value(values, rotation.bound, ValueType::I64)?;
    let length = emit_effect(
        next_value,
        instructions,
        InstructionKind::ListLength,
        vec![list.id],
        ValueType::I64,
        Effect::Read,
        pc,
    )?;
    let remaining = emit(
        next_value,
        instructions,
        InstructionKind::IntegerSubtract,
        vec![bound.id, index.id],
        ValueType::I64,
        pc,
    )?;
    let split = emit(
        next_value,
        instructions,
        InstructionKind::IntegerSubtract,
        vec![length.id, remaining.id],
        ValueType::I64,
        pc,
    )?;
    for end in [split, length, remaining] {
        let order = instructions
            .iter()
            .filter(|instruction| instruction.effect.is_ordered())
            .count() as u32;
        instructions.push(
            Instruction::new(
                InstructionKind::ListReversePrefix {
                    element_type: ValueType::I64,
                }
                .at_pc(pc),
                vec![list.id, end.id],
                None,
                Effect::Write,
            )
            .ordered(order),
        );
    }
    let exited = emit(
        next_value,
        instructions,
        InstructionKind::Constant(Constant::Boolean(false)),
        Vec::new(),
        ValueType::Bool,
        pc,
    )?;
    values.insert(rotation.index, bound);
    values.insert(rotation.condition, exited);
    Ok(())
}

fn leaders(code: &[WvmInstruction]) -> Result<Vec<usize>, String> {
    let mut result = BTreeSet::from([0]);
    for (pc, instruction) in code.iter().enumerate() {
        match instruction {
            WvmInstruction::Jump { target } => {
                result.insert(*target);
                if pc + 1 < code.len() {
                    result.insert(pc + 1);
                }
            }
            WvmInstruction::Branch { yes, no, .. } => {
                result.insert(*yes);
                result.insert(*no);
                if pc + 1 < code.len() {
                    result.insert(pc + 1);
                }
            }
            WvmInstruction::Return { .. } if pc + 1 < code.len() => {
                result.insert(pc + 1);
            }
            _ => {}
        }
    }
    if result.iter().any(|leader| *leader >= code.len()) {
        return Err("scalar CFG target outside bytecode".to_owned());
    }
    Ok(result.into_iter().collect())
}

fn infer_types(executable: &ExecutableFunction) -> Option<Vec<Option<ValueType>>> {
    let mut types = vec![None; executable.bytecode().register_count];
    for (pc, instruction) in executable.bytecode().code.iter().enumerate() {
        let definition = match instruction {
            WvmInstruction::ConstSmallInt { dst, .. }
            | WvmInstruction::ConstI64 { dst, .. }
            | WvmInstruction::BinaryOp { dst, .. }
            | WvmInstruction::GetItem { dst, .. }
            | WvmInstruction::ListPop { dst, .. }
            | WvmInstruction::Length { dst, .. }
            | WvmInstruction::Call { dst, .. } => Some((*dst, ValueType::I64)),
            WvmInstruction::BuildList { dst, items }
                if items.is_empty() && owned_local_list_at(executable, *dst, pc) =>
            {
                Some((*dst, ValueType::Handle))
            }
            WvmInstruction::ConstBool { dst, .. }
            | WvmInstruction::CompareOp { dst, .. }
            | WvmInstruction::BooleanOp { dst, .. }
            | WvmInstruction::UnaryOp {
                dst,
                op: UnaryOperator::Not,
                ..
            } => Some((*dst, ValueType::Bool)),
            _ => None,
        };
        if let Some((register, ty)) = definition {
            types[register as usize] = Some(ty);
        }
    }
    for _ in 0..types.len() {
        let mut changed = false;
        for instruction in &executable.bytecode().code {
            if let WvmInstruction::Move { dst, src } = instruction
                && types[*dst as usize].is_none()
                && types[*src as usize].is_some()
            {
                types[*dst as usize] = types[*src as usize];
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    executable
        .bytecode()
        .code
        .iter()
        .all(supported)
        .then(|| list_types_are_closed(executable, &types))?
        .then_some(types)
}

fn owned_local_list_at(executable: &ExecutableFunction, register: Register, pc: usize) -> bool {
    executable
        .structure_map()
        .instruction_fact(pc)
        .and_then(|instruction| instruction.output)
        .and_then(|value| executable.structure_map().value(value))
        .is_some_and(|value| {
            value.register == register
                && value.origin
                    == Fact::Proven(ValueOrigin::Allocation {
                        pc,
                        kind: ObjectKind::List,
                    })
                && value.ty == Fact::Proven(SlotType::Object(ObjectKind::List))
                && value.escape == Fact::Proven(EscapeState::Local)
        })
}

fn list_types_are_closed(executable: &ExecutableFunction, types: &[Option<ValueType>]) -> bool {
    let ty = |register: Register| types.get(register as usize).copied().flatten();
    executable
        .bytecode()
        .code
        .iter()
        .enumerate()
        .all(|(pc, instruction)| match instruction {
            WvmInstruction::BuildList { dst, items } => {
                items.is_empty() && owned_local_list_at(executable, *dst, pc)
            }
            WvmInstruction::GetItem { object, key, .. } => {
                ty(*object) == Some(ValueType::Handle) && ty(*key) == Some(ValueType::I64)
            }
            WvmInstruction::ListAppend { list, value } => {
                ty(*list) == Some(ValueType::Handle) && ty(*value) == Some(ValueType::I64)
            }
            WvmInstruction::ListInsert { list, index, value } => {
                ty(*list) == Some(ValueType::Handle)
                    && ty(*index) == Some(ValueType::I64)
                    && ty(*value) == Some(ValueType::I64)
            }
            WvmInstruction::ListPop { list, index, .. } => {
                ty(*list) == Some(ValueType::Handle) && ty(*index) == Some(ValueType::I64)
            }
            WvmInstruction::Length { object, .. } => ty(*object) == Some(ValueType::Handle),
            _ => true,
        })
}

fn supported(instruction: &WvmInstruction) -> bool {
    if let WvmInstruction::BuildList { items, .. } = instruction {
        return items.is_empty();
    }
    matches!(
        instruction,
        WvmInstruction::ConstSmallInt { .. }
            | WvmInstruction::ConstI64 { .. }
            | WvmInstruction::ConstBool { .. }
            | WvmInstruction::Move { .. }
            | WvmInstruction::LoadConstant { .. }
            | WvmInstruction::BinaryOp {
                op: BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply,
                ..
            }
            | WvmInstruction::CompareOp { .. }
            | WvmInstruction::UnaryOp {
                op: UnaryOperator::Not,
                ..
            }
            | WvmInstruction::BooleanOp { .. }
            | WvmInstruction::GetItem { .. }
            | WvmInstruction::ListAppend { .. }
            | WvmInstruction::ListInsert { .. }
            | WvmInstruction::ListPop { .. }
            | WvmInstruction::Length { .. }
            | WvmInstruction::Call { .. }
            | WvmInstruction::Jump { .. }
            | WvmInstruction::Branch { .. }
            | WvmInstruction::Return { .. }
    )
}

fn lower_instruction(
    executable: &ExecutableFunction,
    instruction: &WvmInstruction,
    pc: usize,
    values: &mut BTreeMap<Register, ValueDef>,
    loaded: &mut BTreeMap<Register, usize>,
    instructions: &mut Vec<Instruction>,
    next_value: &mut u32,
) -> Result<(), String> {
    let pc = u32::try_from(pc).map_err(|_| "scalar CFG pc overflow".to_owned())?;
    let (dst, kind, inputs, ty, effect) = match instruction {
        WvmInstruction::ConstSmallInt { dst, value } | WvmInstruction::ConstI64 { dst, value } => (
            *dst,
            InstructionKind::Constant(Constant::Integer(*value)),
            Vec::new(),
            ValueType::I64,
            Effect::Pure,
        ),
        WvmInstruction::ConstBool { dst, value } => (
            *dst,
            InstructionKind::Constant(Constant::Boolean(*value)),
            Vec::new(),
            ValueType::Bool,
            Effect::Pure,
        ),
        WvmInstruction::Move { dst, src } => {
            values.insert(
                *dst,
                *values
                    .get(src)
                    .ok_or_else(|| format!("missing scalar r{src}"))?,
            );
            return Ok(());
        }
        WvmInstruction::LoadConstant { dst, constant } => {
            if !matches!(
                executable.constants().get(constant.0),
                Some(ExecutableConstant::Function(_))
            ) {
                return Err("scalar CFG constant is not a function".to_owned());
            }
            loaded.insert(*dst, constant.0);
            return Ok(());
        }
        WvmInstruction::BinaryOp {
            dst, op, lhs, rhs, ..
        } => (
            *dst,
            match op {
                BinaryOperator::Add => InstructionKind::IntegerAdd,
                BinaryOperator::Subtract => InstructionKind::IntegerSubtract,
                BinaryOperator::Multiply => InstructionKind::IntegerMultiply,
                _ => return Err("unsupported scalar arithmetic".to_owned()),
            },
            vec![
                value(values, *lhs, ValueType::I64)?.id,
                value(values, *rhs, ValueType::I64)?.id,
            ],
            ValueType::I64,
            Effect::Pure,
        ),
        WvmInstruction::CompareOp {
            dst, op, lhs, rhs, ..
        } => (
            *dst,
            InstructionKind::IntegerCompare {
                comparison: match op {
                    CompareOperator::Eq => NumericComparison::Equal,
                    CompareOperator::NotEq => NumericComparison::NotEqual,
                    CompareOperator::Lt => NumericComparison::LessThan,
                    CompareOperator::Le => NumericComparison::LessEqual,
                    CompareOperator::Gt => NumericComparison::GreaterThan,
                    CompareOperator::Ge => NumericComparison::GreaterEqual,
                },
            },
            vec![
                value(values, *lhs, ValueType::I64)?.id,
                value(values, *rhs, ValueType::I64)?.id,
            ],
            ValueType::Bool,
            Effect::Pure,
        ),
        WvmInstruction::UnaryOp {
            dst,
            op: UnaryOperator::Not,
            src,
        } => (
            *dst,
            InstructionKind::BooleanNot,
            vec![value(values, *src, ValueType::Bool)?.id],
            ValueType::Bool,
            Effect::Pure,
        ),
        WvmInstruction::BooleanOp { dst, op, lhs, rhs } => (
            *dst,
            match op {
                BooleanOperator::And => InstructionKind::BooleanAnd,
                BooleanOperator::Or => InstructionKind::BooleanOr,
            },
            vec![
                value(values, *lhs, ValueType::Bool)?.id,
                value(values, *rhs, ValueType::Bool)?.id,
            ],
            ValueType::Bool,
            Effect::Pure,
        ),
        WvmInstruction::BuildList { dst, items }
            if items.is_empty() && owned_local_list_at(executable, *dst, pc as usize) =>
        {
            let capacity = next(next_value, ValueType::I64)?;
            instructions.push(Instruction::new(
                InstructionKind::Constant(Constant::Integer(0)).at_pc(pc),
                Vec::new(),
                Some(capacity),
                Effect::Pure,
            ));
            (
                *dst,
                InstructionKind::OwnedList {
                    identity: pc.saturating_add(1),
                    element_type: ValueType::I64,
                    reset_on_definition: true,
                    copy_from_source: false,
                },
                vec![capacity.id],
                ValueType::Handle,
                Effect::Pure,
            )
        }
        WvmInstruction::GetItem { dst, object, key } => (
            *dst,
            InstructionKind::ListGet,
            vec![
                value(values, *object, ValueType::Handle)?.id,
                value(values, *key, ValueType::I64)?.id,
            ],
            ValueType::I64,
            Effect::Read,
        ),
        WvmInstruction::ListAppend {
            list,
            value: appended,
        } => (
            *list,
            InstructionKind::ListAppend,
            vec![
                value(values, *list, ValueType::Handle)?.id,
                value(values, *appended, ValueType::I64)?.id,
            ],
            ValueType::Handle,
            Effect::Write,
        ),
        WvmInstruction::ListInsert {
            list,
            index,
            value: inserted,
        } => (
            *list,
            InstructionKind::ListInsert,
            vec![
                value(values, *list, ValueType::Handle)?.id,
                value(values, *index, ValueType::I64)?.id,
                value(values, *inserted, ValueType::I64)?.id,
            ],
            ValueType::Handle,
            Effect::Write,
        ),
        WvmInstruction::ListPop { dst, list, index } => (
            *dst,
            InstructionKind::ListPop,
            vec![
                value(values, *list, ValueType::Handle)?.id,
                value(values, *index, ValueType::I64)?.id,
            ],
            ValueType::I64,
            Effect::Write,
        ),
        WvmInstruction::Length { dst, object } => (
            *dst,
            InstructionKind::ListLength,
            vec![value(values, *object, ValueType::Handle)?.id],
            ValueType::I64,
            Effect::Read,
        ),
        WvmInstruction::Call {
            dst,
            callable,
            args,
        } => {
            let constant = loaded
                .get(callable)
                .copied()
                .ok_or_else(|| "scalar CFG call target crossed a block".to_owned())?;
            let Some(ExecutableConstant::Function(callee)) = executable.constants().get(constant)
            else {
                return Err("scalar CFG call target missing".to_owned());
            };
            let output = inline_conditional(callee, args, values, pc, instructions, next_value)?;
            values.insert(*dst, output);
            return Ok(());
        }
        WvmInstruction::Jump { .. }
        | WvmInstruction::Branch { .. }
        | WvmInstruction::Return { .. } => return Ok(()),
        _ => return Err("unsupported scalar CFG instruction".to_owned()),
    };
    let output = next(next_value, ty)?;
    let instruction = Instruction::new(kind.at_pc(pc), inputs, Some(output), effect);
    let order = instructions
        .iter()
        .filter(|instruction| instruction.effect.is_ordered())
        .count() as u32;
    instructions.push(if effect.is_ordered() {
        instruction.ordered(order)
    } else {
        instruction
    });
    values.insert(dst, output);
    Ok(())
}

fn inline_conditional(
    callee: &ExecutableFunction,
    args: &[Register],
    values: &BTreeMap<Register, ValueDef>,
    pc: u32,
    instructions: &mut Vec<Instruction>,
    next_value: &mut u32,
) -> Result<ValueDef, String> {
    crate::verifier::verify(callee)?;
    if callee.parameters().len() != 3 || args.len() != 3 {
        return Err("unsupported scalar conditional callee".to_owned());
    }
    let parameters = callee.parameters();
    let code = callee.bytecode().code.as_slice();
    let Some(WvmInstruction::Branch {
        cond: enabled_cond, ..
    }) = code.get(3)
    else {
        return Err("unsupported scalar conditional guard".to_owned());
    };
    let Some(WvmInstruction::CompareOp {
        op: comparison,
        lhs: compared_left,
        rhs: compared_right,
        ..
    }) = code.get(4)
    else {
        return Err("unsupported scalar conditional comparison".to_owned());
    };
    let binary = code
        .iter()
        .filter_map(|instruction| match instruction {
            WvmInstruction::BinaryOp { op, lhs, rhs, .. } => Some((*op, *lhs, *rhs)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if *enabled_cond != parameters[2].register
        && !matches!(code.get(2), Some(WvmInstruction::Move { dst, src }) if *dst == *enabled_cond && *src == parameters[2].register)
    {
        return Err("unsupported scalar conditional boolean".to_owned());
    }
    if *compared_left != parameters[0].register
        || *compared_right != parameters[1].register
        || binary.len() != 2
        || binary
            .iter()
            .any(|(_, lhs, rhs)| *lhs != parameters[0].register || *rhs != parameters[1].register)
    {
        return Err("unsupported scalar conditional operands".to_owned());
    }
    let left = value(values, args[0], ValueType::I64)?;
    let right = value(values, args[1], ValueType::I64)?;
    let enabled = value(values, args[2], ValueType::Bool)?;
    let condition = emit(
        next_value,
        instructions,
        InstructionKind::IntegerCompare {
            comparison: numeric_comparison(*comparison),
        },
        vec![left.id, right.id],
        ValueType::Bool,
        pc,
    )?;
    let both = emit(
        next_value,
        instructions,
        InstructionKind::BooleanAnd,
        vec![enabled.id, condition.id],
        ValueType::Bool,
        pc,
    )?;
    let yes = emit(
        next_value,
        instructions,
        integer_operation(binary[0].0)?,
        vec![left.id, right.id],
        ValueType::I64,
        pc,
    )?;
    let no = emit(
        next_value,
        instructions,
        integer_operation(binary[1].0)?,
        vec![left.id, right.id],
        ValueType::I64,
        pc,
    )?;
    emit(
        next_value,
        instructions,
        InstructionKind::Select,
        vec![both.id, yes.id, no.id],
        ValueType::I64,
        pc,
    )
}

const fn numeric_comparison(comparison: CompareOperator) -> NumericComparison {
    match comparison {
        CompareOperator::Eq => NumericComparison::Equal,
        CompareOperator::NotEq => NumericComparison::NotEqual,
        CompareOperator::Lt => NumericComparison::LessThan,
        CompareOperator::Le => NumericComparison::LessEqual,
        CompareOperator::Gt => NumericComparison::GreaterThan,
        CompareOperator::Ge => NumericComparison::GreaterEqual,
    }
}

fn integer_operation(operation: BinaryOperator) -> Result<InstructionKind, String> {
    match operation {
        BinaryOperator::Add => Ok(InstructionKind::IntegerAdd),
        BinaryOperator::Subtract => Ok(InstructionKind::IntegerSubtract),
        BinaryOperator::Multiply => Ok(InstructionKind::IntegerMultiply),
        BinaryOperator::Divide | BinaryOperator::FloorDivide | BinaryOperator::Power => {
            Err("unsupported scalar conditional arithmetic".to_owned())
        }
    }
}

fn emit(
    next_value: &mut u32,
    instructions: &mut Vec<Instruction>,
    kind: InstructionKind,
    inputs: Vec<ValueId>,
    ty: ValueType,
    pc: u32,
) -> Result<ValueDef, String> {
    emit_effect(next_value, instructions, kind, inputs, ty, Effect::Pure, pc)
}

fn emit_effect(
    next_value: &mut u32,
    instructions: &mut Vec<Instruction>,
    kind: InstructionKind,
    inputs: Vec<ValueId>,
    ty: ValueType,
    effect: Effect,
    pc: u32,
) -> Result<ValueDef, String> {
    let output = next(next_value, ty)?;
    let instruction = Instruction::new(kind.at_pc(pc), inputs, Some(output), effect);
    let order = instructions
        .iter()
        .filter(|instruction| instruction.effect.is_ordered())
        .count() as u32;
    instructions.push(if effect.is_ordered() {
        instruction.ordered(order)
    } else {
        instruction
    });
    Ok(output)
}
fn arguments(
    registers: &[(Register, ValueType)],
    values: &BTreeMap<Register, ValueDef>,
) -> Result<Vec<ValueId>, String> {
    registers
        .iter()
        .map(|(register, _)| {
            values
                .get(register)
                .map(|value| value.id)
                .ok_or_else(|| format!("missing scalar CFG argument r{register}"))
        })
        .collect()
}
fn value(
    values: &BTreeMap<Register, ValueDef>,
    register: Register,
    ty: ValueType,
) -> Result<ValueDef, String> {
    let value = *values
        .get(&register)
        .ok_or_else(|| format!("missing scalar r{register}"))?;
    (value.ty == ty)
        .then_some(value)
        .ok_or_else(|| format!("scalar type changed for r{register}"))
}
fn next(next_value: &mut u32, ty: ValueType) -> Result<ValueDef, String> {
    let id = *next_value;
    *next_value = next_value
        .checked_add(1)
        .ok_or_else(|| "scalar CFG value overflow".to_owned())?;
    Ok(ValueDef::new(ValueId::new(id), ty))
}
fn block_id(index: usize) -> Result<BlockId, String> {
    Ok(BlockId::new(
        u32::try_from(index).map_err(|_| "scalar CFG block overflow".to_owned())?,
    ))
}
fn target_id(ids: &BTreeMap<usize, BlockId>, pc: usize) -> Result<BlockId, String> {
    ids.get(&pc)
        .copied()
        .ok_or_else(|| format!("scalar CFG target {pc} is not a leader"))
}

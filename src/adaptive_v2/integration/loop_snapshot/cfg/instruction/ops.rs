use std::collections::BTreeMap;

use super::super::SsaValue;
use crate::adaptive_v2::wxir_v2::ir::{Constant, InstructionKind, ValueType};
use crate::bytecode::{BinaryOperator, Instruction as WvmInstruction, Register};
use crate::executable::{ExecutableConstant, ExecutableFunction};
use crate::structure_map::{Fact, SlotType};

mod scalars;

pub(super) struct LoweredOp {
    pub(super) kind: InstructionKind,
    pub(super) inputs: Vec<crate::adaptive_v2::wxir_v2::ir::ValueId>,
    pub(super) dst: Register,
    pub(super) ty: ValueType,
}

pub(super) fn lower(
    executable: &ExecutableFunction,
    pc: usize,
    instruction: &WvmInstruction,
    values: &BTreeMap<Register, SsaValue>,
    element_types: &BTreeMap<Register, ValueType>,
) -> Result<Option<LoweredOp>, String> {
    Ok(match instruction {
        WvmInstruction::ConstSmallInt { dst, value } | WvmInstruction::ConstI64 { dst, value } => {
            Some(LoweredOp {
                kind: InstructionKind::Constant(Constant::Integer(*value)),
                inputs: Vec::new(),
                dst: *dst,
                ty: ValueType::I64,
            })
        }
        WvmInstruction::ConstFloat { dst, value } => Some(LoweredOp {
            kind: InstructionKind::Constant(Constant::FloatBits(value.to_bits())),
            inputs: Vec::new(),
            dst: *dst,
            ty: ValueType::F64,
        }),
        WvmInstruction::ConstBool { dst, value } => Some(LoweredOp {
            kind: InstructionKind::Constant(Constant::Boolean(*value)),
            inputs: Vec::new(),
            dst: *dst,
            ty: ValueType::Bool,
        }),
        WvmInstruction::BinaryOp {
            dst, op, lhs, rhs, ..
        } => Some(binary(values, *dst, *op, *lhs, *rhs)?),
        WvmInstruction::CompareOp {
            dst, op, lhs, rhs, ..
        } => Some(scalars::compare(values, *dst, *op, *lhs, *rhs)?),
        WvmInstruction::UnaryOp { dst, op, src } => Some(scalars::unary(values, *dst, *op, *src)?),
        WvmInstruction::BooleanOp { dst, op, lhs, rhs } => {
            Some(scalars::boolean(values, *dst, *op, *lhs, *rhs)?)
        }
        WvmInstruction::AddI64 { dst, lhs, rhs } => Some(scalars::integer_binary(
            values,
            *dst,
            *lhs,
            *rhs,
            InstructionKind::IntegerAdd,
        )?),
        WvmInstruction::LtI64 { dst, lhs, rhs } => Some(scalars::integer_compare(
            values,
            *dst,
            *lhs,
            *rhs,
            crate::adaptive_v2::wxir_v2::ir::NumericComparison::LessThan,
        )?),
        WvmInstruction::Move { dst, src } => {
            let source = scalars::read(values, *src)?;
            Some(LoweredOp {
                kind: InstructionKind::Copy,
                inputs: vec![source.id],
                dst: *dst,
                ty: source.ty,
            })
        }
        WvmInstruction::GetItem { dst, object, key } => {
            let list = scalars::read(values, *object)?;
            let index = scalars::read(values, *key)?;
            if list.ty != ValueType::Handle || index.ty != ValueType::I64 {
                return Err("adaptive-v2 loop list access types are unsupported".to_owned());
            }
            Some(LoweredOp {
                kind: InstructionKind::ListGet,
                inputs: vec![list.id, index.id],
                dst: *dst,
                ty: element_types
                    .get(dst)
                    .or_else(|| element_types.get(object))
                    .copied()
                    .map(Ok)
                    .unwrap_or_else(|| output_type(executable, pc, *dst))?,
            })
        }
        WvmInstruction::Length { dst, object } => {
            let list = scalars::read(values, *object)?;
            if list.ty != ValueType::Handle {
                return Err("adaptive-v2 loop list length type is unsupported".to_owned());
            }
            Some(LoweredOp {
                kind: InstructionKind::ListLength,
                inputs: vec![list.id],
                dst: *dst,
                ty: ValueType::I64,
            })
        }
        WvmInstruction::SetItem { object, key, value } => {
            let list = scalars::read(values, *object)?;
            let index = scalars::read(values, *key)?;
            let stored = scalars::read(values, *value)?;
            if list.ty != ValueType::Handle || index.ty != ValueType::I64 {
                return Err("adaptive-v2 loop list write types are unsupported".to_owned());
            }
            Some(LoweredOp {
                kind: InstructionKind::ListSet,
                inputs: vec![list.id, index.id, stored.id],
                dst: *object,
                ty: ValueType::Handle,
            })
        }
        WvmInstruction::Call {
            dst,
            callable,
            args,
        } => Some(inline_call(executable, pc, values, *dst, *callable, args)?),
        WvmInstruction::ListAppend { list, value } => {
            let list_value = scalars::read(values, *list)?;
            let appended = scalars::read(values, *value)?;
            if list_value.ty != ValueType::Handle
                || !matches!(appended.ty, ValueType::I64 | ValueType::F64)
            {
                return Err("adaptive-v2 loop list append types are unsupported".to_owned());
            }
            Some(LoweredOp {
                kind: InstructionKind::ListAppend,
                inputs: vec![list_value.id, appended.id],
                dst: *list,
                ty: ValueType::Handle,
            })
        }
        WvmInstruction::ListInsert { list, index, value } => {
            let list_value = scalars::read(values, *list)?;
            let index_value = scalars::read(values, *index)?;
            let inserted = scalars::read(values, *value)?;
            if list_value.ty != ValueType::Handle
                || index_value.ty != ValueType::I64
                || inserted.ty != ValueType::I64
            {
                return Err("adaptive-v2 loop list insert types are unsupported".to_owned());
            }
            Some(LoweredOp {
                kind: InstructionKind::ListInsert,
                inputs: vec![list_value.id, index_value.id, inserted.id],
                dst: *list,
                ty: ValueType::Handle,
            })
        }
        WvmInstruction::ListPop { dst, list, index } => {
            let list_value = scalars::read(values, *list)?;
            let index_value = scalars::read(values, *index)?;
            if list_value.ty != ValueType::Handle || index_value.ty != ValueType::I64 {
                return Err("adaptive-v2 loop list pop types are unsupported".to_owned());
            }
            Some(LoweredOp {
                kind: InstructionKind::ListPop,
                inputs: vec![list_value.id, index_value.id],
                dst: *dst,
                ty: ValueType::I64,
            })
        }
        WvmInstruction::ConstNone { .. }
        | WvmInstruction::LoadConstant { .. }
        | WvmInstruction::BuildTuple { .. }
        | WvmInstruction::BuildList { .. }
        | WvmInstruction::BuildDict { .. }
        | WvmInstruction::GetAttr { .. }
        | WvmInstruction::GetSlice { .. }
        | WvmInstruction::SetAttr { .. }
        | WvmInstruction::SetSlice { .. }
        | WvmInstruction::LoadCurrentFunction { .. }
        | WvmInstruction::CallMethod { .. }
        | WvmInstruction::Jump { .. }
        | WvmInstruction::Branch { .. }
        | WvmInstruction::Return { .. } => None,
    })
}

pub(super) fn is_inlined_constant(
    executable: &ExecutableFunction,
    pc: usize,
    register: Register,
    constant: usize,
) -> bool {
    matches!(
        executable.constants().get(constant),
        Some(ExecutableConstant::Function(_))
    ) && executable.bytecode().code[pc.saturating_add(1)..]
        .iter()
        .any(|instruction| {
            matches!(instruction, WvmInstruction::Call { callable, .. } if *callable == register)
        })
}

fn output_type(
    executable: &ExecutableFunction,
    pc: usize,
    register: Register,
) -> Result<ValueType, String> {
    let proven = executable
        .structure_map()
        .instruction_fact(pc)
        .and_then(|instruction| instruction.output)
        .and_then(|value| executable.structure_map().value(value))
        .and_then(|value| match value.ty {
            Fact::Proven(slot) => Some(slot),
            Fact::Guardable(_) | Fact::Unknown => None,
        });
    if let Some(slot) = proven {
        return value_type(slot);
    }
    let mut aliases = std::collections::BTreeSet::from([register]);
    for instruction in &executable.bytecode().code[pc.saturating_add(1)..] {
        if let WvmInstruction::Move { dst, src } = instruction
            && aliases.contains(src)
        {
            aliases.insert(*dst);
            continue;
        }
        if matches!(instruction, WvmInstruction::GetItem { object, .. } | WvmInstruction::Length { object, .. } | WvmInstruction::SetItem { object, .. } if aliases.contains(object))
        {
            return Ok(ValueType::Handle);
        }
        let WvmInstruction::Call { callable, args, .. } = instruction else {
            continue;
        };
        let Some(position) = args.iter().position(|argument| aliases.contains(argument)) else {
            continue;
        };
        let Some(callee) = constant_callee(executable, pc, *callable) else {
            continue;
        };
        crate::verifier::verify(callee)?;
        if let Some(parameter) = callee.parameters().get(position) {
            return value_type(parameter.ty);
        }
    }
    Ok(ValueType::I64)
}

fn value_type(slot: SlotType) -> Result<ValueType, String> {
    match slot {
        SlotType::SmallInt => Ok(ValueType::I64),
        SlotType::Float => Ok(ValueType::F64),
        SlotType::Bool => Ok(ValueType::Bool),
        SlotType::Object(_) => Ok(ValueType::Handle),
        SlotType::Any => Err("adaptive-v2 loop list element type is unknown".to_owned()),
    }
}

fn inline_call(
    executable: &ExecutableFunction,
    pc: usize,
    values: &BTreeMap<Register, SsaValue>,
    dst: Register,
    callable: Register,
    args: &[Register],
) -> Result<LoweredOp, String> {
    let callee = constant_callee(executable, pc, callable)
        .ok_or_else(|| "adaptive-v2 loop call target is not a function".to_owned())?;
    crate::verifier::verify(callee)?;
    let [operation, WvmInstruction::Return { src }] = callee.bytecode().code.as_slice() else {
        return Err("adaptive-v2 loop callee body is unsupported".to_owned());
    };
    let WvmInstruction::BinaryOp {
        dst: result,
        op,
        lhs,
        rhs,
        ..
    } = operation
    else {
        return Err("adaptive-v2 loop callee operation is unsupported".to_owned());
    };
    let [left, right] = callee.parameters() else {
        return Err("adaptive-v2 loop callee arity is unsupported".to_owned());
    };
    if args.len() != 2 || *result != *src || *lhs != left.register || *rhs != right.register {
        return Err("adaptive-v2 loop callee ABI is unsupported".to_owned());
    }
    binary(values, dst, *op, args[0], args[1])
}

pub(super) fn constant_callee(
    executable: &ExecutableFunction,
    pc: usize,
    callable: Register,
) -> Option<&ExecutableFunction> {
    let constant = executable.bytecode().code[..pc]
        .iter()
        .rev()
        .find_map(|instruction| match instruction {
            WvmInstruction::LoadConstant { dst, constant } if *dst == callable => Some(constant.0),
            _ => None,
        })?;
    match executable.constants().get(constant) {
        Some(ExecutableConstant::Function(callee)) => Some(callee),
        _ => None,
    }
}

fn binary(
    values: &BTreeMap<Register, SsaValue>,
    dst: Register,
    op: BinaryOperator,
    lhs: Register,
    rhs: Register,
) -> Result<LoweredOp, String> {
    let left = scalars::read(values, lhs)?;
    let right = scalars::read(values, rhs)?;
    let (kind, ty) = match (op, left.ty, right.ty) {
        (BinaryOperator::Add, ValueType::I64, ValueType::I64) => {
            (InstructionKind::IntegerAdd, ValueType::I64)
        }
        (BinaryOperator::Subtract, ValueType::I64, ValueType::I64) => {
            (InstructionKind::IntegerSubtract, ValueType::I64)
        }
        (BinaryOperator::Multiply, ValueType::I64, ValueType::I64) => {
            (InstructionKind::IntegerMultiply, ValueType::I64)
        }
        (BinaryOperator::Add, ValueType::F64, ValueType::F64) => {
            (InstructionKind::FloatAdd, ValueType::F64)
        }
        (BinaryOperator::Subtract, ValueType::F64, ValueType::F64) => {
            (InstructionKind::FloatSubtract, ValueType::F64)
        }
        (BinaryOperator::Multiply, ValueType::F64, ValueType::F64) => {
            (InstructionKind::FloatMultiply, ValueType::F64)
        }
        (BinaryOperator::Divide, ValueType::F64, ValueType::F64) => {
            (InstructionKind::FloatDivide, ValueType::F64)
        }
        (BinaryOperator::Power, ValueType::F64, ValueType::F64) => {
            (InstructionKind::FloatPower, ValueType::F64)
        }
        (BinaryOperator::Divide | BinaryOperator::FloorDivide | BinaryOperator::Power, _, _)
        | (BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply, _, _) => {
            return Err("adaptive-v2 loop arithmetic operand types are unsupported".to_owned());
        }
    };
    Ok(LoweredOp {
        kind,
        inputs: vec![left.id, right.id],
        dst,
        ty,
    })
}

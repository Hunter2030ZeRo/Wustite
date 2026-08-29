use std::collections::BTreeMap;

use super::{LoweredOp, SsaValue};
use crate::adaptive_v2::wxir_v2::ir::{InstructionKind, NumericComparison, ValueType};
use crate::bytecode::{BooleanOperator, CompareOperator, Register, UnaryOperator};

pub(super) fn compare(
    values: &BTreeMap<Register, SsaValue>,
    dst: Register,
    op: CompareOperator,
    lhs: Register,
    rhs: Register,
) -> Result<LoweredOp, String> {
    let left = read(values, lhs)?;
    let right = read(values, rhs)?;
    let comparison = comparison(op);
    let kind = match (left.ty, right.ty) {
        (ValueType::I64, ValueType::I64) => InstructionKind::IntegerCompare { comparison },
        (ValueType::F64, ValueType::F64) => InstructionKind::FloatCompare { comparison },
        _ => return Err("adaptive-v2 loop comparison operand types are unsupported".to_owned()),
    };
    Ok(LoweredOp {
        kind,
        inputs: vec![left.id, right.id],
        dst,
        ty: ValueType::Bool,
    })
}

pub(super) fn unary(
    values: &BTreeMap<Register, SsaValue>,
    dst: Register,
    op: UnaryOperator,
    src: Register,
) -> Result<LoweredOp, String> {
    let source = read(values, src)?;
    let kind = match (op, source.ty) {
        (UnaryOperator::Negate, ValueType::I64) => InstructionKind::IntegerNegate,
        (UnaryOperator::Negate, ValueType::F64) => InstructionKind::FloatNegate,
        (UnaryOperator::Not, ValueType::Bool) => InstructionKind::BooleanNot,
        _ => return Err("adaptive-v2 loop unary operand type is unsupported".to_owned()),
    };
    Ok(LoweredOp {
        kind,
        inputs: vec![source.id],
        dst,
        ty: source.ty,
    })
}

pub(super) fn boolean(
    values: &BTreeMap<Register, SsaValue>,
    dst: Register,
    op: BooleanOperator,
    lhs: Register,
    rhs: Register,
) -> Result<LoweredOp, String> {
    let left = read(values, lhs)?;
    let right = read(values, rhs)?;
    if left.ty != ValueType::Bool || right.ty != ValueType::Bool {
        return Err("adaptive-v2 loop boolean operand type is unsupported".to_owned());
    }
    Ok(LoweredOp {
        kind: match op {
            BooleanOperator::And => InstructionKind::BooleanAnd,
            BooleanOperator::Or => InstructionKind::BooleanOr,
        },
        inputs: vec![left.id, right.id],
        dst,
        ty: ValueType::Bool,
    })
}

pub(super) fn integer_binary(
    values: &BTreeMap<Register, SsaValue>,
    dst: Register,
    lhs: Register,
    rhs: Register,
    kind: InstructionKind,
) -> Result<LoweredOp, String> {
    let [left, right] = typed_inputs(values, lhs, rhs, ValueType::I64)?;
    Ok(LoweredOp {
        kind,
        inputs: vec![left.id, right.id],
        dst,
        ty: ValueType::I64,
    })
}

pub(super) fn integer_compare(
    values: &BTreeMap<Register, SsaValue>,
    dst: Register,
    lhs: Register,
    rhs: Register,
    comparison: NumericComparison,
) -> Result<LoweredOp, String> {
    let [left, right] = typed_inputs(values, lhs, rhs, ValueType::I64)?;
    Ok(LoweredOp {
        kind: InstructionKind::IntegerCompare { comparison },
        inputs: vec![left.id, right.id],
        dst,
        ty: ValueType::Bool,
    })
}

pub(super) fn read(
    values: &BTreeMap<Register, SsaValue>,
    register: Register,
) -> Result<SsaValue, String> {
    values
        .get(&register)
        .copied()
        .ok_or_else(|| format!("adaptive-v2 loop reads undefined r{register}"))
}

fn typed_inputs(
    values: &BTreeMap<Register, SsaValue>,
    lhs: Register,
    rhs: Register,
    ty: ValueType,
) -> Result<[SsaValue; 2], String> {
    let result = [read(values, lhs)?, read(values, rhs)?];
    if result.iter().any(|value| value.ty != ty) {
        return Err("adaptive-v2 loop operand type changed".to_owned());
    }
    Ok(result)
}

const fn comparison(op: CompareOperator) -> NumericComparison {
    match op {
        CompareOperator::Eq => NumericComparison::Equal,
        CompareOperator::NotEq => NumericComparison::NotEqual,
        CompareOperator::Lt => NumericComparison::LessThan,
        CompareOperator::Le => NumericComparison::LessEqual,
        CompareOperator::Gt => NumericComparison::GreaterThan,
        CompareOperator::Ge => NumericComparison::GreaterEqual,
    }
}

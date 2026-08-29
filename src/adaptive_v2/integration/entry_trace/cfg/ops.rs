use crate::adaptive_v2::wxir_v2::ir::{
    Constant, InstructionKind, NumericComparison, ValueId, ValueType,
};
use crate::bytecode::{
    BinaryOperator, BooleanOperator, CompareOperator, Instruction as WvmInstruction, Register,
    UnaryOperator,
};

pub(super) struct Operation {
    pub(super) kind: InstructionKind,
    pub(super) inputs: Vec<ValueId>,
    pub(super) dst: Register,
    pub(super) ty: ValueType,
}

pub(super) fn lower(
    instruction: &WvmInstruction,
    lookup: impl Fn(Register) -> Result<(ValueId, ValueType), String>,
) -> Result<Operation, String> {
    let operation = match instruction {
        WvmInstruction::ConstSmallInt { dst, value } | WvmInstruction::ConstI64 { dst, value } => {
            Operation {
                kind: InstructionKind::Constant(Constant::Integer(*value)),
                inputs: Vec::new(),
                dst: *dst,
                ty: ValueType::I64,
            }
        }
        WvmInstruction::ConstFloat { dst, value } => Operation {
            kind: InstructionKind::Constant(Constant::FloatBits(value.to_bits())),
            inputs: Vec::new(),
            dst: *dst,
            ty: ValueType::F64,
        },
        WvmInstruction::ConstBool { dst, value } => Operation {
            kind: InstructionKind::Constant(Constant::Boolean(*value)),
            inputs: Vec::new(),
            dst: *dst,
            ty: ValueType::Bool,
        },
        WvmInstruction::Move { dst, src } => {
            let (source, ty) = lookup(*src)?;
            Operation {
                kind: InstructionKind::Copy,
                inputs: vec![source],
                dst: *dst,
                ty,
            }
        }
        WvmInstruction::AddI64 { dst, lhs, rhs } => binary(
            *dst,
            *lhs,
            *rhs,
            ValueType::I64,
            ValueType::I64,
            InstructionKind::IntegerAdd,
            &lookup,
        )?,
        WvmInstruction::LtI64 { dst, lhs, rhs } => binary(
            *dst,
            *lhs,
            *rhs,
            ValueType::I64,
            ValueType::Bool,
            InstructionKind::IntegerLessThan,
            &lookup,
        )?,
        WvmInstruction::BinaryOp {
            dst, op, lhs, rhs, ..
        } => {
            let (_, ty) = same_type(*lhs, *rhs, &lookup)?;
            binary(
                *dst,
                *lhs,
                *rhs,
                ty,
                binary_output(*op, ty)?,
                binary_kind(*op, ty)?,
                &lookup,
            )?
        }
        WvmInstruction::CompareOp {
            dst, op, lhs, rhs, ..
        } => {
            let (_, ty) = same_type(*lhs, *rhs, &lookup)?;
            binary(
                *dst,
                *lhs,
                *rhs,
                ty,
                ValueType::Bool,
                compare_kind(*op, ty)?,
                &lookup,
            )?
        }
        WvmInstruction::UnaryOp { dst, op, src } => {
            let (source, ty) = lookup(*src)?;
            Operation {
                kind: unary_kind(*op, ty)?,
                inputs: vec![source],
                dst: *dst,
                ty: unary_output(*op, ty)?,
            }
        }
        WvmInstruction::BooleanOp { dst, op, lhs, rhs } => binary(
            *dst,
            *lhs,
            *rhs,
            ValueType::Bool,
            ValueType::Bool,
            match op {
                BooleanOperator::And => InstructionKind::BooleanAnd,
                BooleanOperator::Or => InstructionKind::BooleanOr,
            },
            &lookup,
        )?,
        _ => return Err("unsupported WVM instruction".to_owned()),
    };
    Ok(operation)
}

fn binary(
    dst: Register,
    lhs: Register,
    rhs: Register,
    input: ValueType,
    output: ValueType,
    kind: InstructionKind,
    lookup: &impl Fn(Register) -> Result<(ValueId, ValueType), String>,
) -> Result<Operation, String> {
    let (left, left_ty) = lookup(lhs)?;
    let (right, right_ty) = lookup(rhs)?;
    if left_ty != input || right_ty != input {
        return Err("adaptive-v2 entry scalar input type changed".to_owned());
    }
    Ok(Operation {
        kind,
        inputs: vec![left, right],
        dst,
        ty: output,
    })
}

fn same_type(
    lhs: Register,
    rhs: Register,
    lookup: &impl Fn(Register) -> Result<(ValueId, ValueType), String>,
) -> Result<(ValueId, ValueType), String> {
    let left = lookup(lhs)?;
    let right = lookup(rhs)?;
    if left.1 != right.1 {
        return Err("adaptive-v2 entry requires monomorphic scalar operands".to_owned());
    }
    Ok(left)
}

fn binary_kind(op: BinaryOperator, ty: ValueType) -> Result<InstructionKind, String> {
    match (op, ty) {
        (BinaryOperator::Add, ValueType::I64) => Ok(InstructionKind::IntegerAdd),
        (BinaryOperator::Subtract, ValueType::I64) => Ok(InstructionKind::IntegerSubtract),
        (BinaryOperator::Multiply, ValueType::I64) => Ok(InstructionKind::IntegerMultiply),
        (BinaryOperator::Add, ValueType::F64) => Ok(InstructionKind::FloatAdd),
        (BinaryOperator::Subtract, ValueType::F64) => Ok(InstructionKind::FloatSubtract),
        (BinaryOperator::Multiply, ValueType::F64) => Ok(InstructionKind::FloatMultiply),
        (BinaryOperator::Divide, ValueType::F64) => Ok(InstructionKind::FloatDivide),
        _ => Err("unsupported adaptive-v2 scalar binary operation".to_owned()),
    }
}

fn binary_output(op: BinaryOperator, ty: ValueType) -> Result<ValueType, String> {
    binary_kind(op, ty).map(|_| ty)
}

fn compare_kind(op: CompareOperator, ty: ValueType) -> Result<InstructionKind, String> {
    let comparison = match op {
        CompareOperator::Eq => NumericComparison::Equal,
        CompareOperator::NotEq => NumericComparison::NotEqual,
        CompareOperator::Lt => NumericComparison::LessThan,
        CompareOperator::Le => NumericComparison::LessEqual,
        CompareOperator::Gt => NumericComparison::GreaterThan,
        CompareOperator::Ge => NumericComparison::GreaterEqual,
    };
    match ty {
        ValueType::I64 => Ok(InstructionKind::IntegerCompare { comparison }),
        ValueType::F64 => Ok(InstructionKind::FloatCompare { comparison }),
        _ => Err("unsupported adaptive-v2 comparison type".to_owned()),
    }
}

fn unary_kind(op: UnaryOperator, ty: ValueType) -> Result<InstructionKind, String> {
    match (op, ty) {
        (UnaryOperator::Negate, ValueType::I64) => Ok(InstructionKind::IntegerNegate),
        (UnaryOperator::Negate, ValueType::F64) => Ok(InstructionKind::FloatNegate),
        (UnaryOperator::Not, ValueType::Bool) => Ok(InstructionKind::BooleanNot),
        _ => Err("unsupported adaptive-v2 unary operation".to_owned()),
    }
}

fn unary_output(op: UnaryOperator, ty: ValueType) -> Result<ValueType, String> {
    unary_kind(op, ty).map(|_| ty)
}

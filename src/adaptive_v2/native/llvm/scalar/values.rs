use std::collections::BTreeMap;

use inkwell::context::Context;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicValueEnum, FloatValue, IntValue};

use super::{NativeError, ValueId, ValueType};
use crate::adaptive_v2::wxir_v2::ir::{Constant, NumericComparison};

pub(super) fn native_type<'ctx>(
    context: &'ctx Context,
    ty: ValueType,
) -> Result<BasicTypeEnum<'ctx>, NativeError> {
    match ty {
        ValueType::I64 | ValueType::Handle => Ok(context.i64_type().into()),
        ValueType::F64 => Ok(context.f64_type().into()),
        ValueType::Bool => Ok(context.bool_type().into()),
        ValueType::BorrowedView => Err(NativeError::Unsupported("scalar LLVM value type")),
    }
}

pub(super) fn constant_value<'ctx>(
    context: &'ctx Context,
    constant: &Constant,
) -> Result<BasicValueEnum<'ctx>, NativeError> {
    match constant {
        Constant::Integer(value) => Ok(context.i64_type().const_int(*value as u64, true).into()),
        Constant::FloatBits(value) => Ok(context
            .f64_type()
            .const_float(f64::from_bits(*value))
            .into()),
        Constant::Boolean(value) => Ok(context
            .bool_type()
            .const_int(u64::from(*value), false)
            .into()),
        Constant::HandleBits(value) => Ok(context.i64_type().const_int(*value, false).into()),
        Constant::UndefinedDead => Err(NativeError::Unsupported("scalar LLVM constant")),
    }
}

pub(super) fn value<'ctx>(
    values: &BTreeMap<ValueId, BasicValueEnum<'ctx>>,
    id: ValueId,
) -> Result<BasicValueEnum<'ctx>, NativeError> {
    values
        .get(&id)
        .copied()
        .ok_or(NativeError::Unsupported("missing scalar LLVM value"))
}

pub(super) fn integer_inputs<'ctx>(
    values: &BTreeMap<ValueId, BasicValueEnum<'ctx>>,
    inputs: &[ValueId],
) -> Result<[IntValue<'ctx>; 2], NativeError> {
    match inputs {
        [left, right] => Ok([
            value(values, *left)?.into_int_value(),
            value(values, *right)?.into_int_value(),
        ]),
        _ => Err(NativeError::Unsupported("scalar LLVM integer arity")),
    }
}

pub(super) fn float_inputs<'ctx>(
    values: &BTreeMap<ValueId, BasicValueEnum<'ctx>>,
    inputs: &[ValueId],
) -> Result<[FloatValue<'ctx>; 2], NativeError> {
    match inputs {
        [left, right] => Ok([
            value(values, *left)?.into_float_value(),
            value(values, *right)?.into_float_value(),
        ]),
        _ => Err(NativeError::Unsupported("scalar LLVM float arity")),
    }
}

pub(super) const fn integer_predicate(comparison: NumericComparison) -> inkwell::IntPredicate {
    match comparison {
        NumericComparison::Equal => inkwell::IntPredicate::EQ,
        NumericComparison::NotEqual => inkwell::IntPredicate::NE,
        NumericComparison::LessThan => inkwell::IntPredicate::SLT,
        NumericComparison::LessEqual => inkwell::IntPredicate::SLE,
        NumericComparison::GreaterThan => inkwell::IntPredicate::SGT,
        NumericComparison::GreaterEqual => inkwell::IntPredicate::SGE,
    }
}

pub(super) const fn float_predicate(comparison: NumericComparison) -> inkwell::FloatPredicate {
    match comparison {
        NumericComparison::Equal => inkwell::FloatPredicate::OEQ,
        NumericComparison::NotEqual => inkwell::FloatPredicate::UNE,
        NumericComparison::LessThan => inkwell::FloatPredicate::OLT,
        NumericComparison::LessEqual => inkwell::FloatPredicate::OLE,
        NumericComparison::GreaterThan => inkwell::FloatPredicate::OGT,
        NumericComparison::GreaterEqual => inkwell::FloatPredicate::OGE,
    }
}

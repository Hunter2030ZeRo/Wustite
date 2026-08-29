use std::collections::HashMap;

use cranelift_codegen::ir::{InstBuilder, Value, condcodes::IntCC, types};
use cranelift_frontend::FunctionBuilder;

use crate::bytecode::Register;
use crate::object::SequenceStrategy;
use crate::wxir::{WxRuntimeInput, WxScalarType, WxType, WxValueId};

pub(super) fn scalar_input(
    values: &HashMap<WxValueId, Value>,
    inputs: &[WxRuntimeInput],
    object: Register,
    scalar: WxScalarType,
) -> Option<Value> {
    let input = inputs
        .iter()
        .find(|input| input.register != object && input.ty == WxType::Scalar(scalar))?;
    values.get(&input.value).copied()
}

pub(super) fn stored_value(
    values: &HashMap<WxValueId, Value>,
    inputs: &[WxRuntimeInput],
    object: Register,
    strategy: SequenceStrategy,
) -> Option<Value> {
    let scalar = match strategy {
        SequenceStrategy::Bool => WxScalarType::I1,
        SequenceStrategy::I64 => WxScalarType::I64,
        SequenceStrategy::F64 => WxScalarType::F64,
        SequenceStrategy::Empty | SequenceStrategy::Object => return None,
    };
    let input = inputs
        .iter()
        .rev()
        .find(|input| input.register != object && input.ty == WxType::Scalar(scalar))?;
    values.get(&input.value).copied()
}

pub(super) fn strategy_type(
    strategy: SequenceStrategy,
    result: WxType,
) -> Option<cranelift_codegen::ir::Type> {
    let scalar = match strategy {
        SequenceStrategy::Bool => WxScalarType::I1,
        SequenceStrategy::I64 => WxScalarType::I64,
        SequenceStrategy::F64 => WxScalarType::F64,
        SequenceStrategy::Empty | SequenceStrategy::Object => return None,
    };
    (result == WxType::Scalar(scalar)).then(|| clif_scalar(scalar))
}

const fn clif_scalar(scalar: WxScalarType) -> cranelift_codegen::ir::Type {
    match scalar {
        WxScalarType::I1 => types::I8,
        WxScalarType::I64 => types::I64,
        WxScalarType::F64 => types::F64,
        _ => unreachable!(),
    }
}

pub(super) fn normalized_index(
    builder: &mut FunctionBuilder<'_>,
    index: Value,
    len: Value,
) -> Value {
    let negative = builder.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
    let wrapped = builder.ins().iadd(index, len);
    builder.ins().select(negative, wrapped, index)
}

pub(super) fn invalid_index(builder: &mut FunctionBuilder<'_>, index: Value, len: Value) -> Value {
    let negative = builder.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
    let too_large = builder
        .ins()
        .icmp(IntCC::SignedGreaterThanOrEqual, index, len);
    builder.ins().bor(negative, too_large)
}

pub(super) fn element_address(
    builder: &mut FunctionBuilder<'_>,
    data: Value,
    index: Value,
    strategy: SequenceStrategy,
) -> Value {
    let size = match strategy {
        SequenceStrategy::Bool => 1,
        SequenceStrategy::I64 | SequenceStrategy::F64 => 8,
        SequenceStrategy::Empty | SequenceStrategy::Object => unreachable!(),
    };
    let offset = builder.ins().imul_imm_s(index, size);
    builder.ins().iadd(data, offset)
}

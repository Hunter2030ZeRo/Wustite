use std::collections::HashMap;

use inkwell::basic_block::BasicBlock;
use inkwell::context::Context;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicValue, BasicValueEnum, IntValue, PhiValue};

use crate::wxir::{
    WxBlockId, WxExitId, WxFunction, WxInst, WxInstResult, WxScalarType, WxSideExit, WxType,
    WxValueId,
};

use super::super::CompileError;

pub(super) fn llvm_type(context: &Context, ty: WxType) -> Result<BasicTypeEnum<'_>, CompileError> {
    match ty {
        WxType::Scalar(WxScalarType::I1) => Ok(context.bool_type().into()),
        WxType::Scalar(WxScalarType::I64) => Ok(context.i64_type().into()),
        _ => Err(CompileError::UnsupportedType(ty)),
    }
}

pub(super) fn one_result(instruction: &WxInst) -> Result<WxInstResult, CompileError> {
    instruction
        .results
        .first()
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction("instruction has no result".to_string()))
}

pub(super) fn two_results(instruction: &WxInst) -> Result<[WxInstResult; 2], CompileError> {
    match instruction.results.as_slice() {
        [value, overflow] => Ok([*value, *overflow]),
        _ => Err(CompileError::InvalidFunction(
            "checked integer instruction requires two results".to_string(),
        )),
    }
}

pub(super) fn block_for<'ctx>(
    blocks: &HashMap<WxBlockId, BasicBlock<'ctx>>,
    block: WxBlockId,
) -> Result<BasicBlock<'ctx>, CompileError> {
    blocks
        .get(&block)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing block {block}")))
}

pub(super) fn exit_block_for<'ctx>(
    blocks: &HashMap<WxExitId, BasicBlock<'ctx>>,
    exit: WxExitId,
) -> Result<BasicBlock<'ctx>, CompileError> {
    blocks
        .get(&exit)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing exit block {exit}")))
}

pub(super) fn value_for<'ctx>(
    values: &HashMap<WxValueId, BasicValueEnum<'ctx>>,
    value: WxValueId,
) -> Result<BasicValueEnum<'ctx>, CompileError> {
    values
        .get(&value)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing value {value}")))
}

pub(super) fn int_value_for<'ctx>(
    values: &HashMap<WxValueId, BasicValueEnum<'ctx>>,
    value: WxValueId,
) -> Result<IntValue<'ctx>, CompileError> {
    match value_for(values, value)? {
        BasicValueEnum::IntValue(value) => Ok(value),
        _ => Err(CompileError::InvalidFunction(format!(
            "value {value} is not an integer"
        ))),
    }
}

pub(super) fn lower_values<'ctx>(
    values: &HashMap<WxValueId, BasicValueEnum<'ctx>>,
    arguments: &[WxValueId],
) -> Result<Vec<BasicValueEnum<'ctx>>, CompileError> {
    arguments
        .iter()
        .map(|argument| value_for(values, *argument))
        .collect()
}

pub(super) fn add_incoming<'ctx>(
    phis: &[PhiValue<'ctx>],
    values: &[BasicValueEnum<'ctx>],
    predecessor: BasicBlock<'ctx>,
) -> Result<(), CompileError> {
    if phis.len() != values.len() {
        return Err(CompileError::InvalidFunction(
            "control-flow argument count does not match block parameters".to_string(),
        ));
    }
    for (phi, value) in phis.iter().zip(values) {
        phi.add_incoming(&[(value as &dyn BasicValue<'ctx>, predecessor)]);
    }
    Ok(())
}

pub(super) fn side_exit(
    function: &WxFunction,
    exit: WxExitId,
) -> Result<&WxSideExit, CompileError> {
    function
        .side_exits
        .iter()
        .find(|metadata| metadata.id == exit)
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing side exit {exit}")))
}

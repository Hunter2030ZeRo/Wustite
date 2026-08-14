use std::collections::HashMap;

use cranelift_codegen::ir::{Block, BlockArg, Type, Value as ClifValue, types};

use crate::wxir::{
    WxBlockId, WxExitId, WxFunction, WxInst, WxInstResult, WxScalarType, WxSideExit, WxType,
    WxValueId,
};

use super::CompileError;

pub(super) fn clif_type(ty: WxType) -> Result<Type, CompileError> {
    match ty {
        WxType::Scalar(WxScalarType::I1) => Ok(types::I8),
        WxType::Scalar(WxScalarType::I64) => Ok(types::I64),
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

pub(super) fn block_for(
    blocks: &HashMap<WxBlockId, Block>,
    block: WxBlockId,
) -> Result<Block, CompileError> {
    blocks
        .get(&block)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing block {block}")))
}

pub(super) fn exit_block_for(
    blocks: &HashMap<WxExitId, Block>,
    exit: WxExitId,
) -> Result<Block, CompileError> {
    blocks
        .get(&exit)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing exit block {exit}")))
}

pub(super) fn value_for(
    values: &HashMap<WxValueId, ClifValue>,
    value: WxValueId,
) -> Result<ClifValue, CompileError> {
    values
        .get(&value)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing value {value}")))
}

pub(super) fn lower_values(
    values: &HashMap<WxValueId, ClifValue>,
    arguments: &[WxValueId],
) -> Result<Vec<BlockArg>, CompileError> {
    arguments
        .iter()
        .map(|argument| value_for(values, *argument).map(BlockArg::from))
        .collect()
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

pub(super) fn offset_i32(offset: usize) -> Result<i32, CompileError> {
    i32::try_from(offset)
        .map_err(|_| CompileError::InvalidFunction("state layout exceeds i32 offsets".to_string()))
}

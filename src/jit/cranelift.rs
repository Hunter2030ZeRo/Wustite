use std::collections::HashMap;

use cranelift_codegen::ir::{
    AbiParam, Block, BlockArg, InstBuilder, MemFlagsData, UserFuncName, Value as ClifValue,
    condcodes::IntCC, types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};

use crate::wxir::{
    WxBinaryOp, WxBlockId, WxCompareOp, WxConstant, WxExitId, WxFloatBinaryOp, WxFunction,
    WxGuardMode, WxInstKind, WxIntBinaryOp, WxIntCompareOp, WxIntOverflowOp, WxScalarType,
    WxTerminator, WxType, WxValueId,
};

use super::compiled_region::{CompiledRegion, NativeRegionEntry};
use super::layout::RegionLayout;
use super::{CompileError, RegionCompiler};

/// Cranelift implementation of the WXIR region compiler.
#[derive(Debug, Default)]
pub struct CraneliftRegionCompiler;

impl CraneliftRegionCompiler {
    pub fn new() -> Self {
        Self
    }
}

impl RegionCompiler for CraneliftRegionCompiler {
    fn compile(&mut self, function: &WxFunction) -> Result<CompiledRegion, CompileError> {
        crate::wxir::verify(function).map_err(CompileError::InvalidFunction)?;
        let layout = RegionLayout::new(function)?;

        let jit_builder = JITBuilder::new(default_libcall_names())
            .map_err(|error| CompileError::Backend(error.to_string()))?;
        let mut module = JITModule::new(jit_builder);
        let pointer_type = module.target_config().pointer_type();
        let mut signature = module.make_signature();
        signature.params.push(AbiParam::new(pointer_type));
        signature.returns.push(AbiParam::new(types::I32));
        let function_id = module
            .declare_function("wustite_region", Linkage::Local, &signature)
            .map_err(|error| CompileError::Backend(error.to_string()))?;

        let mut context = module.make_context();
        context.func.signature = signature;
        context.func.name = UserFuncName::user(0, function_id.as_u32());
        let mut builder_context = FunctionBuilderContext::new();

        {
            let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            lower_function(&mut builder, function, &layout)?;
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }

        module
            .define_function(function_id, &mut context)
            .map_err(|error| CompileError::Backend(format!("{error:#?}")))?;
        module.clear_context(&mut context);
        module
            .finalize_definitions()
            .map_err(|error| CompileError::Backend(error.to_string()))?;

        let code_ptr = module.get_finalized_function(function_id);
        // SAFETY: this symbol was declared with the private state-pointer/u32
        // ABI, and `CompiledRegion` takes ownership of `module` below.
        let entry: NativeRegionEntry = unsafe { std::mem::transmute(code_ptr) };
        Ok(CompiledRegion::new(
            module,
            entry,
            layout,
            function.entry_state.clone(),
            function.side_exits.clone(),
        ))
    }
}

fn lower_function(
    builder: &mut FunctionBuilder<'_>,
    function: &WxFunction,
    layout: &RegionLayout,
) -> Result<(), CompileError> {
    let mem_flags = MemFlagsData::new();

    let mut blocks = HashMap::new();
    for block in &function.blocks {
        let clif_block = builder.create_block();
        for parameter in &block.parameters {
            builder.append_block_param(clif_block, clif_type(parameter.ty)?);
        }
        blocks.insert(block.id, clif_block);
    }

    let mut exit_blocks = HashMap::new();
    for exit in &function.side_exits {
        let clif_block = builder.create_block();
        for state in &exit.state {
            builder.append_block_param(clif_block, clif_type(state.ty)?);
        }
        exit_blocks.insert(exit.id, clif_block);
    }

    let mut values = HashMap::new();
    for block in &function.blocks {
        let clif_block = block_for(&blocks, block.id)?;
        let parameters = builder.block_params(clif_block).to_vec();
        for (parameter, value) in block.parameters.iter().zip(parameters) {
            values.insert(parameter.id, value);
        }
    }

    let prologue = builder.create_block();
    builder.switch_to_block(prologue);
    builder.append_block_params_for_function_params(prologue);
    let state_pointer = builder
        .block_params(prologue)
        .first()
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction("missing native state pointer".to_string()))?;

    let entry_block = function
        .blocks
        .iter()
        .find(|block| block.id == function.entry)
        .ok_or_else(|| CompileError::InvalidFunction("missing WXIR entry block".to_string()))?;
    let mut entry_arguments = Vec::with_capacity(entry_block.parameters.len());
    for parameter in &entry_block.parameters {
        let state = function
            .entry_state
            .iter()
            .find(|state| state.value == parameter.id)
            .ok_or_else(|| {
                CompileError::InvalidFunction(format!(
                    "entry parameter {} has no WVM state mapping",
                    parameter.id
                ))
            })?;
        let slot = layout.slot(state.register)?;
        let value = builder.ins().load(
            clif_type(state.ty)?,
            mem_flags,
            state_pointer,
            offset_i32(slot.offset)?,
        );
        entry_arguments.push(value.into());
    }
    builder
        .ins()
        .jump(block_for(&blocks, function.entry)?, &entry_arguments);

    for block in &function.blocks {
        builder.switch_to_block(block_for(&blocks, block.id)?);
        for instruction in &block.instructions {
            match &instruction.kind {
                WxInstKind::Constant(constant) => {
                    let result = one_result(instruction)?;
                    let value = match constant {
                        WxConstant::Bool(value) => {
                            builder.ins().iconst(types::I8, i64::from(*value))
                        }
                        WxConstant::Int(value)
                            if result.ty == WxType::Scalar(WxScalarType::I64) =>
                        {
                            builder.ins().iconst(types::I64, *value)
                        }
                        _ => return Err(CompileError::UnsupportedInstruction("Constant")),
                    };
                    values.insert(result.id, value);
                }
                WxInstKind::Binary { op, lhs, rhs } => {
                    let result = one_result(instruction)?;
                    if result.ty != WxType::Scalar(WxScalarType::I64) {
                        return Err(CompileError::UnsupportedType(result.ty));
                    }
                    let lhs = value_for(&values, *lhs)?;
                    let rhs = value_for(&values, *rhs)?;
                    let value = match op {
                        WxBinaryOp::Integer(WxIntBinaryOp::Add) => builder.ins().iadd(lhs, rhs),
                        WxBinaryOp::Integer(_)
                        | WxBinaryOp::Float(WxFloatBinaryOp::Add)
                        | WxBinaryOp::Float(WxFloatBinaryOp::Sub)
                        | WxBinaryOp::Float(WxFloatBinaryOp::Mul)
                        | WxBinaryOp::Float(WxFloatBinaryOp::Div) => {
                            return Err(CompileError::UnsupportedInstruction("Binary"));
                        }
                    };
                    values.insert(result.id, value);
                }
                WxInstKind::IntegerBinaryWithOverflow { op, lhs, rhs } => {
                    let [result, overflow] = two_results(instruction)?;
                    if result.ty != WxType::Scalar(WxScalarType::I64) {
                        return Err(CompileError::UnsupportedType(result.ty));
                    }
                    let lhs = value_for(&values, *lhs)?;
                    let rhs = value_for(&values, *rhs)?;
                    let (value, overflow_value) = match op {
                        WxIntOverflowOp::Add => builder.ins().sadd_overflow(lhs, rhs),
                    };
                    values.insert(result.id, value);
                    values.insert(overflow.id, overflow_value);
                }
                WxInstKind::Compare { op, lhs, rhs } => {
                    let result = one_result(instruction)?;
                    if result.ty != WxType::Scalar(WxScalarType::I1) {
                        return Err(CompileError::UnsupportedType(result.ty));
                    }
                    let value = match op {
                        WxCompareOp::Integer(WxIntCompareOp::SignedLt) => builder.ins().icmp(
                            IntCC::SignedLessThan,
                            value_for(&values, *lhs)?,
                            value_for(&values, *rhs)?,
                        ),
                        _ => return Err(CompileError::UnsupportedInstruction("Compare")),
                    };
                    values.insert(result.id, value);
                }
                WxInstKind::Guard {
                    condition,
                    exit,
                    mode,
                } => {
                    let condition = value_for(&values, *condition)?;
                    let exit_metadata = side_exit(function, *exit)?;
                    let exit_arguments = exit_metadata
                        .state
                        .iter()
                        .map(|state| value_for(&values, state.value).map(BlockArg::from))
                        .collect::<Result<Vec<_>, _>>()?;
                    let continuation = builder.create_block();
                    let exit_block = exit_block_for(&exit_blocks, *exit)?;
                    match mode {
                        WxGuardMode::ExitWhenTrue => {
                            builder.ins().brif(
                                condition,
                                exit_block,
                                &exit_arguments,
                                continuation,
                                &[],
                            );
                        }
                        WxGuardMode::ExitWhenFalse => {
                            builder.ins().brif(
                                condition,
                                continuation,
                                &[],
                                exit_block,
                                &exit_arguments,
                            );
                        }
                    }
                    builder.switch_to_block(continuation);
                }
                WxInstKind::Cast { .. } => {
                    return Err(CompileError::UnsupportedInstruction("Cast"));
                }
                WxInstKind::Load { .. } => {
                    return Err(CompileError::UnsupportedInstruction("Load"));
                }
                WxInstKind::Store { .. } => {
                    return Err(CompileError::UnsupportedInstruction("Store"));
                }
                WxInstKind::PointerOffset { .. } => {
                    return Err(CompileError::UnsupportedInstruction("PointerOffset"));
                }
                WxInstKind::Splat { .. } => {
                    return Err(CompileError::UnsupportedInstruction("Splat"));
                }
                WxInstKind::ExtractLane { .. } => {
                    return Err(CompileError::UnsupportedInstruction("ExtractLane"));
                }
                WxInstKind::InsertLane { .. } => {
                    return Err(CompileError::UnsupportedInstruction("InsertLane"));
                }
                WxInstKind::Shuffle { .. } => {
                    return Err(CompileError::UnsupportedInstruction("Shuffle"));
                }
                WxInstKind::Call { .. } => {
                    return Err(CompileError::UnsupportedInstruction("Call"));
                }
            }
        }

        match &block.terminator {
            WxTerminator::Jump { target, arguments } => {
                builder.ins().jump(
                    block_for(&blocks, *target)?,
                    &lower_values(&values, arguments)?,
                );
            }
            WxTerminator::Branch { condition, yes, no } => {
                builder.ins().brif(
                    value_for(&values, *condition)?,
                    block_for(&blocks, yes.block)?,
                    &lower_values(&values, &yes.arguments)?,
                    block_for(&blocks, no.block)?,
                    &lower_values(&values, &no.arguments)?,
                );
            }
            WxTerminator::SideExit {
                exit,
                values: state,
            } => {
                builder.ins().jump(
                    exit_block_for(&exit_blocks, *exit)?,
                    &lower_values(&values, state)?,
                );
            }
            WxTerminator::Return { .. } => {
                return Err(CompileError::UnsupportedInstruction("Return"));
            }
        }
    }

    for exit in &function.side_exits {
        let exit_block = exit_block_for(&exit_blocks, exit.id)?;
        builder.switch_to_block(exit_block);
        let parameters = builder.block_params(exit_block).to_vec();
        for (state, value) in exit.state.iter().zip(parameters) {
            let slot = layout.slot(state.register)?;
            builder
                .ins()
                .store(mem_flags, value, state_pointer, offset_i32(slot.offset)?);
        }
        let exit_id = builder.ins().iconst(types::I32, i64::from(exit.id.0));
        builder.ins().return_(&[exit_id]);
    }

    Ok(())
}

fn clif_type(ty: WxType) -> Result<cranelift_codegen::ir::Type, CompileError> {
    match ty {
        WxType::Scalar(WxScalarType::I1) => Ok(types::I8),
        WxType::Scalar(WxScalarType::I64) => Ok(types::I64),
        _ => Err(CompileError::UnsupportedType(ty)),
    }
}

fn one_result(
    instruction: &crate::wxir::WxInst,
) -> Result<crate::wxir::WxInstResult, CompileError> {
    instruction
        .results
        .first()
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction("instruction has no result".to_string()))
}

fn two_results(
    instruction: &crate::wxir::WxInst,
) -> Result<[crate::wxir::WxInstResult; 2], CompileError> {
    match instruction.results.as_slice() {
        [value, overflow] => Ok([*value, *overflow]),
        _ => Err(CompileError::InvalidFunction(
            "checked integer instruction requires two results".to_string(),
        )),
    }
}

fn block_for(blocks: &HashMap<WxBlockId, Block>, block: WxBlockId) -> Result<Block, CompileError> {
    blocks
        .get(&block)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing block {block}")))
}

fn exit_block_for(
    blocks: &HashMap<WxExitId, Block>,
    exit: WxExitId,
) -> Result<Block, CompileError> {
    blocks
        .get(&exit)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing exit block {exit}")))
}

fn value_for(
    values: &HashMap<WxValueId, ClifValue>,
    value: WxValueId,
) -> Result<ClifValue, CompileError> {
    values
        .get(&value)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing value {value}")))
}

fn lower_values(
    values: &HashMap<WxValueId, ClifValue>,
    arguments: &[WxValueId],
) -> Result<Vec<BlockArg>, CompileError> {
    arguments
        .iter()
        .map(|argument| value_for(values, *argument).map(BlockArg::from))
        .collect()
}

fn side_exit(
    function: &WxFunction,
    exit: WxExitId,
) -> Result<&crate::wxir::WxSideExit, CompileError> {
    function
        .side_exits
        .iter()
        .find(|metadata| metadata.id == exit)
        .ok_or_else(|| CompileError::InvalidFunction(format!("missing side exit {exit}")))
}

fn offset_i32(offset: usize) -> Result<i32, CompileError> {
    i32::try_from(offset)
        .map_err(|_| CompileError::InvalidFunction("state layout exceeds i32 offsets".to_string()))
}

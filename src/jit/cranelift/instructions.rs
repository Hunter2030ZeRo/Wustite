use std::collections::HashMap;

use cranelift_codegen::ir::{
    Block, BlockArg, InstBuilder, Value as ClifValue, condcodes::IntCC, types,
};
use cranelift_frontend::FunctionBuilder;

use crate::wxir::{
    WxBinaryOp, WxCompareOp, WxConstant, WxExitId, WxFloatBinaryOp, WxFunction, WxGuardMode,
    WxInst, WxInstKind, WxIntBinaryOp, WxIntCompareOp, WxIntOverflowOp, WxScalarType, WxType,
    WxValueId,
};

use super::CompileError;
use super::helpers::{exit_block_for, one_result, side_exit, two_results, value_for};

pub(super) fn lower_instruction(
    builder: &mut FunctionBuilder<'_>,
    function: &WxFunction,
    exit_blocks: &HashMap<WxExitId, Block>,
    values: &mut HashMap<WxValueId, ClifValue>,
    instruction: &WxInst,
) -> Result<(), CompileError> {
    match &instruction.kind {
        WxInstKind::Constant(constant) => {
            let result = one_result(instruction)?;
            let value = match constant {
                WxConstant::Bool(value) => builder.ins().iconst(types::I8, i64::from(*value)),
                WxConstant::Int(value) if result.ty == WxType::Scalar(WxScalarType::I64) => {
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
            let lhs = value_for(values, *lhs)?;
            let rhs = value_for(values, *rhs)?;
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
            let lhs = value_for(values, *lhs)?;
            let rhs = value_for(values, *rhs)?;
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
                    value_for(values, *lhs)?,
                    value_for(values, *rhs)?,
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
            let condition = value_for(values, *condition)?;
            let exit_metadata = side_exit(function, *exit)?;
            let exit_arguments = exit_metadata
                .state
                .iter()
                .map(|state| value_for(values, state.value).map(BlockArg::from))
                .collect::<Result<Vec<_>, _>>()?;
            let continuation = builder.create_block();
            let exit_block = exit_block_for(exit_blocks, *exit)?;
            match mode {
                WxGuardMode::ExitWhenTrue => {
                    builder
                        .ins()
                        .brif(condition, exit_block, &exit_arguments, continuation, &[]);
                }
                WxGuardMode::ExitWhenFalse => {
                    builder
                        .ins()
                        .brif(condition, continuation, &[], exit_block, &exit_arguments);
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

    Ok(())
}

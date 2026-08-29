use std::collections::HashMap;

use cranelift_codegen::ir::{
    Block, BlockArg, InstBuilder, Value as ClifValue, condcodes::IntCC, immediates::Ieee64, types,
};
use cranelift_frontend::FunctionBuilder;

use crate::wxir::{
    WxBinaryOp, WxCastOp, WxCompareOp, WxConstant, WxExitId, WxFloatBinaryOp, WxFloatCompareOp,
    WxFunction, WxGuardMode, WxInst, WxInstKind, WxIntBinaryOp, WxIntCompareOp, WxIntOverflowOp,
    WxScalarType, WxType, WxValueId,
};

use super::CompileError;
use super::helpers::{exit_block_for, one_result, side_exit, two_results, value_for};
use super::native_calls::SequenceViewValues;
use super::native_calls::{RuntimeCall, RuntimeEnvironment, RuntimeFunctions};

mod sequence;

pub(super) fn lower_instruction(
    builder: &mut FunctionBuilder<'_>,
    function: &WxFunction,
    exit_blocks: &HashMap<WxExitId, Block>,
    runtime: &NativeRuntime<'_>,
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
                WxConstant::F64(value) if result.ty == WxType::Scalar(WxScalarType::F64) => {
                    builder.ins().f64const(Ieee64::with_bits(value.to_bits()))
                }
                _ => return Err(CompileError::UnsupportedInstruction("Constant")),
            };
            values.insert(result.id, value);
        }
        WxInstKind::Binary { op, lhs, rhs } => {
            let result = one_result(instruction)?;
            let lhs = value_for(values, *lhs)?;
            let rhs = value_for(values, *rhs)?;
            let value = match (result.ty, op) {
                (WxType::Scalar(WxScalarType::I64), WxBinaryOp::Integer(WxIntBinaryOp::Add)) => {
                    builder.ins().iadd(lhs, rhs)
                }
                (WxType::Scalar(WxScalarType::I64), WxBinaryOp::Integer(WxIntBinaryOp::Sub)) => {
                    builder.ins().isub(lhs, rhs)
                }
                (WxType::Scalar(WxScalarType::I64), WxBinaryOp::Integer(WxIntBinaryOp::Mul)) => {
                    builder.ins().imul(lhs, rhs)
                }
                (
                    WxType::Scalar(WxScalarType::I64),
                    WxBinaryOp::Integer(WxIntBinaryOp::FloorDiv),
                ) => {
                    let quotient = builder.ins().sdiv(lhs, rhs);
                    let remainder = builder.ins().srem(lhs, rhs);
                    let has_remainder = builder.ins().icmp_imm_s(IntCC::NotEqual, remainder, 0);
                    let lhs_negative = builder.ins().icmp_imm_s(IntCC::SignedLessThan, lhs, 0);
                    let rhs_negative = builder.ins().icmp_imm_s(IntCC::SignedLessThan, rhs, 0);
                    let signs_differ = builder.ins().bxor(lhs_negative, rhs_negative);
                    let adjust = builder.ins().band(has_remainder, signs_differ);
                    let minus_one = builder.ins().iconst(types::I64, -1);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let correction = builder.ins().select(adjust, minus_one, zero);
                    builder.ins().iadd(quotient, correction)
                }
                (WxType::Scalar(WxScalarType::I1), WxBinaryOp::Integer(WxIntBinaryOp::And)) => {
                    builder.ins().band(lhs, rhs)
                }
                (WxType::Scalar(WxScalarType::I1), WxBinaryOp::Integer(WxIntBinaryOp::Or)) => {
                    builder.ins().bor(lhs, rhs)
                }
                (WxType::Scalar(WxScalarType::F64), WxBinaryOp::Float(WxFloatBinaryOp::Add)) => {
                    builder.ins().fadd(lhs, rhs)
                }
                (WxType::Scalar(WxScalarType::F64), WxBinaryOp::Float(WxFloatBinaryOp::Sub)) => {
                    builder.ins().fsub(lhs, rhs)
                }
                (WxType::Scalar(WxScalarType::F64), WxBinaryOp::Float(WxFloatBinaryOp::Mul)) => {
                    builder.ins().fmul(lhs, rhs)
                }
                (WxType::Scalar(WxScalarType::F64), WxBinaryOp::Float(WxFloatBinaryOp::Div)) => {
                    builder.ins().fdiv(lhs, rhs)
                }
                (_, _) => {
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
                WxIntOverflowOp::Sub => builder.ins().ssub_overflow(lhs, rhs),
                WxIntOverflowOp::Mul => builder.ins().smul_overflow(lhs, rhs),
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
                WxCompareOp::Integer(WxIntCompareOp::Eq) => builder.ins().icmp(
                    IntCC::Equal,
                    value_for(values, *lhs)?,
                    value_for(values, *rhs)?,
                ),
                WxCompareOp::Integer(WxIntCompareOp::SignedLt) => builder.ins().icmp(
                    IntCC::SignedLessThan,
                    value_for(values, *lhs)?,
                    value_for(values, *rhs)?,
                ),
                WxCompareOp::Integer(WxIntCompareOp::Ne) => builder.ins().icmp(
                    IntCC::NotEqual,
                    value_for(values, *lhs)?,
                    value_for(values, *rhs)?,
                ),
                WxCompareOp::Integer(WxIntCompareOp::SignedLe) => builder.ins().icmp(
                    IntCC::SignedLessThanOrEqual,
                    value_for(values, *lhs)?,
                    value_for(values, *rhs)?,
                ),
                WxCompareOp::Float(op) => builder.ins().fcmp(
                    float_condition(*op),
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
        WxInstKind::Cast { op, value } => {
            let result = one_result(instruction)?;
            let value = match (op, result.ty) {
                (WxCastOp::IntToFloat { signed: true }, WxType::Scalar(WxScalarType::F64)) => {
                    builder
                        .ins()
                        .fcvt_from_sint(types::F64, value_for(values, *value)?)
                }
                _ => return Err(CompileError::UnsupportedInstruction("Cast")),
            };
            values.insert(result.id, value);
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
        WxInstKind::GuardSequence { .. } => {
            return Err(CompileError::UnsupportedInstruction("GuardSequence"));
        }
        WxInstKind::MaterializeSequence { .. } => {
            return Err(CompileError::UnsupportedInstruction("MaterializeSequence"));
        }
        WxInstKind::SequenceLength {
            pc,
            object,
            inputs,
            output,
            ..
        } => sequence::lower_length(
            builder,
            runtime,
            values,
            instruction,
            *pc,
            *object,
            inputs,
            *output,
        )?,
        WxInstKind::SequenceGet {
            pc,
            object,
            inputs,
            output,
            ..
        } => sequence::lower_get(
            builder,
            runtime,
            values,
            instruction,
            *pc,
            *object,
            inputs,
            *output,
        )?,
        WxInstKind::SequenceSet {
            pc, object, inputs, ..
        } => sequence::lower_set(builder, runtime, values, instruction, *pc, *object, inputs)?,
        WxInstKind::SequenceMutate {
            pc, inputs, output, ..
        } => runtime.functions.lower_call(
            builder,
            &mut RuntimeEnvironment {
                context: runtime.context,
                error_block: runtime.error_block,
                values,
            },
            RuntimeCall {
                instruction,
                pc: *pc,
                inputs,
                output: *output,
                sequence: true,
            },
        )?,
        WxInstKind::RuntimeCall {
            pc, inputs, output, ..
        } => runtime.functions.lower_call(
            builder,
            &mut RuntimeEnvironment {
                context: runtime.context,
                error_block: runtime.error_block,
                values,
            },
            RuntimeCall {
                instruction,
                pc: *pc,
                inputs,
                output: *output,
                sequence: false,
            },
        )?,
    }

    Ok(())
}

fn float_condition(op: WxFloatCompareOp) -> cranelift_codegen::ir::condcodes::FloatCC {
    use cranelift_codegen::ir::condcodes::FloatCC;
    match op {
        WxFloatCompareOp::Eq => FloatCC::Equal,
        WxFloatCompareOp::Ne => FloatCC::NotEqual,
        WxFloatCompareOp::Lt => FloatCC::LessThan,
        WxFloatCompareOp::Le => FloatCC::LessThanOrEqual,
        WxFloatCompareOp::Gt => FloatCC::GreaterThan,
        WxFloatCompareOp::Ge => FloatCC::GreaterThanOrEqual,
    }
}

pub(super) struct NativeRuntime<'a> {
    pub(super) functions: &'a RuntimeFunctions,
    pub(super) context: ClifValue,
    pub(super) error_block: Block,
    pub(super) sequence_views: &'a HashMap<WxValueId, SequenceViewValues>,
}

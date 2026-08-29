use std::collections::HashMap;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue, PhiValue, PointerValue};
use inkwell::{FloatPredicate, IntPredicate};

use crate::wxir::{
    WxBinaryOp, WxCastOp, WxCompareOp, WxConstant, WxExitId, WxFloatBinaryOp, WxFloatCompareOp,
    WxFunction, WxGuardMode, WxInst, WxInstKind, WxIntBinaryOp, WxIntCompareOp, WxIntOverflowOp,
    WxScalarType, WxType, WxValueId,
};

use super::CompileError;
use super::helpers::{
    add_incoming, float_value_for, int_value_for, one_result, side_exit, two_results,
};
use super::llvm_error;
use super::native_calls::{RuntimeCall, RuntimeEnvironment, RuntimeFunctions};

mod scalars;

pub(super) struct InstructionContext<'a, 'ctx> {
    pub(super) builder: &'a Builder<'ctx>,
    pub(super) module: &'a Module<'ctx>,
    pub(super) function: &'a WxFunction,
    pub(super) exit_blocks: &'a HashMap<WxExitId, BasicBlock<'ctx>>,
    pub(super) exit_phis: &'a HashMap<WxExitId, Vec<PhiValue<'ctx>>>,
    pub(super) runtime_functions: &'a RuntimeFunctions<'ctx>,
    pub(super) runtime_context: PointerValue<'ctx>,
    pub(super) runtime_error_block: BasicBlock<'ctx>,
    pub(super) llvm_function: FunctionValue<'ctx>,
    pub(super) values: &'a mut HashMap<WxValueId, BasicValueEnum<'ctx>>,
}

impl InstructionContext<'_, '_> {
    pub(super) fn lower(&mut self, instruction: &WxInst) -> Result<(), CompileError> {
        match &instruction.kind {
            WxInstKind::Constant(constant) => self.lower_constant(instruction, *constant),
            WxInstKind::Binary { op, lhs, rhs } => self.lower_binary(instruction, *op, *lhs, *rhs),
            WxInstKind::IntegerBinaryWithOverflow { op, lhs, rhs } => {
                self.lower_overflow(instruction, *op, *lhs, *rhs)
            }
            WxInstKind::Compare { op, lhs, rhs } => {
                self.lower_compare(instruction, *op, *lhs, *rhs)
            }
            WxInstKind::Guard {
                condition,
                exit,
                mode,
            } => self.lower_guard(*condition, *exit, *mode),
            WxInstKind::Cast { op, value } => self.lower_cast(instruction, *op, *value),
            WxInstKind::Load { .. } => Err(CompileError::UnsupportedInstruction("Load")),
            WxInstKind::Store { .. } => Err(CompileError::UnsupportedInstruction("Store")),
            WxInstKind::PointerOffset { .. } => {
                Err(CompileError::UnsupportedInstruction("PointerOffset"))
            }
            WxInstKind::Splat { .. } => Err(CompileError::UnsupportedInstruction("Splat")),
            WxInstKind::ExtractLane { .. } => {
                Err(CompileError::UnsupportedInstruction("ExtractLane"))
            }
            WxInstKind::InsertLane { .. } => {
                Err(CompileError::UnsupportedInstruction("InsertLane"))
            }
            WxInstKind::Shuffle { .. } => Err(CompileError::UnsupportedInstruction("Shuffle")),
            WxInstKind::Call { .. } => Err(CompileError::UnsupportedInstruction("Call")),
            WxInstKind::GuardSequence { .. } => {
                Err(CompileError::UnsupportedInstruction("GuardSequence"))
            }
            WxInstKind::MaterializeSequence { .. } => {
                Err(CompileError::UnsupportedInstruction("MaterializeSequence"))
            }
            WxInstKind::SequenceLength {
                pc, inputs, output, ..
            }
            | WxInstKind::SequenceGet {
                pc, inputs, output, ..
            } => self.runtime_functions.lower_call(
                self.builder,
                &mut RuntimeEnvironment {
                    context: self.runtime_context,
                    error_block: self.runtime_error_block,
                    llvm_function: self.llvm_function,
                    values: self.values,
                },
                RuntimeCall {
                    instruction,
                    pc: *pc,
                    inputs,
                    output: Some(*output),
                    sequence: true,
                },
            ),
            WxInstKind::SequenceSet { pc, inputs, .. } => self.runtime_functions.lower_call(
                self.builder,
                &mut RuntimeEnvironment {
                    context: self.runtime_context,
                    error_block: self.runtime_error_block,
                    llvm_function: self.llvm_function,
                    values: self.values,
                },
                RuntimeCall {
                    instruction,
                    pc: *pc,
                    inputs,
                    output: None,
                    sequence: true,
                },
            ),
            WxInstKind::SequenceMutate {
                pc, inputs, output, ..
            } => self.runtime_functions.lower_call(
                self.builder,
                &mut RuntimeEnvironment {
                    context: self.runtime_context,
                    error_block: self.runtime_error_block,
                    llvm_function: self.llvm_function,
                    values: self.values,
                },
                RuntimeCall {
                    instruction,
                    pc: *pc,
                    inputs,
                    output: *output,
                    sequence: true,
                },
            ),
            WxInstKind::RuntimeCall {
                pc, inputs, output, ..
            } => self.runtime_functions.lower_call(
                self.builder,
                &mut RuntimeEnvironment {
                    context: self.runtime_context,
                    error_block: self.runtime_error_block,
                    llvm_function: self.llvm_function,
                    values: self.values,
                },
                RuntimeCall {
                    instruction,
                    pc: *pc,
                    inputs,
                    output: *output,
                    sequence: false,
                },
            ),
        }
    }

    fn lower_constant(
        &mut self,
        instruction: &WxInst,
        constant: WxConstant,
    ) -> Result<(), CompileError> {
        let result = one_result(instruction)?;
        let value = match (constant, result.ty) {
            (WxConstant::Bool(value), WxType::Scalar(WxScalarType::I1)) => self
                .module
                .get_context()
                .bool_type()
                .const_int(u64::from(value), false)
                .into(),
            (WxConstant::Int(value), WxType::Scalar(WxScalarType::I64)) => self
                .module
                .get_context()
                .i64_type()
                .const_int(u64::from_ne_bytes(value.to_ne_bytes()), true)
                .into(),
            (WxConstant::F64(value), WxType::Scalar(WxScalarType::F64)) => self
                .module
                .get_context()
                .f64_type()
                .const_float(value)
                .into(),
            _ => return Err(CompileError::UnsupportedInstruction("Constant")),
        };
        self.values.insert(result.id, value);
        Ok(())
    }

    fn lower_overflow(
        &mut self,
        instruction: &WxInst,
        op: WxIntOverflowOp,
        lhs: WxValueId,
        rhs: WxValueId,
    ) -> Result<(), CompileError> {
        let [result, overflow] = two_results(instruction)?;
        if result.ty != WxType::Scalar(WxScalarType::I64) {
            return Err(CompileError::UnsupportedType(result.ty));
        }
        let intrinsic_name = match op {
            WxIntOverflowOp::Add => "llvm.sadd.with.overflow",
            WxIntOverflowOp::Sub => "llvm.ssub.with.overflow",
            WxIntOverflowOp::Mul => "llvm.smul.with.overflow",
        };
        let i64_type = self.module.get_context().i64_type();
        let intrinsic = Intrinsic::find(intrinsic_name).ok_or_else(|| {
            CompileError::Backend("LLVM overflow intrinsic is absent".to_string())
        })?;
        let declaration = intrinsic
            .get_declaration(self.module, &[i64_type.into()])
            .ok_or_else(|| CompileError::Backend("LLVM overflow declaration failed".to_string()))?;
        let call = self
            .builder
            .build_call(
                declaration,
                &[
                    int_value_for(self.values, lhs)?.into(),
                    int_value_for(self.values, rhs)?.into(),
                ],
                "checked_add",
            )
            .map_err(llvm_error)?;
        let aggregate = match call.try_as_basic_value().basic() {
            Some(BasicValueEnum::StructValue(value)) => value,
            _ => {
                return Err(CompileError::Backend(
                    "LLVM overflow intrinsic returned a non-struct".to_string(),
                ));
            }
        };
        let value = self
            .builder
            .build_extract_value(aggregate, 0, "sum")
            .map_err(llvm_error)?;
        let overflow_value = self
            .builder
            .build_extract_value(aggregate, 1, "overflow")
            .map_err(llvm_error)?;
        self.values.insert(result.id, value);
        self.values.insert(overflow.id, overflow_value);
        Ok(())
    }

    fn lower_guard(
        &mut self,
        condition: WxValueId,
        exit: WxExitId,
        mode: WxGuardMode,
    ) -> Result<(), CompileError> {
        let predecessor = self.builder.get_insert_block().ok_or_else(|| {
            CompileError::Backend("LLVM builder has no current block".to_string())
        })?;
        let exit_metadata = side_exit(self.function, exit)?;
        let exit_values = exit_metadata
            .state
            .iter()
            .map(|state| super::helpers::value_for(self.values, state.value))
            .collect::<Result<Vec<_>, _>>()?;
        let phis = self
            .exit_phis
            .get(&exit)
            .ok_or_else(|| CompileError::InvalidFunction(format!("missing exit phi {exit}")))?;
        add_incoming(phis, &exit_values, predecessor)?;
        let exit_block =
            self.exit_blocks.get(&exit).copied().ok_or_else(|| {
                CompileError::InvalidFunction(format!("missing exit block {exit}"))
            })?;
        let continuation = self.module.get_context().append_basic_block(
            predecessor.get_parent().ok_or_else(|| {
                CompileError::Backend("LLVM block has no parent function".to_string())
            })?,
            "guard_cont",
        );
        let condition = int_value_for(self.values, condition)?;
        match mode {
            WxGuardMode::ExitWhenTrue => self
                .builder
                .build_conditional_branch(condition, exit_block, continuation)
                .map_err(llvm_error)?,
            WxGuardMode::ExitWhenFalse => self
                .builder
                .build_conditional_branch(condition, continuation, exit_block)
                .map_err(llvm_error)?,
        };
        self.builder.position_at_end(continuation);
        Ok(())
    }
}

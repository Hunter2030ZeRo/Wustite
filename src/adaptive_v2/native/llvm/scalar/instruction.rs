use std::collections::BTreeMap;

use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};

use super::super::{NativeError, VerifiedSnapshot, llvm_error};
use super::values::{
    constant_value, float_inputs, float_predicate, integer_inputs, integer_predicate, value,
};
use crate::adaptive_v2::wxir_v2::ir::{InstructionKind, NumericComparison, ValueId};

pub(super) struct InstructionLowering<'a, 'ctx> {
    pub(super) context: &'ctx Context,
    pub(super) builder: &'a inkwell::builder::Builder<'ctx>,
    pub(super) function: FunctionValue<'ctx>,
    pub(super) module: &'a Module<'ctx>,
    pub(super) frame: PointerValue<'ctx>,
    pub(super) snapshot: &'a VerifiedSnapshot,
    pub(super) values: &'a BTreeMap<ValueId, BasicValueEnum<'ctx>>,
}

impl<'ctx> InstructionLowering<'_, 'ctx> {
    pub(super) fn lower(
        &self,
        kind: &InstructionKind,
        inputs: &[ValueId],
    ) -> Result<Option<BasicValueEnum<'ctx>>, NativeError> {
        let result = match kind {
            InstructionKind::Constant(constant) => constant_value(self.context, constant)?,
            InstructionKind::Copy => value(self.values, inputs[0])?,
            InstructionKind::IntegerAdd => {
                let [left, right] = integer_inputs(self.values, inputs)?;
                self.builder
                    .build_int_add(left, right, "iadd")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::IntegerSubtract => {
                let [left, right] = integer_inputs(self.values, inputs)?;
                self.builder
                    .build_int_sub(left, right, "isub")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::IntegerMultiply => {
                let [left, right] = integer_inputs(self.values, inputs)?;
                self.builder
                    .build_int_mul(left, right, "imul")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::IntegerFloorDivide { divisor } => {
                let left = value(self.values, inputs[0])?.into_int_value();
                let right = self.context.i64_type().const_int(*divisor as u64, true);
                let quotient = self
                    .builder
                    .build_int_signed_div(left, right, "ifloordiv.quotient")
                    .map_err(llvm_error)?;
                let remainder = self
                    .builder
                    .build_int_signed_rem(left, right, "ifloordiv.remainder")
                    .map_err(llvm_error)?;
                let zero = self.context.i64_type().const_zero();
                let has_remainder = self
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        remainder,
                        zero,
                        "ifloordiv.has_remainder",
                    )
                    .map_err(llvm_error)?;
                let left_negative = self
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::SLT,
                        left,
                        zero,
                        "ifloordiv.left_negative",
                    )
                    .map_err(llvm_error)?;
                let right_negative = self
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::SLT,
                        right,
                        zero,
                        "ifloordiv.right_negative",
                    )
                    .map_err(llvm_error)?;
                let signs_differ = self
                    .builder
                    .build_xor(left_negative, right_negative, "ifloordiv.signs_differ")
                    .map_err(llvm_error)?;
                let adjust = self
                    .builder
                    .build_and(has_remainder, signs_differ, "ifloordiv.adjust")
                    .map_err(llvm_error)?;
                let correction = self
                    .builder
                    .build_select(
                        adjust,
                        self.context.i64_type().const_int(1, false),
                        zero,
                        "ifloordiv.correction",
                    )
                    .map_err(llvm_error)?
                    .into_int_value();
                self.builder
                    .build_int_sub(quotient, correction, "ifloordiv")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::IntegerToFloat => self
                .builder
                .build_signed_int_to_float(
                    value(self.values, inputs[0])?.into_int_value(),
                    self.context.f64_type(),
                    "sitofp",
                )
                .map_err(llvm_error)?
                .into(),
            InstructionKind::IntegerLessThan => {
                self.integer_compare(NumericComparison::LessThan, inputs)?
            }
            InstructionKind::IntegerCompare { comparison } => {
                self.integer_compare(*comparison, inputs)?
            }
            InstructionKind::FloatAdd => {
                let [left, right] = float_inputs(self.values, inputs)?;
                self.builder
                    .build_float_add(left, right, "fadd")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::FloatSubtract => {
                let [left, right] = float_inputs(self.values, inputs)?;
                self.builder
                    .build_float_sub(left, right, "fsub")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::FloatMultiply => {
                let [left, right] = float_inputs(self.values, inputs)?;
                self.builder
                    .build_float_mul(left, right, "fmul")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::FloatDivide => {
                let [left, right] = float_inputs(self.values, inputs)?;
                self.builder
                    .build_float_div(left, right, "fdiv")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::FloatPower => {
                let [left, right] = float_inputs(self.values, inputs)?;
                let intrinsic = Intrinsic::find("llvm.pow")
                    .ok_or_else(|| NativeError::Backend("LLVM pow intrinsic is absent".into()))?;
                let declaration = intrinsic
                    .get_declaration(self.module, &[self.context.f64_type().into()])
                    .ok_or_else(|| NativeError::Backend("LLVM pow declaration failed".into()))?;
                self.builder
                    .build_call(declaration, &[left.into(), right.into()], "fpow")
                    .map_err(llvm_error)?
                    .try_as_basic_value()
                    .basic()
                    .ok_or_else(|| NativeError::Backend("LLVM pow result is absent".into()))?
            }
            InstructionKind::FloatCompare { comparison } => {
                let [left, right] = float_inputs(self.values, inputs)?;
                self.builder
                    .build_float_compare(float_predicate(*comparison), left, right, "fcmp")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::IntegerNegate => self
                .builder
                .build_int_neg(value(self.values, inputs[0])?.into_int_value(), "ineg")
                .map_err(llvm_error)?
                .into(),
            InstructionKind::FloatNegate => self
                .builder
                .build_float_neg(value(self.values, inputs[0])?.into_float_value(), "fneg")
                .map_err(llvm_error)?
                .into(),
            InstructionKind::BooleanNot => self
                .builder
                .build_xor(
                    value(self.values, inputs[0])?.into_int_value(),
                    self.context.bool_type().const_int(1, false),
                    "not",
                )
                .map_err(llvm_error)?
                .into(),
            InstructionKind::BooleanAnd => {
                let [left, right] = integer_inputs(self.values, inputs)?;
                self.builder
                    .build_and(left, right, "and")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::BooleanOr => {
                let [left, right] = integer_inputs(self.values, inputs)?;
                self.builder
                    .build_or(left, right, "or")
                    .map_err(llvm_error)?
                    .into()
            }
            InstructionKind::Select => {
                let condition = value(self.values, inputs[0])?.into_int_value();
                let yes = value(self.values, inputs[1])?;
                let no = value(self.values, inputs[2])?;
                self.builder
                    .build_select(condition, yes, no, "select")
                    .map_err(llvm_error)?
            }
            InstructionKind::Guard { guard } => {
                self.lower_guard(*guard, inputs)?;
                return Ok(None);
            }
            InstructionKind::ObjectGet
            | InstructionKind::ObjectSet
            | InstructionKind::OwnedList { .. }
            | InstructionKind::ListGet
            | InstructionKind::ListLength
            | InstructionKind::ListSet
            | InstructionKind::ListReversePrefix { .. }
            | InstructionKind::ListClear
            | InstructionKind::ListAppend
            | InstructionKind::ListInsert
            | InstructionKind::ListPop
            | InstructionKind::Call { .. }
            | InstructionKind::Allocate
            | InstructionKind::Helper { .. }
            | InstructionKind::BranchGuard { .. }
            | InstructionKind::NestedLoopExit { .. }
            | InstructionKind::BorrowView
            | InstructionKind::ResolveHandle
            | InstructionKind::LiveProbe
            | InstructionKind::AtPc { .. } => {
                return Err(NativeError::Unsupported("scalar LLVM instruction"));
            }
        };
        Ok(Some(result))
    }

    fn integer_compare(
        &self,
        comparison: NumericComparison,
        inputs: &[ValueId],
    ) -> Result<BasicValueEnum<'ctx>, NativeError> {
        let [left, right] = integer_inputs(self.values, inputs)?;
        self.builder
            .build_int_compare(integer_predicate(comparison), left, right, "icmp")
            .map(BasicValueEnum::from)
            .map_err(llvm_error)
    }

    fn lower_guard(&self, guard: u32, inputs: &[ValueId]) -> Result<(), NativeError> {
        let recipe = self
            .snapshot
            .body()
            .deopts
            .iter()
            .find(|recipe| recipe.id == guard)
            .ok_or(NativeError::Unsupported("missing scalar LLVM guard deopt"))?;
        super::super::lower_guard(
            self.context,
            self.builder,
            self.function,
            self.frame,
            value(self.values, inputs[0])?.into_int_value(),
            guard,
            recipe.root_point.get(),
        )
    }
}

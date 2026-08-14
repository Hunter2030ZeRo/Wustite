use inkwell::IntPredicate;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::values::{BasicValueEnum, PointerValue};

use crate::bytecode::Register;
use crate::wxir::{WxScalarType, WxType};

use super::{CompileError, RegionLayout, llvm_error};

pub(super) struct StateBuffer<'a, 'ctx> {
    pub(super) builder: &'a Builder<'ctx>,
    pub(super) context: &'ctx Context,
    pub(super) pointer: PointerValue<'ctx>,
    pub(super) layout: &'a RegionLayout,
}

impl<'ctx> StateBuffer<'_, 'ctx> {
    pub(super) fn load(
        &self,
        register: Register,
        ty: WxType,
    ) -> Result<BasicValueEnum<'ctx>, CompileError> {
        let pointer = self.slot_pointer(register)?;
        let word = self
            .builder
            .build_load(self.context.i64_type(), pointer, "state_word")
            .map_err(llvm_error)?;
        let BasicValueEnum::IntValue(word) = word else {
            return Err(CompileError::Backend(
                "LLVM state word is not an integer".to_string(),
            ));
        };
        match ty {
            WxType::Scalar(WxScalarType::I1) => self
                .builder
                .build_int_compare(
                    IntPredicate::NE,
                    word,
                    self.context.i64_type().const_zero(),
                    "state_bool",
                )
                .map(BasicValueEnum::from)
                .map_err(llvm_error),
            WxType::Scalar(WxScalarType::I64) => Ok(word.into()),
            _ => Err(CompileError::UnsupportedType(ty)),
        }
    }

    pub(super) fn store(
        &self,
        register: Register,
        ty: WxType,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), CompileError> {
        let BasicValueEnum::IntValue(value) = value else {
            return Err(CompileError::InvalidFunction(format!(
                "state value for r{register} is not an integer"
            )));
        };
        let word = match ty {
            WxType::Scalar(WxScalarType::I1) => self
                .builder
                .build_int_z_extend(value, self.context.i64_type(), "bool_word")
                .map_err(llvm_error)?,
            WxType::Scalar(WxScalarType::I64) => value,
            _ => return Err(CompileError::UnsupportedType(ty)),
        };
        self.builder
            .build_store(self.slot_pointer(register)?, word)
            .map_err(llvm_error)?;
        Ok(())
    }

    fn slot_pointer(&self, register: Register) -> Result<PointerValue<'ctx>, CompileError> {
        let index = u64::try_from(self.layout.word_index(register)?).map_err(|_| {
            CompileError::InvalidFunction("state layout exceeds u64 indices".to_string())
        })?;
        let index = self.context.i64_type().const_int(index, false);
        // SAFETY: [Categories 8, 10, and 13 — generated state-buffer GEP]
        // RegionLayout produced this word index, and CompiledRegion always passes
        // a buffer with at least RegionLayout::word_count initialized u64 words.
        unsafe {
            self.builder
                .build_gep(
                    self.context.i64_type(),
                    self.pointer,
                    &[index],
                    "state_slot",
                )
                .map_err(llvm_error)
        }
    }
}

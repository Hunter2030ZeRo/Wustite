use std::collections::BTreeMap;

use inkwell::context::Context;
use inkwell::values::{BasicValueEnum, PointerValue};

use super::super::{
    EXIT_ID_OFFSET, EXIT_KIND_OFFSET, NativeError, OUTPUTS_OFFSET, SLOT_PAYLOAD_OFFSET,
    byte_pointer, llvm_error, load_pointer, slot_offset, store_i32, value_tag,
};
use super::values::value;
use crate::adaptive_v2::wxir_v2::ir::{BlockId, Terminator, ValueId, ValueType};

pub(super) struct TerminatorLowering<'a, 'ctx> {
    pub(super) context: &'ctx Context,
    pub(super) builder: &'a inkwell::builder::Builder<'ctx>,
    pub(super) frame: PointerValue<'ctx>,
    pub(super) blocks: &'a BTreeMap<BlockId, inkwell::basic_block::BasicBlock<'ctx>>,
    pub(super) values: &'a BTreeMap<ValueId, BasicValueEnum<'ctx>>,
    pub(super) types: &'a BTreeMap<ValueId, ValueType>,
}

impl TerminatorLowering<'_, '_> {
    pub(super) fn lower(&self, terminator: &Terminator) -> Result<(), NativeError> {
        match terminator {
            Terminator::Jump { target, arguments } if arguments.is_empty() => {
                self.builder
                    .build_unconditional_branch(self.blocks[target])
                    .map_err(llvm_error)?;
            }
            Terminator::Branch { condition, yes, no } => {
                self.builder
                    .build_conditional_branch(
                        value(self.values, *condition)?.into_int_value(),
                        self.blocks[yes],
                        self.blocks[no],
                    )
                    .map_err(llvm_error)?;
            }
            Terminator::Return { values } => {
                self.store_outputs(values)?;
                self.builder
                    .build_return(Some(&self.context.i32_type().const_zero()))
                    .map_err(llvm_error)?;
            }
            Terminator::SideExit { id, .. } => {
                store_i32(
                    self.context,
                    self.builder,
                    self.frame,
                    EXIT_KIND_OFFSET,
                    1,
                    "exit_kind",
                )?;
                store_i32(
                    self.context,
                    self.builder,
                    self.frame,
                    EXIT_ID_OFFSET,
                    *id,
                    "exit_id",
                )?;
                self.builder
                    .build_return(Some(&self.context.i32_type().const_int(1, false)))
                    .map_err(llvm_error)?;
            }
            Terminator::Jump { .. }
            | Terminator::Backedge { .. }
            | Terminator::IrreducibleBackedge => {
                return Err(NativeError::Unsupported("scalar LLVM terminator"));
            }
        }
        Ok(())
    }

    fn store_outputs(&self, returned: &[ValueId]) -> Result<(), NativeError> {
        let outputs = load_pointer(
            self.context,
            self.builder,
            self.frame,
            OUTPUTS_OFFSET,
            "outputs",
        )?;
        for (index, id) in returned.iter().enumerate() {
            let offset = slot_offset(index)?;
            let ty = *self
                .types
                .get(id)
                .ok_or(NativeError::Unsupported("missing scalar type"))?;
            let tag = byte_pointer(
                self.context,
                self.builder,
                outputs,
                offset - SLOT_PAYLOAD_OFFSET,
                "tag",
            )?;
            self.builder
                .build_store(
                    tag,
                    self.context
                        .i32_type()
                        .const_int(u64::from(value_tag(ty)), false),
                )
                .map_err(llvm_error)?;
            let payload = byte_pointer(self.context, self.builder, outputs, offset, "payload")?;
            self.builder
                .build_store(payload, value(self.values, *id)?)
                .map_err(llvm_error)?;
        }
        Ok(())
    }
}

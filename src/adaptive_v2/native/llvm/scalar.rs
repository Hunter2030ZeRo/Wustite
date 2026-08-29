use std::collections::BTreeMap;

use inkwell::AddressSpace;
use inkwell::context::Context;
use inkwell::values::{BasicValueEnum, PointerValue};

use super::{
    MACHINE_ENTRIES_OFFSET, NativeError, VerifiedSnapshot, byte_pointer, finalize_cfg_module,
    llvm_error, load_pointer, slot_offset,
};
use crate::adaptive_v2::wxir_v2::ir::{ValueId, ValueType};

mod instruction;
mod terminator;
mod values;
use instruction::InstructionLowering;
use terminator::TerminatorLowering;
use values::native_type;

pub(super) fn compile(
    context: &'static Context,
    snapshot: &VerifiedSnapshot,
    symbol: &str,
) -> Result<
    inkwell::execution_engine::JitFunction<'static, super::super::entry::NativeEntry>,
    NativeError,
> {
    let module = context.create_module(symbol);
    let builder = context.create_builder();
    let function_type = context
        .i32_type()
        .fn_type(&[context.ptr_type(AddressSpace::default()).into()], false);
    let function = module.add_function(symbol, function_type, None);
    let prologue = context.append_basic_block(function, "prologue");
    let blocks = snapshot
        .body()
        .blocks
        .iter()
        .map(|block| {
            (
                block.id,
                context.append_basic_block(function, &format!("block_{}", block.id.get())),
            )
        })
        .collect::<BTreeMap<_, _>>();
    builder.position_at_end(prologue);
    let frame = function
        .get_first_param()
        .and_then(|value| match value {
            BasicValueEnum::PointerValue(pointer) => Some(pointer),
            _ => None,
        })
        .ok_or_else(|| NativeError::Backend("missing native frame".into()))?;
    count_entry(context, &builder, frame)?;
    let inputs = load_pointer(context, &builder, frame, super::INPUTS_OFFSET, "inputs")?;
    let entry = snapshot
        .body()
        .blocks
        .iter()
        .find(|block| block.id == snapshot.body().entry)
        .ok_or(NativeError::Unsupported("missing scalar entry"))?;
    if snapshot
        .body()
        .blocks
        .iter()
        .any(|block| block.id != entry.id && !block.parameters.is_empty())
    {
        return Err(NativeError::Unsupported("scalar LLVM phi"));
    }
    let mut values = BTreeMap::new();
    let mut types = BTreeMap::new();
    for (index, parameter) in entry.parameters.iter().enumerate() {
        let pointer = byte_pointer(context, &builder, inputs, slot_offset(index)?, "input")?;
        let loaded = builder
            .build_load(native_type(context, parameter.ty)?, pointer, "input_value")
            .map_err(llvm_error)?;
        values.insert(parameter.id, loaded);
        types.insert(parameter.id, parameter.ty);
    }
    builder
        .build_unconditional_branch(blocks[&entry.id])
        .map_err(llvm_error)?;
    for block in &snapshot.body().blocks {
        builder.position_at_end(blocks[&block.id]);
        for instruction in &block.instructions {
            let result = InstructionLowering {
                context,
                builder: &builder,
                function,
                module: &module,
                frame,
                snapshot,
                values: &values,
            }
            .lower(instruction.kind.semantic(), &instruction.inputs)?;
            if let (Some(output), Some(result)) = (instruction.output, result) {
                values.insert(output.id, result);
                types.insert(output.id, output.ty);
            }
        }
        TerminatorLowering {
            context,
            builder: &builder,
            frame,
            blocks: &blocks,
            values: &values,
            types: &types,
        }
        .lower(&block.terminator)?;
    }
    finalize_cfg_module(context, &module, symbol)
}

fn count_entry<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    frame: PointerValue<'ctx>,
) -> Result<(), NativeError> {
    let pointer = byte_pointer(context, builder, frame, MACHINE_ENTRIES_OFFSET, "entries")?;
    let current = builder
        .build_load(context.i64_type(), pointer, "entry_count")
        .map_err(llvm_error)?
        .into_int_value();
    let next = builder
        .build_int_add(
            current,
            context.i64_type().const_int(1, false),
            "next_entry",
        )
        .map_err(llvm_error)?;
    builder.build_store(pointer, next).map_err(llvm_error)?;
    Ok(())
}

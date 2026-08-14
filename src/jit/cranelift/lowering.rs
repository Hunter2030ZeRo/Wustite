use std::collections::HashMap;

use cranelift_codegen::ir::{InstBuilder, MemFlagsData, types};
use cranelift_frontend::FunctionBuilder;

use crate::wxir::{WxFunction, WxTerminator};

use super::CompileError;
use super::RegionLayout;
use super::helpers::{block_for, clif_type, exit_block_for, lower_values, offset_i32, value_for};
use super::instructions::lower_instruction;

pub(super) fn lower_function(
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
            lower_instruction(builder, function, &exit_blocks, &mut values, instruction)?;
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

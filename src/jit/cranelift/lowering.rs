use std::collections::HashMap;

use cranelift_codegen::ir::{InstBuilder, MemFlagsData, types};
use cranelift_frontend::FunctionBuilder;

use crate::bytecode::Register;
use crate::object::SequenceStrategy;
use crate::wxir::{WxFunction, WxInstKind, WxTerminator, WxValueId};

use super::CompileError;
use super::RegionLayout;
use super::helpers::{block_for, clif_type, exit_block_for, lower_values, offset_i32, value_for};
use super::instructions::{NativeRuntime, lower_instruction};
use super::native_calls::RuntimeFunctions;

pub(super) fn lower_function(
    builder: &mut FunctionBuilder<'_>,
    function: &WxFunction,
    layout: &RegionLayout,
    runtime_functions: &RuntimeFunctions,
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
    let runtime_context = builder
        .block_params(prologue)
        .get(1)
        .copied()
        .ok_or_else(|| {
            CompileError::InvalidFunction("missing native runtime context".to_string())
        })?;
    let runtime_error_block = builder.create_block();

    let entry_block = function
        .blocks
        .iter()
        .find(|block| block.id == function.entry)
        .ok_or_else(|| CompileError::InvalidFunction("missing WXIR entry block".to_string()))?;
    let mut sequence_views = HashMap::new();
    for (value, register, strategy) in direct_sequence_candidates(function) {
        let view = runtime_functions.lower_sequence_view(
            builder,
            runtime_context,
            runtime_error_block,
            register,
            strategy,
        )?;
        sequence_views.insert(value, view);
    }
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
            lower_instruction(
                builder,
                function,
                &exit_blocks,
                &NativeRuntime {
                    functions: runtime_functions,
                    context: runtime_context,
                    error_block: runtime_error_block,
                    sequence_views: &sequence_views,
                },
                &mut values,
                instruction,
            )?;
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

    builder.switch_to_block(runtime_error_block);
    let error_exit = builder.ins().iconst(types::I32, i64::from(u32::MAX));
    builder.ins().return_(&[error_exit]);

    Ok(())
}

fn direct_sequence_candidates(
    function: &WxFunction,
) -> Vec<(WxValueId, Register, SequenceStrategy)> {
    let instructions = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();
    if instructions
        .iter()
        .any(|instruction| match &instruction.kind {
            WxInstKind::RuntimeCall { effects, .. } => {
                effects.may_mutate || effects.may_call_unknown || effects.may_access_global_state
            }
            WxInstKind::Call { .. }
            | WxInstKind::SequenceMutate { .. }
            | WxInstKind::MaterializeSequence { .. } => true,
            _ => false,
        })
    {
        return Vec::new();
    }
    let mut result = Vec::new();
    for instruction in &instructions {
        let (object, inputs, strategy) = match &instruction.kind {
            WxInstKind::SequenceLength {
                object,
                inputs,
                strategy: Some(strategy),
                ..
            }
            | WxInstKind::SequenceGet {
                object,
                inputs,
                strategy: Some(strategy),
                ..
            }
            | WxInstKind::SequenceSet {
                object,
                inputs,
                strategy: Some(strategy),
                ..
            } if matches!(
                strategy,
                SequenceStrategy::Bool | SequenceStrategy::I64 | SequenceStrategy::F64
            ) =>
            {
                (*object, inputs, *strategy)
            }
            _ => continue,
        };
        let Some(value) = inputs
            .iter()
            .find(|input| input.register == object)
            .map(|input| input.value)
        else {
            continue;
        };
        if !function
            .entry_state
            .iter()
            .any(|state| state.register == object)
        {
            continue;
        }
        if !result.iter().any(|(candidate, _, _)| *candidate == value) {
            result.push((value, object, strategy));
        }
    }
    result
}

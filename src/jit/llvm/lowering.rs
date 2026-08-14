use std::collections::HashMap;

use inkwell::AddressSpace;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue, PhiValue};

use crate::wxir::{WxBlockId, WxExitId, WxFunction, WxTerminator, WxValueId};

use super::helpers::{
    add_incoming, block_for, exit_block_for, int_value_for, llvm_type, lower_values,
};
use super::instructions::InstructionContext;
use super::state_buffer::StateBuffer;
use super::{CompileError, RegionLayout, llvm_error};

pub(super) fn lower_function<'ctx>(
    context: &'ctx Context,
    function: &WxFunction,
    layout: &RegionLayout,
    symbol: &str,
) -> Result<Module<'ctx>, CompileError> {
    let module = context.create_module(symbol);
    let builder = context.create_builder();
    let pointer_type = context.ptr_type(AddressSpace::default());
    let function_type = context.i32_type().fn_type(&[pointer_type.into()], false);
    let llvm_function = module.add_function(symbol, function_type, None);
    let prologue = context.append_basic_block(llvm_function, "prologue");

    let (blocks, block_phis, mut values) =
        create_blocks(context, &builder, llvm_function, function)?;
    let (exit_blocks, exit_phis) = create_exit_blocks(context, &builder, llvm_function, function)?;
    builder.position_at_end(prologue);
    let state_pointer = match llvm_function.get_first_param() {
        Some(BasicValueEnum::PointerValue(pointer)) => pointer,
        _ => {
            return Err(CompileError::InvalidFunction(
                "missing native state pointer".to_string(),
            ));
        }
    };
    let state_buffer = StateBuffer {
        builder: &builder,
        context,
        pointer: state_pointer,
        layout,
    };
    let entry_block = function
        .blocks
        .iter()
        .find(|block| block.id == function.entry)
        .ok_or_else(|| CompileError::InvalidFunction("missing WXIR entry block".to_string()))?;
    let entry_values = entry_block
        .parameters
        .iter()
        .map(|parameter| {
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
            state_buffer.load(state.register, state.ty)
        })
        .collect::<Result<Vec<_>, _>>()?;
    add_incoming(
        block_phis
            .get(&function.entry)
            .ok_or_else(|| CompileError::InvalidFunction("missing entry phis".to_string()))?,
        &entry_values,
        prologue,
    )?;
    builder
        .build_unconditional_branch(block_for(&blocks, function.entry)?)
        .map_err(llvm_error)?;

    for block in &function.blocks {
        builder.position_at_end(block_for(&blocks, block.id)?);
        for instruction in &block.instructions {
            InstructionContext {
                builder: &builder,
                module: &module,
                function,
                exit_blocks: &exit_blocks,
                exit_phis: &exit_phis,
                values: &mut values,
            }
            .lower(instruction)?;
        }
        lower_terminator(
            &builder,
            &blocks,
            &block_phis,
            &exit_blocks,
            &exit_phis,
            &values,
            &block.terminator,
        )?;
    }

    for exit in &function.side_exits {
        builder.position_at_end(exit_block_for(&exit_blocks, exit.id)?);
        let phis = exit_phis.get(&exit.id).ok_or_else(|| {
            CompileError::InvalidFunction(format!("missing exit phis {}", exit.id))
        })?;
        for (state, phi) in exit.state.iter().zip(phis) {
            state_buffer.store(state.register, state.ty, phi.as_basic_value())?;
        }
        builder
            .build_return(Some(
                &context.i32_type().const_int(u64::from(exit.id.0), false),
            ))
            .map_err(llvm_error)?;
    }

    Ok(module)
}

type BlockMaps<'ctx> = (
    HashMap<WxBlockId, BasicBlock<'ctx>>,
    HashMap<WxBlockId, Vec<PhiValue<'ctx>>>,
    HashMap<WxValueId, BasicValueEnum<'ctx>>,
);

fn create_blocks<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    llvm_function: FunctionValue<'ctx>,
    function: &WxFunction,
) -> Result<BlockMaps<'ctx>, CompileError> {
    let mut blocks = HashMap::new();
    let mut block_phis = HashMap::new();
    let mut values = HashMap::new();
    for block in &function.blocks {
        let llvm_block = context.append_basic_block(llvm_function, &format!("b{}", block.id.0));
        blocks.insert(block.id, llvm_block);
        builder.position_at_end(llvm_block);
        let mut phis = Vec::with_capacity(block.parameters.len());
        for parameter in &block.parameters {
            let phi = builder
                .build_phi(llvm_type(context, parameter.ty)?, "param")
                .map_err(llvm_error)?;
            values.insert(parameter.id, phi.as_basic_value());
            phis.push(phi);
        }
        block_phis.insert(block.id, phis);
    }
    Ok((blocks, block_phis, values))
}

type ExitMaps<'ctx> = (
    HashMap<WxExitId, BasicBlock<'ctx>>,
    HashMap<WxExitId, Vec<PhiValue<'ctx>>>,
);

fn create_exit_blocks<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    llvm_function: FunctionValue<'ctx>,
    function: &WxFunction,
) -> Result<ExitMaps<'ctx>, CompileError> {
    let mut blocks = HashMap::new();
    let mut phis_by_exit = HashMap::new();
    for exit in &function.side_exits {
        let block = context.append_basic_block(llvm_function, &format!("x{}", exit.id.0));
        blocks.insert(exit.id, block);
        builder.position_at_end(block);
        let mut phis = Vec::with_capacity(exit.state.len());
        for state in &exit.state {
            phis.push(
                builder
                    .build_phi(llvm_type(context, state.ty)?, "exit_value")
                    .map_err(llvm_error)?,
            );
        }
        phis_by_exit.insert(exit.id, phis);
    }
    Ok((blocks, phis_by_exit))
}

fn lower_terminator<'ctx>(
    builder: &Builder<'ctx>,
    blocks: &HashMap<WxBlockId, BasicBlock<'ctx>>,
    block_phis: &HashMap<WxBlockId, Vec<PhiValue<'ctx>>>,
    exit_blocks: &HashMap<WxExitId, BasicBlock<'ctx>>,
    exit_phis: &HashMap<WxExitId, Vec<PhiValue<'ctx>>>,
    values: &HashMap<WxValueId, BasicValueEnum<'ctx>>,
    terminator: &WxTerminator,
) -> Result<(), CompileError> {
    let predecessor = builder
        .get_insert_block()
        .ok_or_else(|| CompileError::Backend("LLVM builder has no current block".to_string()))?;
    match terminator {
        WxTerminator::Jump { target, arguments } => {
            let incoming = lower_values(values, arguments)?;
            add_incoming(
                block_phis.get(target).ok_or_else(|| {
                    CompileError::InvalidFunction(format!("missing block phis {target}"))
                })?,
                &incoming,
                predecessor,
            )?;
            builder
                .build_unconditional_branch(block_for(blocks, *target)?)
                .map_err(llvm_error)?;
        }
        WxTerminator::Branch { condition, yes, no } => {
            let yes_values = lower_values(values, &yes.arguments)?;
            let no_values = lower_values(values, &no.arguments)?;
            add_incoming(
                block_phis.get(&yes.block).ok_or_else(|| {
                    CompileError::InvalidFunction(format!("missing block phis {}", yes.block))
                })?,
                &yes_values,
                predecessor,
            )?;
            add_incoming(
                block_phis.get(&no.block).ok_or_else(|| {
                    CompileError::InvalidFunction(format!("missing block phis {}", no.block))
                })?,
                &no_values,
                predecessor,
            )?;
            builder
                .build_conditional_branch(
                    int_value_for(values, *condition)?,
                    block_for(blocks, yes.block)?,
                    block_for(blocks, no.block)?,
                )
                .map_err(llvm_error)?;
        }
        WxTerminator::SideExit {
            exit,
            values: state,
        } => {
            let incoming = lower_values(values, state)?;
            add_incoming(
                exit_phis.get(exit).ok_or_else(|| {
                    CompileError::InvalidFunction(format!("missing exit phis {exit}"))
                })?,
                &incoming,
                predecessor,
            )?;
            builder
                .build_unconditional_branch(exit_block_for(exit_blocks, *exit)?)
                .map_err(llvm_error)?;
        }
        WxTerminator::Return { .. } => {
            return Err(CompileError::UnsupportedInstruction("Return"));
        }
    }
    Ok(())
}

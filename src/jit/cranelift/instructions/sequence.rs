use std::collections::HashMap;

use cranelift_codegen::ir::{InstBuilder, MemFlagsData, Value, condcodes::IntCC};
use cranelift_frontend::FunctionBuilder;

use crate::bytecode::Register;
use crate::wxir::{WxInst, WxRuntimeInput, WxScalarType, WxType, WxValueId};

use super::super::helpers::one_result;
use super::super::native_calls::{RuntimeCall, RuntimeEnvironment, SequenceViewValues};
use super::{CompileError, NativeRuntime};

mod access;

#[allow(
    clippy::too_many_arguments,
    reason = "lowering receives the sequence instruction fields alongside compiler state"
)]
pub(super) fn lower_length(
    builder: &mut FunctionBuilder<'_>,
    runtime: &NativeRuntime<'_>,
    values: &mut HashMap<WxValueId, Value>,
    instruction: &WxInst,
    pc: u32,
    object: Register,
    inputs: &[WxRuntimeInput],
    output: Register,
) -> Result<(), CompileError> {
    let result = one_result(instruction)?;
    if let Some(view) = view_for(runtime, inputs, object)
        && result.ty == WxType::Scalar(WxScalarType::I64)
    {
        values.insert(result.id, view.len);
        return Ok(());
    }
    lower_fallback(
        builder,
        runtime,
        values,
        instruction,
        pc,
        inputs,
        Some(output),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "lowering receives the sequence instruction fields alongside compiler state"
)]
pub(super) fn lower_get(
    builder: &mut FunctionBuilder<'_>,
    runtime: &NativeRuntime<'_>,
    values: &mut HashMap<WxValueId, Value>,
    instruction: &WxInst,
    pc: u32,
    object: Register,
    inputs: &[WxRuntimeInput],
    output: Register,
) -> Result<(), CompileError> {
    let result = one_result(instruction)?;
    let Some(view) = view_for(runtime, inputs, object) else {
        return lower_fallback(
            builder,
            runtime,
            values,
            instruction,
            pc,
            inputs,
            Some(output),
        );
    };
    let Some(index) = access::scalar_input(values, inputs, object, WxScalarType::I64) else {
        return lower_fallback(
            builder,
            runtime,
            values,
            instruction,
            pc,
            inputs,
            Some(output),
        );
    };
    let Some(load_type) = access::strategy_type(view.strategy, result.ty) else {
        return lower_fallback(
            builder,
            runtime,
            values,
            instruction,
            pc,
            inputs,
            Some(output),
        );
    };
    let normalized = access::normalized_index(builder, index, view.len);
    let invalid = access::invalid_index(builder, normalized, view.len);
    let slow = builder.create_block();
    let direct = builder.create_block();
    let continuation = builder.create_block();
    builder.append_block_param(continuation, load_type);
    builder.ins().brif(invalid, slow, &[], direct, &[]);

    builder.switch_to_block(direct);
    let address = access::element_address(builder, view.data, normalized, view.strategy);
    let loaded = builder
        .ins()
        .load(load_type, MemFlagsData::new(), address, 0);
    builder.ins().jump(continuation, &[loaded.into()]);

    builder.switch_to_block(slow);
    lower_fallback(
        builder,
        runtime,
        values,
        instruction,
        pc,
        inputs,
        Some(output),
    )?;
    let fallback = values
        .get(&result.id)
        .copied()
        .ok_or_else(|| CompileError::InvalidFunction("missing sequence fallback result".into()))?;
    builder.ins().jump(continuation, &[fallback.into()]);
    builder.switch_to_block(continuation);
    values.insert(result.id, builder.block_params(continuation)[0]);
    Ok(())
}

pub(super) fn lower_set(
    builder: &mut FunctionBuilder<'_>,
    runtime: &NativeRuntime<'_>,
    values: &mut HashMap<WxValueId, Value>,
    instruction: &WxInst,
    pc: u32,
    object: Register,
    inputs: &[WxRuntimeInput],
) -> Result<(), CompileError> {
    let Some(view) = view_for(runtime, inputs, object) else {
        return lower_fallback(builder, runtime, values, instruction, pc, inputs, None);
    };
    let Some(index) = access::scalar_input(values, inputs, object, WxScalarType::I64) else {
        return lower_fallback(builder, runtime, values, instruction, pc, inputs, None);
    };
    let Some(value) = access::stored_value(values, inputs, object, view.strategy) else {
        return lower_fallback(builder, runtime, values, instruction, pc, inputs, None);
    };
    let normalized = access::normalized_index(builder, index, view.len);
    let bounds = access::invalid_index(builder, normalized, view.len);
    let read_only = builder.ins().icmp_imm_s(IntCC::Equal, view.writable, 0);
    let invalid = builder.ins().bor(bounds, read_only);
    let slow = builder.create_block();
    let direct = builder.create_block();
    let continuation = builder.create_block();
    builder.ins().brif(invalid, slow, &[], direct, &[]);

    builder.switch_to_block(direct);
    let address = access::element_address(builder, view.data, normalized, view.strategy);
    builder.ins().store(MemFlagsData::new(), value, address, 0);
    builder.ins().jump(continuation, &[]);

    builder.switch_to_block(slow);
    lower_fallback(builder, runtime, values, instruction, pc, inputs, None)?;
    builder.ins().jump(continuation, &[]);
    builder.switch_to_block(continuation);
    Ok(())
}

fn lower_fallback(
    builder: &mut FunctionBuilder<'_>,
    runtime: &NativeRuntime<'_>,
    values: &mut HashMap<WxValueId, Value>,
    instruction: &WxInst,
    pc: u32,
    inputs: &[WxRuntimeInput],
    output: Option<Register>,
) -> Result<(), CompileError> {
    runtime.functions.lower_call(
        builder,
        &mut RuntimeEnvironment {
            context: runtime.context,
            error_block: runtime.error_block,
            values,
        },
        RuntimeCall {
            instruction,
            pc,
            inputs,
            output,
            sequence: true,
        },
    )
}

fn view_for<'a>(
    runtime: &'a NativeRuntime<'_>,
    inputs: &[WxRuntimeInput],
    object: Register,
) -> Option<&'a SequenceViewValues> {
    let value = inputs.iter().find(|input| input.register == object)?.value;
    runtime.sequence_views.get(&value)
}

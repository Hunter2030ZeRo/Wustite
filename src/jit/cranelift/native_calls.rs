use std::collections::HashMap;

use cranelift_codegen::ir::{
    AbiParam, Block, FuncRef, Function, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind,
    Value, types,
};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{Linkage, Module};

use crate::bytecode::Register;
use crate::object::SequenceStrategy;
use crate::wxir::{WxInst, WxRuntimeInput, WxScalarType, WxType, WxValueId};

use super::helpers::one_result;
use super::{CompileError, JITModule};

pub(super) struct RuntimeFunctions {
    execute: FuncRef,
    execute_one: FuncRef,
    execute_one_i64: FuncRef,
    execute_two: FuncRef,
    execute_sequence: FuncRef,
    execute_sequence_one: FuncRef,
    execute_sequence_one_i64: FuncRef,
    execute_sequence_one_f64: FuncRef,
    execute_sequence_two: FuncRef,
    sync_bool: FuncRef,
    sync_i64: FuncRef,
    sync_f64: FuncRef,
    read_bool: FuncRef,
    read_i64: FuncRef,
    read_f64: FuncRef,
    read_ptr: FuncRef,
    sequence_view: FuncRef,
}

impl RuntimeFunctions {
    pub(super) fn declare(
        module: &mut JITModule,
        function: &mut Function,
    ) -> Result<Self, CompileError> {
        let pointer = module.target_config().pointer_type();
        Ok(Self {
            execute: declare(
                module,
                function,
                super::super::runtime::EXECUTE_SYMBOL,
                &[pointer, types::I32],
                Some(types::I8),
            )?,
            execute_one: declare(
                module,
                function,
                super::super::runtime::EXECUTE_ONE_SYMBOL,
                &[pointer, types::I32, types::I32, types::I8, types::I64],
                Some(types::I8),
            )?,
            execute_one_i64: declare(
                module,
                function,
                super::super::runtime::EXECUTE_ONE_I64_SYMBOL,
                &[
                    pointer,
                    types::I32,
                    types::I32,
                    types::I8,
                    types::I64,
                    types::I32,
                ],
                Some(types::I64),
            )?,
            execute_two: declare(
                module,
                function,
                super::super::runtime::EXECUTE_TWO_SYMBOL,
                &[
                    pointer,
                    types::I32,
                    types::I32,
                    types::I8,
                    types::I64,
                    types::I32,
                    types::I8,
                    types::I64,
                ],
                Some(types::I8),
            )?,
            execute_sequence: declare(
                module,
                function,
                super::super::runtime::EXECUTE_SEQUENCE_SYMBOL,
                &[pointer, types::I32],
                Some(types::I8),
            )?,
            execute_sequence_one: declare(
                module,
                function,
                super::super::runtime::EXECUTE_SEQUENCE_ONE_SYMBOL,
                &[pointer, types::I32, types::I32, types::I8, types::I64],
                Some(types::I8),
            )?,
            execute_sequence_one_i64: declare(
                module,
                function,
                super::super::runtime::EXECUTE_SEQUENCE_ONE_I64_SYMBOL,
                &[
                    pointer,
                    types::I32,
                    types::I32,
                    types::I8,
                    types::I64,
                    types::I32,
                ],
                Some(types::I64),
            )?,
            execute_sequence_one_f64: declare(
                module,
                function,
                super::super::runtime::EXECUTE_SEQUENCE_ONE_F64_SYMBOL,
                &[
                    pointer,
                    types::I32,
                    types::I32,
                    types::I8,
                    types::I64,
                    types::I32,
                    pointer,
                ],
                Some(types::I8),
            )?,
            execute_sequence_two: declare(
                module,
                function,
                super::super::runtime::EXECUTE_SEQUENCE_TWO_SYMBOL,
                &[
                    pointer,
                    types::I32,
                    types::I32,
                    types::I8,
                    types::I64,
                    types::I32,
                    types::I8,
                    types::I64,
                ],
                Some(types::I8),
            )?,
            sync_bool: declare(
                module,
                function,
                super::super::runtime::SYNC_BOOL_SYMBOL,
                &[pointer, types::I32, types::I8],
                None,
            )?,
            sync_i64: declare(
                module,
                function,
                super::super::runtime::SYNC_I64_SYMBOL,
                &[pointer, types::I32, types::I64],
                None,
            )?,
            sync_f64: declare(
                module,
                function,
                super::super::runtime::SYNC_F64_SYMBOL,
                &[pointer, types::I32, types::F64],
                None,
            )?,
            read_bool: declare(
                module,
                function,
                super::super::runtime::READ_BOOL_SYMBOL,
                &[pointer, types::I32],
                Some(types::I8),
            )?,
            read_i64: declare(
                module,
                function,
                super::super::runtime::READ_I64_SYMBOL,
                &[pointer, types::I32],
                Some(types::I64),
            )?,
            read_f64: declare(
                module,
                function,
                super::super::runtime::READ_F64_SYMBOL,
                &[pointer, types::I32],
                Some(types::F64),
            )?,
            read_ptr: declare(
                module,
                function,
                super::super::runtime::READ_PTR_SYMBOL,
                &[pointer, types::I32],
                Some(types::I64),
            )?,
            sequence_view: declare(
                module,
                function,
                super::super::runtime::SEQUENCE_VIEW_SYMBOL,
                &[pointer, types::I32, types::I8, pointer],
                Some(types::I8),
            )?,
        })
    }

    pub(super) fn lower_call(
        &self,
        builder: &mut FunctionBuilder<'_>,
        environment: &mut RuntimeEnvironment<'_>,
        call: RuntimeCall<'_>,
    ) -> Result<(), CompileError> {
        let mut fused = Vec::new();
        for input in call.inputs {
            let register = builder.ins().iconst(types::I32, i64::from(input.register));
            let value = environment
                .values
                .get(&input.value)
                .copied()
                .ok_or_else(|| {
                    CompileError::InvalidFunction(format!("missing value {}", input.value))
                })?;
            let (function, tag, bits) = match input.ty {
                WxType::Scalar(WxScalarType::I1) => (
                    self.sync_bool,
                    builder.ins().iconst(types::I8, 0),
                    builder.ins().uextend(types::I64, value),
                ),
                WxType::Scalar(WxScalarType::I64) => {
                    (self.sync_i64, builder.ins().iconst(types::I8, 1), value)
                }
                WxType::Scalar(WxScalarType::F64) => (
                    self.sync_f64,
                    builder.ins().iconst(types::I8, 2),
                    builder
                        .ins()
                        .bitcast(types::I64, MemFlagsData::new(), value),
                ),
                WxType::Scalar(WxScalarType::RuntimeHandle) => continue,
                ty => return Err(CompileError::UnsupportedType(ty)),
            };
            fused.push((register, tag, bits, function, value));
        }

        let pc = builder.ins().iconst(types::I32, i64::from(call.pc));
        let execute = if call.sequence {
            self.execute_sequence
        } else {
            self.execute
        };
        let execute_one = if call.sequence {
            self.execute_sequence_one
        } else {
            self.execute_one
        };
        let execute_one_i64 = if call.sequence {
            self.execute_sequence_one_i64
        } else {
            self.execute_one_i64
        };
        let execute_two = if call.sequence {
            self.execute_sequence_two
        } else {
            self.execute_two
        };
        let mut packed_result = None;
        let mut direct_f64_result = None;
        let status = match (fused.as_slice(), call.output) {
            ([], _) => {
                let execution = builder.ins().call(execute, &[environment.context, pc]);
                first_call_result(builder, execution)?
            }
            ([(register, tag, bits, _, _)], Some(output))
                if one_result(call.instruction)?.ty == WxType::Scalar(WxScalarType::I64) =>
            {
                let output = builder.ins().iconst(types::I32, i64::from(output));
                let execution = builder.ins().call(
                    execute_one_i64,
                    &[environment.context, pc, *register, *tag, *bits, output],
                );
                let packed = first_call_result(builder, execution)?;
                let status = builder.ins().band_imm_u(packed, 1);
                packed_result = Some((one_result(call.instruction)?.id, packed));
                status
            }
            ([(register, tag, bits, _, _)], Some(output))
                if call.sequence
                    && one_result(call.instruction)?.ty == WxType::Scalar(WxScalarType::F64) =>
            {
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let pointer_type = builder.func.dfg.value_type(environment.context);
                let output_pointer = builder.ins().stack_addr(pointer_type, slot, 0);
                let output = builder.ins().iconst(types::I32, i64::from(output));
                let execution = builder.ins().call(
                    self.execute_sequence_one_f64,
                    &[
                        environment.context,
                        pc,
                        *register,
                        *tag,
                        *bits,
                        output,
                        output_pointer,
                    ],
                );
                direct_f64_result = Some((one_result(call.instruction)?.id, slot));
                first_call_result(builder, execution)?
            }
            ([(register, tag, bits, _, _)], _) => {
                let execution = builder.ins().call(
                    execute_one,
                    &[environment.context, pc, *register, *tag, *bits],
                );
                first_call_result(builder, execution)?
            }
            (
                [
                    (register0, tag0, bits0, _, _),
                    (register1, tag1, bits1, _, _),
                ],
                _,
            ) => {
                let execution = builder.ins().call(
                    execute_two,
                    &[
                        environment.context,
                        pc,
                        *register0,
                        *tag0,
                        *bits0,
                        *register1,
                        *tag1,
                        *bits1,
                    ],
                );
                first_call_result(builder, execution)?
            }
            (inputs, _) => {
                for (register, _, _, function, value) in inputs {
                    builder
                        .ins()
                        .call(*function, &[environment.context, *register, *value]);
                }
                let execution = builder.ins().call(execute, &[environment.context, pc]);
                first_call_result(builder, execution)?
            }
        };
        let continuation = builder.create_block();
        builder
            .ins()
            .brif(status, continuation, &[], environment.error_block, &[]);
        builder.switch_to_block(continuation);

        if let Some((id, packed)) = packed_result {
            environment
                .values
                .insert(id, builder.ins().sshr_imm_u(packed, 1));
            return Ok(());
        }
        if let Some((id, slot)) = direct_f64_result {
            let pointer_type = builder.func.dfg.value_type(environment.context);
            let value = builder.ins().stack_load(pointer_type, types::F64, slot, 0);
            environment.values.insert(id, value);
            return Ok(());
        }

        if let Some(register) = call.output {
            let result = one_result(call.instruction)?;
            let register_value = builder.ins().iconst(types::I32, i64::from(register));
            let function = match result.ty {
                WxType::Scalar(WxScalarType::I1) => self.read_bool,
                WxType::Scalar(WxScalarType::I64) => self.read_i64,
                WxType::Scalar(WxScalarType::F64) => self.read_f64,
                WxType::Scalar(WxScalarType::RuntimeHandle) => self.read_ptr,
                ty => return Err(CompileError::UnsupportedType(ty)),
            };
            let read = builder
                .ins()
                .call(function, &[environment.context, register_value]);
            let value = builder.inst_results(read).first().copied().ok_or_else(|| {
                CompileError::InvalidFunction("runtime read returned no value".to_string())
            })?;
            environment.values.insert(result.id, value);
        }
        Ok(())
    }

    pub(super) fn lower_sequence_view(
        &self,
        builder: &mut FunctionBuilder<'_>,
        context: Value,
        error_block: Block,
        register: Register,
        strategy: SequenceStrategy,
    ) -> Result<SequenceViewValues, CompileError> {
        let slot =
            builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 32, 3));
        let pointer_type = builder.func.dfg.value_type(context);
        let output = builder.ins().stack_addr(pointer_type, slot, 0);
        let register = builder.ins().iconst(types::I32, i64::from(register));
        let strategy_value = builder
            .ins()
            .iconst(types::I8, i64::from(sequence_strategy_code(strategy)));
        let call = builder.ins().call(
            self.sequence_view,
            &[context, register, strategy_value, output],
        );
        let status = first_call_result(builder, call)?;
        let continuation = builder.create_block();
        builder
            .ins()
            .brif(status, continuation, &[], error_block, &[]);
        builder.switch_to_block(continuation);
        Ok(SequenceViewValues {
            data: builder
                .ins()
                .stack_load(pointer_type, pointer_type, slot, 0),
            len: builder.ins().stack_load(pointer_type, types::I64, slot, 8),
            _layout_version: builder.ins().stack_load(pointer_type, types::I64, slot, 16),
            writable: builder.ins().stack_load(pointer_type, types::I64, slot, 24),
            strategy,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SequenceViewValues {
    pub(super) data: Value,
    pub(super) len: Value,
    pub(super) _layout_version: Value,
    pub(super) writable: Value,
    pub(super) strategy: SequenceStrategy,
}

pub(super) struct RuntimeEnvironment<'a> {
    pub(super) context: Value,
    pub(super) error_block: Block,
    pub(super) values: &'a mut HashMap<WxValueId, Value>,
}

pub(super) struct RuntimeCall<'a> {
    pub(super) instruction: &'a WxInst,
    pub(super) pc: u32,
    pub(super) inputs: &'a [WxRuntimeInput],
    pub(super) output: Option<Register>,
    pub(super) sequence: bool,
}

fn declare(
    module: &mut JITModule,
    function: &mut Function,
    name: &str,
    parameters: &[cranelift_codegen::ir::Type],
    result: Option<cranelift_codegen::ir::Type>,
) -> Result<FuncRef, CompileError> {
    let mut signature = module.make_signature();
    signature
        .params
        .extend(parameters.iter().copied().map(AbiParam::new));
    if let Some(result) = result {
        signature.returns.push(AbiParam::new(result));
    }
    let id = module
        .declare_function(name, Linkage::Import, &signature)
        .map_err(|error| CompileError::Backend(error.to_string()))?;
    Ok(module.declare_func_in_func(id, function))
}

fn first_call_result(
    builder: &FunctionBuilder<'_>,
    call: cranelift_codegen::ir::Inst,
) -> Result<Value, CompileError> {
    builder.inst_results(call).first().copied().ok_or_else(|| {
        CompileError::InvalidFunction("runtime execute returned no status".to_string())
    })
}

const fn sequence_strategy_code(strategy: SequenceStrategy) -> u8 {
    match strategy {
        SequenceStrategy::Empty => 0,
        SequenceStrategy::Bool => 1,
        SequenceStrategy::I64 => 2,
        SequenceStrategy::F64 => 3,
        SequenceStrategy::Object => 4,
    }
}

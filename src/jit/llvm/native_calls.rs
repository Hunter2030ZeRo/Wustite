use std::collections::HashMap;

use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};

use crate::bytecode::Register;
use crate::wxir::{WxInst, WxRuntimeInput, WxScalarType, WxType, WxValueId};

use super::helpers::one_result;
use super::{CompileError, llvm_error};

pub(super) struct RuntimeFunctions<'ctx> {
    execute: FunctionValue<'ctx>,
    execute_one: FunctionValue<'ctx>,
    execute_one_i64: FunctionValue<'ctx>,
    execute_two: FunctionValue<'ctx>,
    execute_sequence: FunctionValue<'ctx>,
    execute_sequence_one: FunctionValue<'ctx>,
    execute_sequence_one_i64: FunctionValue<'ctx>,
    execute_sequence_two: FunctionValue<'ctx>,
    sync_bool: FunctionValue<'ctx>,
    sync_i64: FunctionValue<'ctx>,
    sync_f64: FunctionValue<'ctx>,
    read_bool: FunctionValue<'ctx>,
    read_i64: FunctionValue<'ctx>,
    read_f64: FunctionValue<'ctx>,
    read_ptr: FunctionValue<'ctx>,
}

impl<'ctx> RuntimeFunctions<'ctx> {
    pub(super) fn declare(context: &'ctx Context, module: &Module<'ctx>) -> Self {
        let pointer = context.ptr_type(AddressSpace::default());
        let i8_type = context.i8_type();
        let i32_type = context.i32_type();
        let i64_type = context.i64_type();
        let f64_type = context.f64_type();
        Self {
            execute: module.add_function(
                super::super::runtime::EXECUTE_SYMBOL,
                i8_type.fn_type(&[pointer.into(), i32_type.into()], false),
                None,
            ),
            execute_one: module.add_function(
                super::super::runtime::EXECUTE_ONE_SYMBOL,
                i8_type.fn_type(
                    &[
                        pointer.into(),
                        i32_type.into(),
                        i32_type.into(),
                        i8_type.into(),
                        i64_type.into(),
                    ],
                    false,
                ),
                None,
            ),
            execute_one_i64: module.add_function(
                super::super::runtime::EXECUTE_ONE_I64_SYMBOL,
                i64_type.fn_type(
                    &[
                        pointer.into(),
                        i32_type.into(),
                        i32_type.into(),
                        i8_type.into(),
                        i64_type.into(),
                        i32_type.into(),
                    ],
                    false,
                ),
                None,
            ),
            execute_two: module.add_function(
                super::super::runtime::EXECUTE_TWO_SYMBOL,
                i8_type.fn_type(
                    &[
                        pointer.into(),
                        i32_type.into(),
                        i32_type.into(),
                        i8_type.into(),
                        i64_type.into(),
                        i32_type.into(),
                        i8_type.into(),
                        i64_type.into(),
                    ],
                    false,
                ),
                None,
            ),
            execute_sequence: module.add_function(
                super::super::runtime::EXECUTE_SEQUENCE_SYMBOL,
                i8_type.fn_type(&[pointer.into(), i32_type.into()], false),
                None,
            ),
            execute_sequence_one: module.add_function(
                super::super::runtime::EXECUTE_SEQUENCE_ONE_SYMBOL,
                i8_type.fn_type(
                    &[
                        pointer.into(),
                        i32_type.into(),
                        i32_type.into(),
                        i8_type.into(),
                        i64_type.into(),
                    ],
                    false,
                ),
                None,
            ),
            execute_sequence_one_i64: module.add_function(
                super::super::runtime::EXECUTE_SEQUENCE_ONE_I64_SYMBOL,
                i64_type.fn_type(
                    &[
                        pointer.into(),
                        i32_type.into(),
                        i32_type.into(),
                        i8_type.into(),
                        i64_type.into(),
                        i32_type.into(),
                    ],
                    false,
                ),
                None,
            ),
            execute_sequence_two: module.add_function(
                super::super::runtime::EXECUTE_SEQUENCE_TWO_SYMBOL,
                i8_type.fn_type(
                    &[
                        pointer.into(),
                        i32_type.into(),
                        i32_type.into(),
                        i8_type.into(),
                        i64_type.into(),
                        i32_type.into(),
                        i8_type.into(),
                        i64_type.into(),
                    ],
                    false,
                ),
                None,
            ),
            sync_bool: module.add_function(
                super::super::runtime::SYNC_BOOL_SYMBOL,
                context
                    .void_type()
                    .fn_type(&[pointer.into(), i32_type.into(), i8_type.into()], false),
                None,
            ),
            sync_i64: module.add_function(
                super::super::runtime::SYNC_I64_SYMBOL,
                context
                    .void_type()
                    .fn_type(&[pointer.into(), i32_type.into(), i64_type.into()], false),
                None,
            ),
            sync_f64: module.add_function(
                super::super::runtime::SYNC_F64_SYMBOL,
                context
                    .void_type()
                    .fn_type(&[pointer.into(), i32_type.into(), f64_type.into()], false),
                None,
            ),
            read_bool: module.add_function(
                super::super::runtime::READ_BOOL_SYMBOL,
                i8_type.fn_type(&[pointer.into(), i32_type.into()], false),
                None,
            ),
            read_i64: module.add_function(
                super::super::runtime::READ_I64_SYMBOL,
                i64_type.fn_type(&[pointer.into(), i32_type.into()], false),
                None,
            ),
            read_f64: module.add_function(
                super::super::runtime::READ_F64_SYMBOL,
                f64_type.fn_type(&[pointer.into(), i32_type.into()], false),
                None,
            ),
            read_ptr: module.add_function(
                super::super::runtime::READ_PTR_SYMBOL,
                i64_type.fn_type(&[pointer.into(), i32_type.into()], false),
                None,
            ),
        }
    }

    pub(super) fn lower_call(
        &self,
        builder: &Builder<'ctx>,
        environment: &mut RuntimeEnvironment<'_, 'ctx>,
        call: RuntimeCall<'_>,
    ) -> Result<(), CompileError> {
        let llvm_context = builder
            .get_insert_block()
            .map(BasicBlock::get_context)
            .ok_or_else(|| CompileError::Backend("LLVM runtime call has no block".to_string()))?;
        let mut fused = Vec::new();
        for input in call.inputs {
            let register = llvm_context
                .i32_type()
                .const_int(u64::from(input.register), false);
            let value = environment
                .values
                .get(&input.value)
                .copied()
                .ok_or_else(|| {
                    CompileError::InvalidFunction(format!("missing value {}", input.value))
                })?;
            let (function, sync_value, tag, bits) = match input.ty {
                WxType::Scalar(WxScalarType::I1) => {
                    let BasicValueEnum::IntValue(value) = value else {
                        return Err(CompileError::InvalidFunction(format!(
                            "boolean value {} is not an integer",
                            input.value
                        )));
                    };
                    let sync_value = builder
                        .build_int_z_extend(value, llvm_context.i8_type(), "runtime_bool")
                        .map_err(llvm_error)?;
                    let bits = builder
                        .build_int_z_extend(sync_value, llvm_context.i64_type(), "runtime_bits")
                        .map_err(llvm_error)?;
                    (
                        self.sync_bool,
                        BasicValueEnum::from(sync_value),
                        llvm_context.i8_type().const_zero(),
                        bits,
                    )
                }
                WxType::Scalar(WxScalarType::I64) => {
                    let BasicValueEnum::IntValue(bits) = value else {
                        return Err(CompileError::InvalidFunction(format!(
                            "integer value {} is not an integer",
                            input.value
                        )));
                    };
                    (
                        self.sync_i64,
                        value,
                        llvm_context.i8_type().const_int(1, false),
                        bits,
                    )
                }
                WxType::Scalar(WxScalarType::F64) => {
                    let bits = builder
                        .build_bit_cast(value, llvm_context.i64_type(), "runtime_bits")
                        .map_err(llvm_error)?
                        .into_int_value();
                    (
                        self.sync_f64,
                        value,
                        llvm_context.i8_type().const_int(2, false),
                        bits,
                    )
                }
                WxType::Scalar(WxScalarType::RuntimeHandle) => continue,
                ty => return Err(CompileError::UnsupportedType(ty)),
            };
            fused.push((register, tag, bits, function, sync_value));
        }
        let pc = llvm_context.i32_type().const_int(u64::from(call.pc), false);
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
        let packed_call = match (fused.as_slice(), call.output) {
            ([(register, tag, bits, _, _)], Some(output))
                if one_result(call.instruction)?.ty == WxType::Scalar(WxScalarType::I64) =>
            {
                let output = llvm_context.i32_type().const_int(u64::from(output), false);
                Some(
                    builder
                        .build_call(
                            execute_one_i64,
                            &[
                                environment.context.into(),
                                pc.into(),
                                (*register).into(),
                                (*tag).into(),
                                (*bits).into(),
                                output.into(),
                            ],
                            "runtime_packed_i64",
                        )
                        .map_err(llvm_error)?
                        .try_as_basic_value()
                        .basic()
                        .ok_or_else(|| {
                            CompileError::Backend(
                                "LLVM packed runtime result is absent".to_string(),
                            )
                        })?
                        .into_int_value(),
                )
            }
            _ => None,
        };
        let (function, arguments) = match fused.as_slice() {
            [] => (execute, vec![environment.context.into(), pc.into()]),
            [(register, tag, bits, _, _)] => (
                execute_one,
                vec![
                    environment.context.into(),
                    pc.into(),
                    (*register).into(),
                    (*tag).into(),
                    (*bits).into(),
                ],
            ),
            [
                (register0, tag0, bits0, _, _),
                (register1, tag1, bits1, _, _),
            ] => (
                execute_two,
                vec![
                    environment.context.into(),
                    pc.into(),
                    (*register0).into(),
                    (*tag0).into(),
                    (*bits0).into(),
                    (*register1).into(),
                    (*tag1).into(),
                    (*bits1).into(),
                ],
            ),
            inputs => {
                for (register, _, _, function, value) in inputs {
                    builder
                        .build_call(
                            *function,
                            &[
                                environment.context.into(),
                                (*register).into(),
                                (*value).into(),
                            ],
                            "",
                        )
                        .map_err(llvm_error)?;
                }
                (execute, vec![environment.context.into(), pc.into()])
            }
        };
        let status = if let Some(packed) = packed_call {
            builder
                .build_and(
                    packed,
                    llvm_context.i64_type().const_int(1, false),
                    "runtime_status",
                )
                .map_err(llvm_error)?
        } else {
            let execution_call = builder
                .build_call(function, &arguments, "runtime_status")
                .map_err(llvm_error)?;
            match execution_call.try_as_basic_value().basic() {
                Some(BasicValueEnum::IntValue(value)) => value,
                _ => {
                    return Err(CompileError::Backend(
                        "LLVM runtime status is absent".to_string(),
                    ));
                }
            }
        };
        let succeeded = builder
            .build_int_compare(
                IntPredicate::NE,
                status,
                status.get_type().const_zero(),
                "runtime_succeeded",
            )
            .map_err(llvm_error)?;
        let continuation =
            llvm_context.append_basic_block(environment.llvm_function, "runtime_continue");
        builder
            .build_conditional_branch(succeeded, continuation, environment.error_block)
            .map_err(llvm_error)?;
        builder.position_at_end(continuation);

        if let Some(packed) = packed_call {
            let result = one_result(call.instruction)?;
            let value = builder
                .build_right_shift(
                    packed,
                    llvm_context.i64_type().const_int(1, false),
                    true,
                    "runtime_i64",
                )
                .map(BasicValueEnum::from)
                .map_err(llvm_error)?;
            environment.values.insert(result.id, value);
            return Ok(());
        }

        if let Some(register) = call.output {
            let result = one_result(call.instruction)?;
            let register = llvm_context
                .i32_type()
                .const_int(u64::from(register), false);
            let function = match result.ty {
                WxType::Scalar(WxScalarType::I1) => self.read_bool,
                WxType::Scalar(WxScalarType::I64) => self.read_i64,
                WxType::Scalar(WxScalarType::F64) => self.read_f64,
                WxType::Scalar(WxScalarType::RuntimeHandle) => self.read_ptr,
                ty => return Err(CompileError::UnsupportedType(ty)),
            };
            let call = builder
                .build_call(
                    function,
                    &[environment.context.into(), register.into()],
                    "runtime_value",
                )
                .map_err(llvm_error)?;
            let value = call.try_as_basic_value().basic().ok_or_else(|| {
                CompileError::Backend("LLVM runtime read returned no value".to_string())
            })?;
            let value = match (result.ty, value) {
                (WxType::Scalar(WxScalarType::I1), BasicValueEnum::IntValue(value)) => builder
                    .build_int_compare(
                        IntPredicate::NE,
                        value,
                        llvm_context.i8_type().const_zero(),
                        "runtime_bool",
                    )
                    .map(BasicValueEnum::from)
                    .map_err(llvm_error)?,
                (_, value) => value,
            };
            environment.values.insert(result.id, value);
        }
        Ok(())
    }
}

pub(super) struct RuntimeEnvironment<'a, 'ctx> {
    pub(super) context: PointerValue<'ctx>,
    pub(super) error_block: BasicBlock<'ctx>,
    pub(super) llvm_function: FunctionValue<'ctx>,
    pub(super) values: &'a mut HashMap<WxValueId, BasicValueEnum<'ctx>>,
}

pub(super) struct RuntimeCall<'a> {
    pub(super) instruction: &'a WxInst,
    pub(super) pc: u32,
    pub(super) inputs: &'a [WxRuntimeInput],
    pub(super) output: Option<Register>,
    pub(super) sequence: bool,
}

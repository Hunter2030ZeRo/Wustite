use crate::bytecode::Instruction;
use crate::executable::ExecutableFunction;
use crate::value::Value;

use super::arithmetic::ValueOps;
use super::quickening::{QuickOutcome, execute_quick};
use super::{Frame, FunctionRuntime, Vm};

struct DispatchContext<'a> {
    executable: &'a ExecutableFunction,
    runtime: &'a mut FunctionRuntime,
    frame: &'a mut Frame,
}

impl Vm {
    pub(super) fn execute_with_runtime(
        &mut self,
        executable: &ExecutableFunction,
        runtime: &mut FunctionRuntime,
        frame: &mut Frame,
    ) -> Result<super::ExecutionResult, String> {
        let function = executable.bytecode();
        while frame.pc < function.code.len() {
            if frame.suppress_osr_pc != Some(frame.pc)
                && let Some(region_id) = executable.structure_map().region_by_entry_pc(frame.pc)
            {
                runtime.profile.record_entry(region_id);
                if let Some(region) = executable.structure_map().region(region_id) {
                    match self.jit_policy {
                        crate::planner::JitPolicy::Profile => {
                            runtime.profile.observe_entry(
                                region_id,
                                &region.entry_summary,
                                &frame.registers,
                            );
                            runtime.profile.observe_entry_sequences(
                                region_id,
                                &region.entry_summary,
                                &frame.registers,
                                &self.object_heap,
                            );
                        }
                        crate::planner::JitPolicy::StructureMap => {
                            if let Some(schema) = runtime.profile_schemas.get(region_id.0) {
                                runtime
                                    .profile
                                    .observe_entry_schema(schema, &frame.registers);
                                runtime.profile.observe_entry_sequences_schema(
                                    schema,
                                    &frame.registers,
                                    &self.object_heap,
                                );
                            }
                        }
                    }
                }
                let adaptive_result = self.adaptive_v2.as_ref().and_then(|adaptive| {
                    adaptive.try_execute_loop(
                        executable,
                        region_id,
                        &frame.registers,
                        &mut self.object_heap,
                    )
                });
                if let Some(result) = adaptive_result {
                    match result? {
                        crate::adaptive_v2::integration::LoopExecution::Return(value) => {
                            return Ok(super::ExecutionResult { value });
                        }
                        crate::adaptive_v2::integration::LoopExecution::Resume {
                            target,
                            registers,
                        } => {
                            for (register, value) in registers {
                                let slot =
                                    frame.registers.get_mut(usize::from(register)).ok_or_else(
                                        || "adaptive-v2 loop resume register overflow".to_owned(),
                                    )?;
                                *slot = value;
                            }
                            frame.pc = target;
                            continue;
                        }
                    }
                }
            }
            if self.try_execute_region(executable, frame, runtime) {
                continue;
            }
            if let Some(instruction) = runtime.quick_code.get(frame.pc)
                && execute_quick(instruction, frame, &mut self.object_heap)?
                    == QuickOutcome::Handled
            {
                continue;
            }
            let instruction = function
                .code
                .get(frame.pc)
                .ok_or_else(|| format!("invalid instruction pc {}", frame.pc))?;
            let context = DispatchContext {
                executable,
                runtime,
                frame,
            };
            if let Some(value) = self.execute_instruction(context, instruction)? {
                return Ok(super::ExecutionResult { value });
            }
        }
        Err("function ended without Return".to_string())
    }

    fn execute_instruction(
        &mut self,
        context: DispatchContext<'_>,
        instruction: &Instruction,
    ) -> Result<Option<Value>, String> {
        let DispatchContext {
            executable,
            runtime,
            frame,
        } = context;
        match instruction {
            Instruction::ConstSmallInt { dst, value } | Instruction::ConstI64 { dst, value } => {
                Self::write_register(frame, *dst, Value::SmallInt(*value))?;
                frame.pc += 1;
            }
            Instruction::ConstBool { dst, value } => {
                Self::write_register(frame, *dst, Value::Bool(*value))?;
                frame.pc += 1;
            }
            Instruction::ConstNone { dst } => {
                Self::write_register(frame, *dst, Value::None)?;
                frame.pc += 1;
            }
            Instruction::Call {
                dst,
                callable,
                args,
                ..
            } => {
                if self.execute_adaptive_object_instruction(
                    executable,
                    &mut frame.registers,
                    frame.pc,
                    instruction,
                )? {
                    runtime.profile.observe_instruction(
                        frame.pc,
                        instruction,
                        &frame.registers,
                        &self.object_heap,
                    );
                    frame.pc += 1;
                    return Ok(None);
                }
                if runtime.jit.is_some()
                    && super::native_jit::leaf::execute_numeric_leaf_call(
                        self,
                        runtime,
                        &mut frame.registers,
                        frame.pc,
                        instruction,
                    )?
                {
                    runtime.profile.observe_instruction(
                        frame.pc,
                        instruction,
                        &frame.registers,
                        &self.object_heap,
                    );
                    frame.pc += 1;
                    return Ok(None);
                }
                self.jit_report.record_interpreter_guest_call();
                let callable = Self::read_register(frame, *callable)?;
                let arguments = args
                    .iter()
                    .map(|register| Self::read_register(frame, *register))
                    .collect::<Result<Vec<_>, String>>()?;
                let value =
                    self.invoke_callable(executable, runtime, frame.pc, callable, &arguments)?;
                Self::write_register(frame, *dst, value)?;
                runtime.profile.observe_instruction(
                    frame.pc,
                    instruction,
                    &frame.registers,
                    &self.object_heap,
                );
                frame.pc += 1;
            }
            Instruction::CallMethod {
                dst,
                receiver,
                name,
                args,
            } => {
                if self.execute_adaptive_object_instruction(
                    executable,
                    &mut frame.registers,
                    frame.pc,
                    instruction,
                )? {
                    runtime.profile.observe_instruction(
                        frame.pc,
                        instruction,
                        &frame.registers,
                        &self.object_heap,
                    );
                    frame.pc += 1;
                    return Ok(None);
                }
                self.jit_report.record_interpreter_guest_call();
                let (receiver, function) = self.prepared_method(
                    runtime,
                    frame.pc,
                    Self::read_register(frame, *receiver)?,
                    name,
                )?;
                let mut arguments = Vec::with_capacity(args.len() + 1);
                arguments.push(Value::Object(receiver));
                arguments.extend(
                    args.iter()
                        .map(|register| Self::read_register(frame, *register))
                        .collect::<Result<Vec<_>, String>>()?,
                );
                let value = self
                    .invoke(executable, runtime, function.as_ref(), &arguments)?
                    .value;
                Self::write_register(frame, *dst, value)?;
                runtime.profile.observe_instruction(
                    frame.pc,
                    instruction,
                    &frame.registers,
                    &self.object_heap,
                );
                frame.pc += 1;
            }
            instruction @ (Instruction::ConstFloat { .. }
            | Instruction::LoadConstant { .. }
            | Instruction::BinaryOp { .. }
            | Instruction::CompareOp { .. }
            | Instruction::UnaryOp { .. }
            | Instruction::BooleanOp { .. }
            | Instruction::BuildTuple { .. }
            | Instruction::BuildList { .. }
            | Instruction::BuildDict { .. }
            | Instruction::GetItem { .. }
            | Instruction::GetAttr { .. }
            | Instruction::GetSlice { .. }
            | Instruction::SetItem { .. }
            | Instruction::SetAttr { .. }
            | Instruction::SetSlice { .. }
            | Instruction::ListAppend { .. }
            | Instruction::ListInsert { .. }
            | Instruction::ListPop { .. }
            | Instruction::Length { .. }
            | Instruction::LoadCurrentFunction { .. }) => {
                self.execute_runtime_instruction(
                    executable,
                    runtime,
                    &mut frame.registers,
                    frame.pc,
                    instruction,
                )?;
                runtime.profile.observe_instruction(
                    frame.pc,
                    instruction,
                    &frame.registers,
                    &self.object_heap,
                );
                frame.pc += 1;
            }
            Instruction::AddI64 { dst, lhs, rhs } => {
                let lhs = super::registers::read_small_int(frame, *lhs)?;
                let rhs = super::registers::read_small_int(frame, *rhs)?;
                let value = ValueOps::new(&mut self.object_heap).binary(
                    crate::bytecode::BinaryOperator::Add,
                    Value::SmallInt(lhs),
                    Value::SmallInt(rhs),
                )?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::LtI64 { dst, lhs, rhs } => {
                let lhs = super::registers::read_small_int(frame, *lhs)?;
                let rhs = super::registers::read_small_int(frame, *rhs)?;
                Self::write_register(frame, *dst, Value::Bool(lhs < rhs))?;
                frame.pc += 1;
            }
            Instruction::Move { dst, src } => {
                let value = Self::read_register(frame, *src)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::Jump { target } => {
                let adaptive_result = self.adaptive_v2.as_ref().and_then(|adaptive| {
                    adaptive.try_execute_preheader_loop(
                        executable,
                        frame.pc,
                        *target,
                        &frame.registers,
                        &mut self.object_heap,
                    )
                });
                if let Some(result) = adaptive_result {
                    match result? {
                        crate::adaptive_v2::integration::LoopExecution::Return(value) => {
                            return Ok(Some(value));
                        }
                        crate::adaptive_v2::integration::LoopExecution::Resume {
                            target,
                            registers,
                        } => {
                            for (register, value) in registers {
                                let slot = frame
                                    .registers
                                    .get_mut(usize::from(register))
                                    .ok_or_else(|| {
                                        "adaptive-v2 preheader resume register overflow".to_owned()
                                    })?;
                                *slot = value;
                            }
                            frame.pc = target;
                        }
                    }
                } else {
                    frame.pc = *target;
                }
            }
            Instruction::Branch { cond, yes, no } => {
                frame.pc = if super::registers::read_bool(frame, *cond)? {
                    *yes
                } else {
                    *no
                };
            }
            Instruction::Return { src } => return Self::read_register(frame, *src).map(Some),
        }
        Ok(None)
    }
}

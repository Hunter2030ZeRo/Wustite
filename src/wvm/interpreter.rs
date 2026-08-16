use crate::bytecode::{BooleanOperator, Instruction};
use crate::executable::ExecutableFunction;
use crate::value::Value;

use super::arithmetic::ValueOps;
use super::objects::ObjectOps;
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
        registers: Vec<Value>,
    ) -> Result<super::ExecutionResult, String> {
        let function = executable.bytecode();
        let mut frame = Frame {
            pc: 0,
            registers,
            suppress_osr_pc: None,
            suppressed_regions: std::collections::HashSet::new(),
        };
        while frame.pc < function.code.len() {
            if frame.suppress_osr_pc != Some(frame.pc)
                && let Some(region_id) = executable.structure_map().region_by_entry_pc(frame.pc)
            {
                runtime.profile.record_entry(region_id);
            }
            if self.try_execute_region(executable, &mut frame, runtime) {
                continue;
            }
            if let Some(instruction) = runtime.quick_code.get(frame.pc)
                && execute_quick(instruction, &mut frame, &mut self.object_heap)?
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
                frame: &mut frame,
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
            Instruction::ConstFloat { dst, value } => {
                Self::write_register(frame, *dst, Value::Float(*value))?;
                frame.pc += 1;
            }
            Instruction::ConstBool { dst, value } => {
                Self::write_register(frame, *dst, Value::Bool(*value))?;
                frame.pc += 1;
            }
            Instruction::LoadConstant { dst, constant } => {
                let value = self.load_constant(executable, runtime, constant.0)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::BinaryOp {
                dst, op, lhs, rhs, ..
            } => {
                let lhs = Self::read_register(frame, *lhs)?;
                let rhs = Self::read_register(frame, *rhs)?;
                let value = ValueOps::new(&mut self.object_heap).binary(*op, lhs, rhs)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::CompareOp {
                dst, op, lhs, rhs, ..
            } => {
                let lhs = Self::read_register(frame, *lhs)?;
                let rhs = Self::read_register(frame, *rhs)?;
                let value = ValueOps::new(&mut self.object_heap).compare(*op, lhs, rhs)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::UnaryOp { dst, op, src } => {
                let value = Self::read_register(frame, *src)?;
                let result = ValueOps::new(&mut self.object_heap).unary(*op, value)?;
                Self::write_register(frame, *dst, result)?;
                frame.pc += 1;
            }
            Instruction::BooleanOp {
                dst, op, lhs, rhs, ..
            } => {
                let lhs = super::registers::read_bool(frame, *lhs)?;
                let rhs = super::registers::read_bool(frame, *rhs)?;
                let value = match op {
                    BooleanOperator::And => lhs && rhs,
                    BooleanOperator::Or => lhs || rhs,
                };
                Self::write_register(frame, *dst, Value::Bool(value))?;
                frame.pc += 1;
            }
            Instruction::BuildTuple { dst, items } => {
                let values = super::registers::read_values(frame, items)?;
                let value = ObjectOps::new(&mut self.object_heap).tuple(values)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::BuildList { dst, items } => {
                let values = super::registers::read_values(frame, items)?;
                let value = ObjectOps::new(&mut self.object_heap).list(values)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::BuildDict { dst, entries } => {
                let mut values = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    values.push((
                        Self::read_register(frame, *key)?,
                        Self::read_register(frame, *value)?,
                    ));
                }
                let value = ObjectOps::new(&mut self.object_heap).dict(values)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::GetItem { dst, object, key } => {
                let object = Self::read_register(frame, *object)?;
                let key = Self::read_register(frame, *key)?;
                let value = ObjectOps::new(&mut self.object_heap).get_item(object, key)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::SetItem {
                object, key, value, ..
            } => {
                let object = Self::read_register(frame, *object)?;
                let key = Self::read_register(frame, *key)?;
                let value = Self::read_register(frame, *value)?;
                ObjectOps::new(&mut self.object_heap).set_item(object, key, value)?;
                frame.pc += 1;
            }
            Instruction::Length { dst, object } => {
                let object = Self::read_register(frame, *object)?;
                let value = ObjectOps::new(&mut self.object_heap).length(object)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::LoadCurrentFunction { dst } => {
                let value = self.load_current_function(executable, runtime)?;
                Self::write_register(frame, *dst, value)?;
                frame.pc += 1;
            }
            Instruction::Call {
                dst,
                callable,
                args,
                ..
            } => {
                let callable = Self::read_register(frame, *callable)?;
                let arguments = super::registers::read_values(frame, args)?;
                let function = self.callable(callable)?;
                let value = self
                    .invoke(executable, runtime, &function, &arguments)?
                    .value;
                Self::write_register(frame, *dst, value)?;
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
            Instruction::Jump { target } => frame.pc = *target,
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

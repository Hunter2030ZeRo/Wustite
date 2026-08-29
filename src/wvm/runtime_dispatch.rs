use crate::bytecode::Instruction;
use crate::executable::ExecutableFunction;
use crate::value::Value;

use super::arithmetic::ValueOps;
use super::objects::ObjectOps;
use super::{FunctionRuntime, Vm};

impl Vm {
    pub(super) fn execute_adaptive_object_instruction(
        &mut self,
        executable: &ExecutableFunction,
        registers: &mut [Value],
        pc: usize,
        instruction: &Instruction,
    ) -> Result<bool, String> {
        let Some(adaptive) = self.adaptive_v2.clone() else {
            return Ok(false);
        };
        let Some(ticket) = adaptive.object_before(
            self.adaptive_execution_id,
            executable,
            pc,
            instruction,
            registers,
            &mut self.object_heap,
        ) else {
            return Ok(false);
        };
        if let Some((dst, value)) = ticket.output() {
            write(registers, dst, value)?;
        }
        adaptive.object_after(ticket, registers);
        Ok(true)
    }

    pub(super) fn execute_runtime_instruction(
        &mut self,
        executable: &ExecutableFunction,
        runtime: &mut FunctionRuntime,
        registers: &mut [Value],
        pc: usize,
        instruction: &Instruction,
    ) -> Result<(), String> {
        if self.execute_adaptive_object_instruction(executable, registers, pc, instruction)? {
            return Ok(());
        }
        match instruction {
            Instruction::ConstFloat { dst, value } => write(registers, *dst, Value::Float(*value)),
            Instruction::ConstNone { dst } => write(registers, *dst, Value::None),
            Instruction::LoadConstant { dst, constant } => {
                let value = self.load_constant(executable, runtime, constant.0)?;
                write(registers, *dst, value)
            }
            Instruction::BinaryOp {
                dst, op, lhs, rhs, ..
            } => {
                let value = ValueOps::new(&mut self.object_heap).binary(
                    *op,
                    read(registers, *lhs)?,
                    read(registers, *rhs)?,
                )?;
                write(registers, *dst, value)
            }
            Instruction::CompareOp {
                dst, op, lhs, rhs, ..
            } => {
                let value = ValueOps::new(&mut self.object_heap).compare(
                    *op,
                    read(registers, *lhs)?,
                    read(registers, *rhs)?,
                )?;
                write(registers, *dst, value)
            }
            Instruction::UnaryOp { dst, op, src } => {
                let value =
                    ValueOps::new(&mut self.object_heap).unary(*op, read(registers, *src)?)?;
                write(registers, *dst, value)
            }
            Instruction::BooleanOp {
                dst, op, lhs, rhs, ..
            } => {
                let lhs = read_bool(registers, *lhs)?;
                let rhs = read_bool(registers, *rhs)?;
                let value = match op {
                    crate::bytecode::BooleanOperator::And => lhs && rhs,
                    crate::bytecode::BooleanOperator::Or => lhs || rhs,
                };
                write(registers, *dst, Value::Bool(value))
            }
            Instruction::BuildTuple { dst, items } => {
                let value =
                    ObjectOps::new(&mut self.object_heap).tuple(read_many(registers, items)?)?;
                write(registers, *dst, value)
            }
            Instruction::BuildList { dst, items } => {
                let value =
                    ObjectOps::new(&mut self.object_heap).list(read_many(registers, items)?)?;
                write(registers, *dst, value)
            }
            Instruction::BuildDict { dst, entries } => {
                let values = entries
                    .iter()
                    .map(|(key, value)| Ok((read(registers, *key)?, read(registers, *value)?)))
                    .collect::<Result<Vec<_>, String>>()?;
                let value = ObjectOps::new(&mut self.object_heap).dict(values)?;
                write(registers, *dst, value)
            }
            Instruction::GetItem { dst, object, key } => {
                let value = ObjectOps::new(&mut self.object_heap)
                    .get_item(read(registers, *object)?, read(registers, *key)?)?;
                write(registers, *dst, value)
            }
            Instruction::GetAttr { dst, object, name } => {
                let Value::Object(receiver) = read(registers, *object)? else {
                    return Err("attribute receiver is not an object".to_string());
                };
                let value = self
                    .object_heap
                    .get_attribute(receiver, name)
                    .map_err(|error| error.to_string())?;
                write(registers, *dst, value)
            }
            Instruction::GetSlice {
                dst,
                object,
                start,
                stop,
                step,
            } => {
                let value = ObjectOps::new(&mut self.object_heap).get_slice(
                    read(registers, *object)?,
                    read_optional(registers, *start)?,
                    read_optional(registers, *stop)?,
                    read_optional(registers, *step)?,
                )?;
                write(registers, *dst, value)
            }
            Instruction::SetItem {
                object, key, value, ..
            } => ObjectOps::new(&mut self.object_heap).set_item(
                read(registers, *object)?,
                read(registers, *key)?,
                read(registers, *value)?,
            ),
            Instruction::SetAttr {
                object,
                name,
                value,
            } => {
                let Value::Object(receiver) = read(registers, *object)? else {
                    return Err("attribute receiver is not an object".to_string());
                };
                self.object_heap
                    .set_attribute(receiver, name.clone(), read(registers, *value)?)
                    .map_err(|error| error.to_string())
            }
            Instruction::SetSlice {
                object,
                start,
                stop,
                step,
                value,
            } => ObjectOps::new(&mut self.object_heap).set_slice(
                read(registers, *object)?,
                read_optional(registers, *start)?,
                read_optional(registers, *stop)?,
                read_optional(registers, *step)?,
                read(registers, *value)?,
            ),
            Instruction::ListAppend { list, value } => ObjectOps::new(&mut self.object_heap)
                .append_list(read(registers, *list)?, read(registers, *value)?),
            Instruction::ListInsert { list, index, value } => ObjectOps::new(&mut self.object_heap)
                .insert_list(
                    read(registers, *list)?,
                    read(registers, *index)?,
                    read(registers, *value)?,
                ),
            Instruction::ListPop { dst, list, index } => {
                let value = ObjectOps::new(&mut self.object_heap)
                    .pop_list(read(registers, *list)?, read(registers, *index)?)?;
                write(registers, *dst, value)
            }
            Instruction::Length { dst, object } => {
                let value =
                    ObjectOps::new(&mut self.object_heap).length(read(registers, *object)?)?;
                write(registers, *dst, value)
            }
            Instruction::LoadCurrentFunction { dst } => {
                let value = self.load_current_function(executable, runtime)?;
                write(registers, *dst, value)
            }
            Instruction::Call {
                dst,
                callable,
                args,
                ..
            } => {
                let callable = read(registers, *callable)?;
                let arguments = read_many(registers, args)?;
                let value = self.invoke_callable(executable, runtime, pc, callable, &arguments)?;
                write(registers, *dst, value)
            }
            Instruction::CallMethod {
                dst,
                receiver,
                name,
                args,
            } => {
                let (receiver, function) =
                    self.prepared_method(runtime, pc, read(registers, *receiver)?, name)?;
                let mut arguments = Vec::with_capacity(args.len() + 1);
                arguments.push(Value::Object(receiver));
                arguments.extend(read_many(registers, args)?);
                let value = self
                    .invoke(executable, runtime, function.as_ref(), &arguments)?
                    .value;
                write(registers, *dst, value)
            }
            Instruction::AddI64 { dst, lhs, rhs } => {
                let value = ValueOps::new(&mut self.object_heap).binary(
                    crate::bytecode::BinaryOperator::Add,
                    read(registers, *lhs)?,
                    read(registers, *rhs)?,
                )?;
                write(registers, *dst, value)
            }
            Instruction::LtI64 { dst, lhs, rhs } => {
                let lhs = read(registers, *lhs)?;
                let rhs = read(registers, *rhs)?;
                let value = ValueOps::new(&mut self.object_heap).compare(
                    crate::bytecode::CompareOperator::Lt,
                    lhs,
                    rhs,
                )?;
                write(registers, *dst, value)
            }
            Instruction::Move { dst, src } => write(registers, *dst, read(registers, *src)?),
            Instruction::ConstSmallInt { .. }
            | Instruction::ConstBool { .. }
            | Instruction::ConstI64 { .. }
            | Instruction::Jump { .. }
            | Instruction::Branch { .. }
            | Instruction::Return { .. } => {
                Err("instruction has no native runtime path".to_string())
            }
        }
    }
}

fn read(registers: &[Value], register: u16) -> Result<Value, String> {
    registers
        .get(usize::from(register))
        .copied()
        .ok_or_else(|| format!("invalid register r{register}"))
}

fn read_bool(registers: &[Value], register: u16) -> Result<bool, String> {
    match read(registers, register)? {
        Value::Bool(value) => Ok(value),
        Value::SmallInt(_)
        | Value::Float(_)
        | Value::None
        | Value::Object(_)
        | Value::Uninitialized => Err(format!("r{register} is not a boolean")),
    }
}

fn read_many(registers: &[Value], selected: &[u16]) -> Result<Vec<Value>, String> {
    selected
        .iter()
        .map(|register| read(registers, *register))
        .collect()
}

fn read_optional(registers: &[Value], register: Option<u16>) -> Result<Option<Value>, String> {
    register
        .map(|register| read(registers, register))
        .transpose()
}

fn write(registers: &mut [Value], register: u16, value: Value) -> Result<(), String> {
    let slot = registers
        .get_mut(usize::from(register))
        .ok_or_else(|| format!("invalid register r{register}"))?;
    *slot = value;
    Ok(())
}

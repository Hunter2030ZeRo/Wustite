use crate::bytecode::{Function, Instruction, Register};
use crate::value::Value;
use crate::profiler::Profile;

pub struct Frame {
    pc: usize,
    registers: Vec<Value>,
}

pub struct Vm {
    pub profile: Option<Profile>,
}

pub struct ExecutionResult {
    pub value: Value, 
    pub profile: Profile,
}

impl Vm {
    pub fn new() -> Self {
        Self { profile: None }
    }

    pub fn execute(&mut self, function: &Function) -> Result<ExecutionResult, String> {
        let mut frame = Frame {
            pc: 0,
            registers: vec![Value::Uninitialized; function.register_count],
        };

        let mut profile = Profile::new(function.code.len());

        while frame.pc < function.code.len() {
            profile.record(frame.pc);
            
            match &function.code[frame.pc] {
                Instruction::ConstI64 { dst, value } => {
                    write_register(&mut frame, *dst, Value::I64(*value))?;
                    frame.pc += 1;
                }

                Instruction::AddI64 { dst, lhs, rhs } => {
                    let lhs = read_i64(&frame, *lhs)?;
                    let rhs = read_i64(&frame, *rhs)?;

                    let result = lhs
                        .checked_add(rhs)
                        .ok_or_else(|| "i64 addition overflow".to_string())?;

                    write_register(&mut frame, *dst, Value::I64(result))?;
                    frame.pc += 1;
                }

                Instruction::LtI64 { dst, lhs, rhs } => {
                    let lhs = read_i64(&frame, *lhs)?;
                    let rhs = read_i64(&frame, *rhs)?;

                    write_register(&mut frame, *dst, Value::Bool(lhs < rhs))?;
                    frame.pc += 1;
                }

                Instruction::Jump { target } => {
                    validate_target(function, *target)?;
                    frame.pc = *target;
                }

                Instruction::Branch { cond, yes, no } => {
                    let condition = read_bool(&frame, *cond)?;
                    let target = if condition { *yes } else { *no };

                    validate_target(function, target)?;
                    frame.pc = target;
                }

                Instruction::Return { src } => {
                    let value = read_register(&frame, *src)?;

                    return Ok(ExecutionResult { value, profile });
                }
            }
        }

        Err("function ended without Return".to_string())
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

fn read_register(frame: &Frame, register: Register) -> Result<Value, String> {
    frame
        .registers
        .get(register as usize)
        .copied()
        .ok_or_else(|| format!("invalid register r{register}"))
}

fn write_register(frame: &mut Frame, register: Register, value: Value) -> Result<(), String> {
    let slot = frame
        .registers
        .get_mut(register as usize)
        .ok_or_else(|| format!("invalid register r{register}"))?;

    *slot = value;
    Ok(())
}

fn read_i64(frame: &Frame, register: Register) -> Result<i64, String> {
    match read_register(frame, register)? {
        Value::I64(value) => Ok(value),
        other => Err(format!("expected i64 in r{register}, found {other:?}")),
    }
}

fn read_bool(frame: &Frame, register: Register) -> Result<bool, String> {
    match read_register(frame, register)? {
        Value::Bool(value) => Ok(value),
        other => Err(format!("expected bool in r{register}, found {other:?}")),
    }
}

fn validate_target(function: &Function, target: usize) -> Result<(), String> {
    if target < function.code.len() {
        Ok(())
    } else {
        Err(format!("invalid jump target {target}"))
    }
}

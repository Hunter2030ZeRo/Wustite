use std::sync::{Arc, Mutex};

use crate::bytecode::{BinaryOperator, Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::object::Object;
use crate::object::ObjectRef;
use crate::structure_map::SlotType;
use crate::value::Value;

use super::super::{FunctionRuntime, Vm};

mod compiler;

use compiler::CompiledNumericLeaf;

pub(in crate::wvm) fn execute_numeric_leaf_call(
    vm: &mut Vm,
    runtime: &mut FunctionRuntime,
    registers: &mut [Value],
    pc: usize,
    instruction: &Instruction,
) -> Result<bool, String> {
    let Instruction::Call {
        dst,
        callable,
        args,
        ..
    } = instruction
    else {
        return Ok(false);
    };
    let Some(callable) = registers.get(usize::from(*callable)).copied() else {
        return Err(format!("missing register r{callable}"));
    };
    let Value::Object(reference) = callable else {
        return Ok(false);
    };
    let prepared = match runtime.leaf_calls.get(pc).and_then(Option::as_ref) {
        Some(prepared) if prepared.target == reference && prepared.arity == args.len() => {
            prepared.clone()
        }
        _ => {
            let function = match vm.object_heap.get(reference) {
                Ok(Object::Function(function)) => function.clone(),
                Ok(_) => return Ok(false),
                Err(error) => return Err(error.to_string()),
            };
            if args.len() != function.parameters().len() {
                return Ok(false);
            }
            let Some(plan) = NumericLeafPlan::new(&function).map(Arc::new) else {
                return Ok(false);
            };
            let prepared = PreparedLeafCall {
                target: reference,
                arity: args.len(),
                name: function.name().map(Arc::<str>::from),
                compiled: CompiledNumericLeaf::compile(&plan)
                    .ok()
                    .map(|compiled| Arc::new(Mutex::new(compiled))),
                plan,
            };
            let Some(slot) = runtime.leaf_calls.get_mut(pc) else {
                return Err(format!("missing leaf call site at pc {pc}"));
            };
            *slot = Some(prepared.clone());
            vm.jit_report.call_sites.leaf_plans =
                vm.jit_report.call_sites.leaf_plans.saturating_add(1);
            prepared
        }
    };
    let execution = if let Some(compiled) = &prepared.compiled {
        match compiled.lock() {
            Ok(compiled) => prepared.plan.execute_compiled(&compiled, registers, args)?,
            Err(_) => prepared.plan.execute(registers, args)?,
        }
    } else {
        prepared.plan.execute(registers, args)?
    };
    let Some(value) = execution else {
        return Ok(false);
    };
    let slot = registers
        .get_mut(usize::from(*dst))
        .ok_or_else(|| format!("missing register r{dst}"))?;
    *slot = value;
    vm.jit_report.guest_calls.direct_native =
        vm.jit_report.guest_calls.direct_native.saturating_add(1);
    vm.jit_report.call_sites.prepared_leaf_hit =
        vm.jit_report.call_sites.prepared_leaf_hit.saturating_add(1);
    if prepared.compiled.is_some() {
        vm.jit_report.call_sites.compiled_leaf_hit =
            vm.jit_report.call_sites.compiled_leaf_hit.saturating_add(1);
    }
    vm.jit_report.record_function_call(prepared.name.as_deref());
    Ok(true)
}

#[derive(Debug, Clone)]
pub(in crate::wvm) struct PreparedLeafCall {
    target: ObjectRef,
    arity: usize,
    name: Option<Arc<str>>,
    compiled: Option<Arc<Mutex<CompiledNumericLeaf>>>,
    plan: Arc<NumericLeafPlan>,
}

#[derive(Debug)]
pub(in crate::wvm) struct NumericLeafPlan {
    parameters: Vec<(Register, LeafScalarType)>,
    operations: Vec<LeafOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeafScalarType {
    Integer,
    Float,
    Bool,
}

#[derive(Debug, Clone, Copy)]
enum LeafOperation {
    Constant {
        dst: Register,
        bits: u64,
    },
    IntegerBinary {
        dst: Register,
        op: BinaryOperator,
        lhs: Register,
        rhs: Register,
    },
    FloatBinary {
        dst: Register,
        op: BinaryOperator,
        lhs: Register,
        lhs_ty: LeafScalarType,
        rhs: Register,
        rhs_ty: LeafScalarType,
    },
    Move {
        dst: Register,
        src: Register,
    },
    Return {
        src: Register,
        ty: LeafScalarType,
    },
}

impl NumericLeafPlan {
    fn new(function: &ExecutableFunction) -> Option<Self> {
        if function.bytecode().register_count > 32 {
            return None;
        }
        let mut types = [None; 32];
        let parameters = function
            .parameters()
            .iter()
            .map(|parameter| {
                let ty = leaf_type(parameter.ty)?;
                types[usize::from(parameter.register)] = Some(ty);
                Some((parameter.register, ty))
            })
            .collect::<Option<Vec<_>>>()?;
        let mut operations = Vec::with_capacity(function.bytecode().code.len());
        for instruction in &function.bytecode().code {
            let operation = match instruction {
                Instruction::ConstSmallInt { dst, value }
                | Instruction::ConstI64 { dst, value } => LeafOperation::Constant {
                    dst: *dst,
                    bits: *value as u64,
                },
                Instruction::ConstFloat { dst, value } => LeafOperation::Constant {
                    dst: *dst,
                    bits: value.to_bits(),
                },
                Instruction::ConstBool { dst, value } => LeafOperation::Constant {
                    dst: *dst,
                    bits: u64::from(*value),
                },
                Instruction::BinaryOp {
                    dst, op, lhs, rhs, ..
                } => {
                    let lhs_ty = types[usize::from(*lhs)]?;
                    let rhs_ty = types[usize::from(*rhs)]?;
                    if lhs_ty == LeafScalarType::Bool || rhs_ty == LeafScalarType::Bool {
                        return None;
                    }
                    let integer_result = lhs_ty == LeafScalarType::Integer
                        && rhs_ty == LeafScalarType::Integer
                        && matches!(
                            op,
                            BinaryOperator::Add
                                | BinaryOperator::Subtract
                                | BinaryOperator::Multiply
                                | BinaryOperator::FloorDivide
                        );
                    if integer_result {
                        types[usize::from(*dst)] = Some(LeafScalarType::Integer);
                        LeafOperation::IntegerBinary {
                            dst: *dst,
                            op: *op,
                            lhs: *lhs,
                            rhs: *rhs,
                        }
                    } else {
                        types[usize::from(*dst)] = Some(LeafScalarType::Float);
                        LeafOperation::FloatBinary {
                            dst: *dst,
                            op: *op,
                            lhs: *lhs,
                            lhs_ty,
                            rhs: *rhs,
                            rhs_ty,
                        }
                    }
                }
                Instruction::Move { dst, src } => {
                    types[usize::from(*dst)] = types[usize::from(*src)];
                    LeafOperation::Move {
                        dst: *dst,
                        src: *src,
                    }
                }
                Instruction::Return { src } => LeafOperation::Return {
                    src: *src,
                    ty: types[usize::from(*src)]?,
                },
                _ => return None,
            };
            match instruction {
                Instruction::ConstSmallInt { dst, .. } | Instruction::ConstI64 { dst, .. } => {
                    types[usize::from(*dst)] = Some(LeafScalarType::Integer);
                }
                Instruction::ConstFloat { dst, .. } => {
                    types[usize::from(*dst)] = Some(LeafScalarType::Float);
                }
                Instruction::ConstBool { dst, .. } => {
                    types[usize::from(*dst)] = Some(LeafScalarType::Bool);
                }
                _ => {}
            }
            operations.push(operation);
        }
        matches!(operations.last(), Some(LeafOperation::Return { .. })).then_some(Self {
            parameters,
            operations,
        })
    }

    fn execute(
        &self,
        caller_registers: &[Value],
        arguments: &[Register],
    ) -> Result<Option<Value>, String> {
        let mut registers = [0_u64; 32];
        for ((parameter, ty), argument) in self.parameters.iter().zip(arguments) {
            registers[usize::from(*parameter)] =
                value_bits(read(caller_registers, *argument)?, *ty)?;
        }
        for operation in &self.operations {
            match *operation {
                LeafOperation::Constant { dst, bits } => registers[usize::from(dst)] = bits,
                LeafOperation::IntegerBinary { dst, op, lhs, rhs } => {
                    let lhs = registers[usize::from(lhs)] as i64;
                    let rhs = registers[usize::from(rhs)] as i64;
                    let Some(value) = integer_binary(op, lhs, rhs)? else {
                        return Ok(None);
                    };
                    registers[usize::from(dst)] = value as u64;
                }
                LeafOperation::FloatBinary {
                    dst,
                    op,
                    lhs,
                    lhs_ty,
                    rhs,
                    rhs_ty,
                } => {
                    let lhs = float_value(registers[usize::from(lhs)], lhs_ty);
                    let rhs = float_value(registers[usize::from(rhs)], rhs_ty);
                    registers[usize::from(dst)] = float_binary(op, lhs, rhs)?.to_bits();
                }
                LeafOperation::Move { dst, src } => {
                    registers[usize::from(dst)] = registers[usize::from(src)];
                }
                LeafOperation::Return { src, ty } => {
                    return Ok(Some(bits_value(registers[usize::from(src)], ty)));
                }
            }
        }
        Err("numeric leaf ended without Return".to_string())
    }

    fn execute_compiled(
        &self,
        compiled: &CompiledNumericLeaf,
        caller_registers: &[Value],
        arguments: &[Register],
    ) -> Result<Option<Value>, String> {
        let mut argument_bits = [0_u64; 32];
        for (index, ((_, ty), argument)) in self.parameters.iter().zip(arguments).enumerate() {
            argument_bits[index] = value_bits(read(caller_registers, *argument)?, *ty)?;
        }
        let mut result = 0_u64;
        if !compiled.execute(&argument_bits[..self.parameters.len()], &mut result) {
            return Ok(None);
        }
        let Some(LeafOperation::Return { ty, .. }) = self.operations.last() else {
            return Err("numeric leaf ended without Return".to_string());
        };
        Ok(Some(bits_value(result, *ty)))
    }
}

const fn leaf_type(ty: SlotType) -> Option<LeafScalarType> {
    match ty {
        SlotType::SmallInt => Some(LeafScalarType::Integer),
        SlotType::Float => Some(LeafScalarType::Float),
        SlotType::Bool => Some(LeafScalarType::Bool),
        SlotType::Object(_) | SlotType::Any => None,
    }
}

fn value_bits(value: Value, ty: LeafScalarType) -> Result<u64, String> {
    match (value, ty) {
        (Value::SmallInt(value), LeafScalarType::Integer) => Ok(value as u64),
        (Value::Float(value), LeafScalarType::Float) => Ok(value.to_bits()),
        (Value::Bool(value), LeafScalarType::Bool) => Ok(u64::from(value)),
        _ => Err("numeric leaf argument type mismatch".to_string()),
    }
}

const fn bits_value(bits: u64, ty: LeafScalarType) -> Value {
    match ty {
        LeafScalarType::Integer => Value::SmallInt(bits as i64),
        LeafScalarType::Float => Value::Float(f64::from_bits(bits)),
        LeafScalarType::Bool => Value::Bool(bits != 0),
    }
}

fn integer_binary(op: BinaryOperator, lhs: i64, rhs: i64) -> Result<Option<i64>, String> {
    let value = match op {
        BinaryOperator::Add => lhs.checked_add(rhs),
        BinaryOperator::Subtract => lhs.checked_sub(rhs),
        BinaryOperator::Multiply => lhs.checked_mul(rhs),
        BinaryOperator::FloorDivide => {
            if rhs == 0 {
                return Err("division by zero".to_string());
            }
            let Some(quotient) = lhs.checked_div(rhs) else {
                return Ok(None);
            };
            let remainder = lhs % rhs;
            Some(if remainder != 0 && (lhs < 0) != (rhs < 0) {
                quotient - 1
            } else {
                quotient
            })
        }
        BinaryOperator::Divide | BinaryOperator::Power => return Ok(None),
    };
    Ok(value)
}

const fn float_value(bits: u64, ty: LeafScalarType) -> f64 {
    match ty {
        LeafScalarType::Integer => (bits as i64) as f64,
        LeafScalarType::Float => f64::from_bits(bits),
        LeafScalarType::Bool => unreachable!(),
    }
}

fn float_binary(op: BinaryOperator, lhs: f64, rhs: f64) -> Result<f64, String> {
    if matches!(op, BinaryOperator::Divide | BinaryOperator::FloorDivide) && rhs == 0.0 {
        return Err("division by zero".to_string());
    }
    Ok(match op {
        BinaryOperator::Add => lhs + rhs,
        BinaryOperator::Subtract => lhs - rhs,
        BinaryOperator::Multiply => lhs * rhs,
        BinaryOperator::Divide => lhs / rhs,
        BinaryOperator::FloorDivide => (lhs / rhs).floor(),
        BinaryOperator::Power => lhs.powf(rhs),
    })
}

fn read(registers: &[Value], register: crate::bytecode::Register) -> Result<Value, String> {
    registers
        .get(usize::from(register))
        .copied()
        .ok_or_else(|| format!("missing register r{register}"))
}

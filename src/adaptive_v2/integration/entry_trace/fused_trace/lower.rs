use std::collections::BTreeMap;

use super::{FusedAccessFact, FusedTraceRequest, RegisterValue, register_value};
use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity};
use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, SnapshotDraft, Terminator,
    ValueDef, ValueId, ValueType,
};
use crate::bytecode::{BinaryOperator, Instruction as WvmInstruction};
use crate::executable::ExecutableConstant;
use crate::structure_map::SlotType;
use crate::value::Value;

pub(super) fn lower(request: FusedTraceRequest<'_>) -> Result<Option<SnapshotDraft>, String> {
    let Some(lowered_parameters) = parameters(&request)? else {
        return Ok(None);
    };
    let parameters = lowered_parameters.definitions;
    let mut values = lowered_parameters.values;
    let mut next_value = lowered_parameters.next_value;
    let executable = request.executable;
    let mut instructions = Vec::new();
    let mut dependencies = base_dependencies(executable.id().as_u64(), request.facts.schema_epoch);
    let mut effect_sequence = 0;
    for (pc, instruction) in executable.bytecode().code.iter().enumerate() {
        match instruction {
            WvmInstruction::ConstSmallInt { dst, value }
            | WvmInstruction::ConstI64 { dst, value } => {
                define_constant(
                    *dst,
                    Constant::Integer(*value),
                    ValueType::I64,
                    pc,
                    &mut next_value,
                    &mut values,
                    &mut instructions,
                )?;
            }
            WvmInstruction::Move { dst, src } => {
                values.insert(*dst, register_value(&values, *src)?);
            }
            WvmInstruction::LoadConstant { dst, constant } => {
                if !matches!(
                    executable.constants().get(constant.0),
                    Some(ExecutableConstant::Function(_))
                ) {
                    return Ok(None);
                }
                values.insert(*dst, RegisterValue::Callee(constant.0));
            }
            WvmInstruction::GetItem { dst, object, key } => {
                let Some(fact) = request.facts.access(pc) else {
                    return Ok(None);
                };
                let handle = ssa(&values, *object, ValueType::Handle)?;
                let index = ssa(&values, *key, ValueType::I64)?;
                let kind = access_kind(fact, false, executable.id().as_u64(), &mut dependencies);
                let output = next(&mut next_value, ValueType::I64)?;
                instructions.push(Instruction::new(
                    kind.at_pc(pc_u32(pc)?),
                    vec![handle.id, index.id],
                    Some(output),
                    Effect::Read,
                ));
                values.insert(*dst, RegisterValue::Ssa(output));
            }
            WvmInstruction::SetItem { object, key, value } => {
                let Some(fact) = request.facts.access(pc) else {
                    return Ok(None);
                };
                let handle = ssa(&values, *object, ValueType::Handle)?;
                let index = ssa(&values, *key, ValueType::I64)?;
                let stored = ssa(&values, *value, ValueType::I64)?;
                let kind = access_kind(fact, true, executable.id().as_u64(), &mut dependencies);
                instructions.push(
                    Instruction::new(
                        kind.at_pc(pc_u32(pc)?),
                        vec![handle.id, index.id, stored.id],
                        None,
                        Effect::Write,
                    )
                    .ordered(effect_sequence),
                );
                effect_sequence = effect_sequence.saturating_add(1);
            }
            WvmInstruction::Call {
                dst,
                callable,
                args,
            } => {
                let RegisterValue::Callee(index) = register_value(&values, *callable)? else {
                    return Ok(None);
                };
                let Some(ExecutableConstant::Function(callee)) = executable.constants().get(index)
                else {
                    return Ok(None);
                };
                let Some(kind) = verified_inline(callee, args.len())? else {
                    return Ok(None);
                };
                let inputs = args
                    .iter()
                    .map(|register| ssa(&values, *register, ValueType::I64).map(|value| value.id))
                    .collect::<Result<Vec<_>, _>>()?;
                let output = next(&mut next_value, ValueType::I64)?;
                instructions.push(Instruction::new(
                    kind.at_pc(pc_u32(pc)?),
                    inputs,
                    Some(output),
                    Effect::Pure,
                ));
                values.insert(*dst, RegisterValue::Ssa(output));
                push_dependency(
                    &mut dependencies,
                    Dependency::current(
                        DependencyKind::Callee,
                        callee.id().as_u64(),
                        callee.id().as_u64(),
                    ),
                );
            }
            WvmInstruction::Return { src } => {
                let returned = ssa_any(&values, *src)?;
                let identity =
                    ExecutableIdentity::new(executable.id().as_u64(), executable.id().as_u64());
                return Ok(Some(
                    SnapshotDraft::new(
                        identity,
                        EntryKind::FunctionEntry,
                        BlockId::new(0),
                        vec![Block::new(
                            BlockId::new(0),
                            parameters,
                            instructions,
                            Terminator::Return {
                                values: vec![returned.id],
                            },
                        )],
                        Vec::new(),
                        Vec::new(),
                        dependencies,
                    )
                    .with_schema_epoch(request.permit.schema_epoch()),
                ));
            }
            _ => return Ok(None),
        }
    }
    Ok(None)
}

struct LoweredParameters {
    definitions: Vec<ValueDef>,
    values: BTreeMap<u16, RegisterValue>,
    next_value: u32,
}

fn parameters(request: &FusedTraceRequest<'_>) -> Result<Option<LoweredParameters>, String> {
    if request.executable.parameters().len() != request.arguments.len() {
        return Ok(None);
    }
    let mut values = BTreeMap::new();
    let mut definitions = Vec::with_capacity(request.arguments.len());
    for (index, (parameter, argument)) in request
        .executable
        .parameters()
        .iter()
        .zip(request.arguments)
        .enumerate()
    {
        let Some(ty) = live_type(parameter.ty, argument) else {
            return Ok(None);
        };
        let id = u32::try_from(index).map_err(|_| "fused parameter overflow".to_owned())?;
        let definition = ValueDef::new(ValueId::new(id), ty);
        definitions.push(definition);
        values.insert(parameter.register, RegisterValue::Ssa(definition));
    }
    let next = u32::try_from(definitions.len())
        .map_err(|_| "fused value identifier overflow".to_owned())?;
    Ok(Some(LoweredParameters {
        definitions,
        values,
        next_value: next,
    }))
}

fn live_type(slot: SlotType, value: &Value) -> Option<ValueType> {
    match (slot, value) {
        (SlotType::Any, Value::SmallInt(_)) => Some(ValueType::I64),
        (SlotType::Any, Value::Float(_)) => Some(ValueType::F64),
        (SlotType::Any, Value::Bool(_)) => Some(ValueType::Bool),
        (SlotType::Any, Value::Object(_)) => Some(ValueType::Handle),
        (SlotType::SmallInt, Value::SmallInt(_)) => Some(ValueType::I64),
        (SlotType::Float, Value::Float(_)) => Some(ValueType::F64),
        (SlotType::Bool, Value::Bool(_)) => Some(ValueType::Bool),
        (SlotType::Object(_), Value::Object(_)) => Some(ValueType::Handle),
        (SlotType::Any, Value::None | Value::Uninitialized)
        | (SlotType::SmallInt, _)
        | (SlotType::Float, _)
        | (SlotType::Bool, _)
        | (SlotType::Object(_), _) => None,
    }
}

fn access_kind(
    fact: FusedAccessFact,
    write: bool,
    executable: u64,
    dependencies: &mut Vec<Dependency>,
) -> InstructionKind {
    match fact {
        FusedAccessFact::ListI64 { layout_epoch } => {
            push_dependency(
                dependencies,
                Dependency::current(DependencyKind::ListLayout, executable, layout_epoch),
            );
            if write {
                InstructionKind::ListSet
            } else {
                InstructionKind::ListGet
            }
        }
    }
}

fn verified_inline(
    callee: &crate::executable::ExecutableFunction,
    arity: usize,
) -> Result<Option<InstructionKind>, String> {
    crate::verifier::verify(callee)?;
    let [operation, WvmInstruction::Return { src }] = callee.bytecode().code.as_slice() else {
        return Ok(None);
    };
    let WvmInstruction::BinaryOp {
        dst, op, lhs, rhs, ..
    } = operation
    else {
        return Ok(None);
    };
    let [left, right] = callee.parameters() else {
        return Ok(None);
    };
    if arity != 2
        || *dst != *src
        || *lhs != left.register
        || *rhs != right.register
        || left.ty != SlotType::SmallInt
        || right.ty != SlotType::SmallInt
    {
        return Ok(None);
    }
    Ok(match op {
        BinaryOperator::Add => Some(InstructionKind::IntegerAdd),
        BinaryOperator::Subtract => Some(InstructionKind::IntegerSubtract),
        BinaryOperator::Multiply => Some(InstructionKind::IntegerMultiply),
        BinaryOperator::Divide | BinaryOperator::FloorDivide | BinaryOperator::Power => None,
    })
}

fn define_constant(
    register: u16,
    constant: Constant,
    ty: ValueType,
    pc: usize,
    next_value: &mut u32,
    values: &mut BTreeMap<u16, RegisterValue>,
    instructions: &mut Vec<Instruction>,
) -> Result<(), String> {
    let output = next(next_value, ty)?;
    instructions.push(Instruction::new(
        InstructionKind::Constant(constant).at_pc(pc_u32(pc)?),
        Vec::new(),
        Some(output),
        Effect::Pure,
    ));
    values.insert(register, RegisterValue::Ssa(output));
    Ok(())
}

fn next(next_value: &mut u32, ty: ValueType) -> Result<ValueDef, String> {
    let id = *next_value;
    *next_value = next_value
        .checked_add(1)
        .ok_or_else(|| "fused value identifier overflow".to_owned())?;
    Ok(ValueDef::new(ValueId::new(id), ty))
}

fn ssa(
    values: &BTreeMap<u16, RegisterValue>,
    register: u16,
    expected: ValueType,
) -> Result<ValueDef, String> {
    let value = ssa_any(values, register)?;
    if value.ty != expected {
        return Err(format!("fused trace type changed for r{register}"));
    }
    Ok(value)
}

fn ssa_any(values: &BTreeMap<u16, RegisterValue>, register: u16) -> Result<ValueDef, String> {
    match register_value(values, register)? {
        RegisterValue::Ssa(value) => Ok(value),
        RegisterValue::Callee(_) => Err("fused trace cannot materialize a callee".to_owned()),
    }
}

fn base_dependencies(executable: u64, schema_epoch: u64) -> Vec<Dependency> {
    vec![
        Dependency::current(DependencyKind::Executable, executable, executable),
        Dependency::current(DependencyKind::Schema, executable, schema_epoch),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
    ]
}

fn push_dependency(dependencies: &mut Vec<Dependency>, dependency: Dependency) {
    if !dependencies.contains(&dependency) {
        dependencies.push(dependency);
    }
}

fn pc_u32(pc: usize) -> Result<u32, String> {
    u32::try_from(pc).map_err(|_| "fused trace pc overflow".to_owned())
}

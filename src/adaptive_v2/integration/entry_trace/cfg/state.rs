use std::collections::{BTreeMap, BTreeSet};

use super::{Builder, SsaValue};
use crate::adaptive_v2::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
};
use crate::adaptive_v2::wxir_v2::ir::{
    Effect, Instruction, InstructionKind, RootMap, SafepointId, ValueDef, ValueId, ValueType,
};
use crate::bytecode::Register;

pub(super) fn add_true_guard(
    builder: &mut Builder<'_>,
    register: Register,
    values: &BTreeMap<Register, SsaValue>,
    instructions: &mut Vec<Instruction>,
) -> Result<(), String> {
    let value = read(values, register)?;
    if value.ty != ValueType::Bool {
        return Err("adaptive-v2 true guard requires a boolean parameter".to_owned());
    }
    let point = SafepointId::new(1);
    instructions.push(Instruction::safepoint(
        InstructionKind::Guard { guard: 1 }.at_pc(0),
        vec![value.id],
        None,
        Effect::Pure,
        point,
    ));
    builder.root_maps.push(RootMap::new(point, BTreeSet::new()));
    builder.deopts.push(deopt(builder, 1, point, values)?);
    Ok(())
}

fn deopt(
    builder: &Builder<'_>,
    id: u32,
    point: SafepointId,
    values: &BTreeMap<Register, SsaValue>,
) -> Result<DeoptRecipe, String> {
    let mut dead = Vec::new();
    let registers = (0..builder.executable.bytecode().register_count)
        .map(|index| {
            let register = u16::try_from(index)
                .map_err(|_| "adaptive-v2 entry register index overflow".to_owned())?;
            Ok(match values.get(&register) {
                Some(value) => {
                    RegisterRecipe::new(register, RegisterSource::Ssa(value.id), value.ty)
                }
                None => {
                    dead.push(register);
                    RegisterRecipe::new(register, RegisterSource::UndefinedDead, ValueType::I64)
                }
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let frame = FrameRecipe::new(builder.identity.id, 0, registers).with_dead_registers(dead);
    Ok(DeoptRecipe::new(
        id,
        builder.identity,
        0,
        ResumeMode::ReplayBeforePc,
        vec![frame],
        point,
    )
    .with_dependencies(builder.dependencies.to_vec()))
}

pub(super) fn define(
    builder: &mut Builder<'_>,
    values: &mut BTreeMap<Register, SsaValue>,
    register: Register,
    ty: ValueType,
) -> ValueDef {
    let value = SsaValue {
        id: ValueId::new(builder.next_value),
        ty,
    };
    builder.next_value = builder.next_value.saturating_add(1);
    values.insert(register, value);
    ValueDef::new(value.id, value.ty)
}

pub(super) fn read(
    values: &BTreeMap<Register, SsaValue>,
    register: Register,
) -> Result<SsaValue, String> {
    values
        .get(&register)
        .copied()
        .ok_or_else(|| format!("adaptive-v2 entry reads undefined r{register}"))
}

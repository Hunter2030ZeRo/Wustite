use crate::object::ObjectKind;
use crate::structure_map::{EffectSummary, ValueComposition, ValueId, ValueOrigin};

use super::*;

impl RegionBuilder<'_> {
    pub(super) fn try_virtualize_container_access(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
    ) -> Result<Option<usize>, WxBuildError> {
        let Some((container, items)) = container_items(&self.executable.bytecode().code[pc]) else {
            return Ok(None);
        };
        let Some(consumer) = self.executable.bytecode().code.get(pc + 1) else {
            return Ok(None);
        };
        let access = match consumer {
            Instruction::Length { dst, object } if *object == container => {
                VirtualAccess::Length { dst: *dst }
            }
            Instruction::GetItem { dst, object, key } if *object == container => {
                VirtualAccess::Projection {
                    dst: *dst,
                    key: *key,
                }
            }
            _ => return Ok(None),
        };
        if pc + 1 > self.plan.backedge {
            return Ok(None);
        }
        let structure_map = self.executable.structure_map();
        let Some(output) = structure_map
            .instruction_fact(pc)
            .and_then(|instruction| instruction.output)
        else {
            return Ok(None);
        };
        let Some(value) = structure_map.value(output) else {
            return Ok(None);
        };
        let eligible_origin = matches!(
            value.origin,
            Fact::Proven(ValueOrigin::Allocation {
                kind: ObjectKind::Tuple | ObjectKind::List,
                ..
            })
        );
        let eligible_composition = matches!(
            &value.composition,
            Fact::Proven(ValueComposition::Sequence(composition)) if composition.len() == items.len()
        );
        if !value.is_virtualizable()
            || !eligible_origin
            || !eligible_composition
            || !has_single_length_consumer(structure_map, output, pc + 1)
            || !has_pure_instruction_fact(structure_map, pc + 1)
        {
            return Ok(None);
        }
        match access {
            VirtualAccess::Length { dst } => {
                let length = i64::try_from(items.len())
                    .map_err(|_| WxBuildError::IdSpaceExhausted("virtual container length"))?;
                self.emit_constant(
                    instructions,
                    environment,
                    dst,
                    WxScalarType::I64,
                    WxConstant::Int(length),
                )?;
            }
            VirtualAccess::Projection { dst, key } => {
                if !matches!(
                    value.origin,
                    Fact::Proven(ValueOrigin::Allocation {
                        kind: ObjectKind::Tuple,
                        ..
                    })
                ) {
                    return Ok(None);
                }
                let Some(index) = constant_index(
                    structure_map,
                    &self.executable.bytecode().code,
                    pc + 1,
                    key,
                    items.len(),
                ) else {
                    return Ok(None);
                };
                let Some(member) = environment.get(&items[index]).copied() else {
                    return Ok(None);
                };
                environment.insert(dst, member);
            }
        }
        Ok(Some(pc + 2))
    }
}

enum VirtualAccess {
    Length { dst: Register },
    Projection { dst: Register, key: Register },
}

fn container_items(instruction: &Instruction) -> Option<(Register, &[Register])> {
    match instruction {
        Instruction::BuildTuple { dst, items } | Instruction::BuildList { dst, items } => {
            Some((*dst, items))
        }
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstFloat { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstNone { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::BinaryOp { .. }
        | Instruction::CompareOp { .. }
        | Instruction::UnaryOp { .. }
        | Instruction::BooleanOp { .. }
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
        | Instruction::LoadCurrentFunction { .. }
        | Instruction::Call { .. }
        | Instruction::CallMethod { .. }
        | Instruction::AddI64 { .. }
        | Instruction::LtI64 { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. }
        | Instruction::Move { .. } => None,
    }
}

fn has_single_length_consumer(
    structure_map: &crate::structure_map::StructureMap,
    value: ValueId,
    length_pc: usize,
) -> bool {
    let consumers = structure_map
        .instruction_facts()
        .iter()
        .filter(|instruction| {
            instruction
                .inputs
                .iter()
                .any(|input| input.value == Some(value))
        })
        .collect::<Vec<_>>();
    consumers.len() == 1 && consumers[0].pc == length_pc
}

fn has_pure_instruction_fact(
    structure_map: &crate::structure_map::StructureMap,
    pc: usize,
) -> bool {
    matches!(
        structure_map
            .instruction_fact(pc)
            .map(|instruction| instruction.effects),
        Some(Fact::Proven(EffectSummary {
            may_mutate: false,
            may_allocate: false,
            may_call_unknown: false,
            may_access_global_state: false,
        }))
    )
}

fn constant_index(
    structure_map: &crate::structure_map::StructureMap,
    code: &[Instruction],
    pc: usize,
    key: Register,
    length: usize,
) -> Option<usize> {
    let key_value = structure_map
        .instruction_fact(pc)?
        .inputs
        .iter()
        .find(|input| input.register == key)?
        .value?;
    let Fact::Proven(ValueOrigin::Immediate { pc: constant_pc }) =
        structure_map.value(key_value)?.origin
    else {
        return None;
    };
    let Instruction::ConstSmallInt { value, .. } = code.get(constant_pc)? else {
        return None;
    };
    let length = i64::try_from(length).ok()?;
    let normalized = if *value < 0 {
        length.checked_add(*value)?
    } else {
        *value
    };
    let index = usize::try_from(normalized).ok()?;
    (index < usize::try_from(length).ok()?).then_some(index)
}

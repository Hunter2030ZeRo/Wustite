use crate::bytecode::Instruction;

use super::super::super::{
    BlockId, Fact, GuardPlacement, IdentityFact, InstructionFact, MutationEffect, MutationKind,
    SlotType, TypeFact, ValueFact, ValueId, ValueOrigin, ValueUse,
};

pub(super) fn classify(
    values: &[ValueFact],
    instructions: &mut [InstructionFact],
    code: &[Instruction],
    block_by_pc: &[BlockId],
) {
    for (pc, instruction) in code.iter().enumerate() {
        instructions[pc].mutations = Fact::Proven(mutation_effects(
            values,
            instruction,
            &instructions[pc].inputs,
        ));
    }
    for (pc, instruction) in code.iter().enumerate() {
        let Some(object) = sequence_object(instruction, &instructions[pc].inputs) else {
            continue;
        };
        let Some(id) = object.value else {
            instructions[pc].guard_placement = Fact::Unknown;
            continue;
        };
        let root = identity_root(values, id);
        let barrier = instructions[..pc].iter().any(|fact| {
            fact.mutations.candidate().is_none_or(|mutations| {
                mutations.iter().any(|mutation| {
                    mutation.identity_root == root
                        && matches!(mutation.kind, MutationKind::Layout | MutationKind::Unknown)
                })
            })
        });
        let placement = if barrier {
            GuardPlacement::AccessSite(pc)
        } else if values.get(root.0 as usize).is_some_and(|value| {
            matches!(value.origin, Fact::Proven(ValueOrigin::Parameter { .. }))
        }) {
            GuardPlacement::RegionEntry
        } else {
            GuardPlacement::BlockEntry(block_by_pc[pc])
        };
        instructions[pc].guard_placement = if requires_runtime_guard(values, root) {
            Fact::Guardable(placement)
        } else {
            Fact::Proven(placement)
        };
    }
}

fn mutation_effects(
    values: &[ValueFact],
    instruction: &Instruction,
    inputs: &[ValueUse],
) -> Vec<MutationEffect> {
    let (targets, kind): (Vec<_>, _) = match instruction {
        Instruction::SetItem { object, .. } => (
            inputs
                .iter()
                .filter(|input| input.register == *object)
                .collect(),
            MutationKind::Content,
        ),
        Instruction::SetSlice { object, .. } => (
            inputs
                .iter()
                .filter(|input| input.register == *object)
                .collect(),
            MutationKind::Layout,
        ),
        Instruction::ListAppend { list, .. }
        | Instruction::ListInsert { list, .. }
        | Instruction::ListPop { list, .. } => (
            inputs
                .iter()
                .filter(|input| input.register == *list)
                .collect(),
            MutationKind::Layout,
        ),
        Instruction::Call { callable, .. } => (
            inputs
                .iter()
                .filter(|input| input.register != *callable && is_object(values, input))
                .collect(),
            MutationKind::Unknown,
        ),
        _ => return Vec::new(),
    };
    let mut result = Vec::new();
    for target in targets {
        let Some(id) = target.value else { continue };
        let effect = MutationEffect {
            identity_root: identity_root(values, id),
            kind,
        };
        if !result.contains(&effect) {
            result.push(effect);
        }
    }
    result
}

fn sequence_object<'a>(instruction: &Instruction, inputs: &'a [ValueUse]) -> Option<&'a ValueUse> {
    let register = match instruction {
        Instruction::GetItem { object, .. }
        | Instruction::GetSlice { object, .. }
        | Instruction::SetItem { object, .. }
        | Instruction::SetSlice { object, .. }
        | Instruction::Length { object, .. } => *object,
        Instruction::ListAppend { list, .. }
        | Instruction::ListInsert { list, .. }
        | Instruction::ListPop { list, .. } => *list,
        _ => return None,
    };
    inputs.iter().find(|input| input.register == register)
}

fn identity_root(values: &[ValueFact], mut id: ValueId) -> ValueId {
    for _ in 0..values.len() {
        match values.get(id.0 as usize).map(|value| value.identity) {
            Some(Fact::Proven(IdentityFact::AliasOf(source))) => id = source,
            _ => return id,
        }
    }
    id
}

fn is_object(values: &[ValueFact], input: &&ValueUse) -> bool {
    input
        .value
        .and_then(|id| values.get(id.0 as usize))
        .is_some_and(|value| {
            matches!(
                value.ty,
                TypeFact::Proven(SlotType::Object(_)) | TypeFact::Guardable(SlotType::Object(_))
            )
        })
}

fn requires_runtime_guard(values: &[ValueFact], root: ValueId) -> bool {
    values.get(root.0 as usize).is_none_or(|value| {
        !matches!(value.sequence.strategy, Fact::Proven(_))
            || matches!(value.ty, TypeFact::Guardable(_) | TypeFact::Unknown)
    })
}

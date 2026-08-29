use crate::bytecode::Instruction;

use super::super::super::{
    BasicBlock, EscapeState, Fact, IdentityFact, InstructionFact, Region, ValueFact, ValueId,
};

pub(super) fn classify(
    values: &mut [ValueFact],
    instructions: &[InstructionFact],
    code: &[Instruction],
    blocks: &[BasicBlock],
    regions: &[Region],
) {
    for (pc, instruction) in code.iter().enumerate() {
        match instruction {
            Instruction::Call { .. } => {
                for input in &instructions[pc].inputs {
                    if let Some(id) = input.value {
                        raise(values, id, EscapeState::Unknown);
                    }
                }
            }
            Instruction::Return { .. } => {
                if let Some(id) = instructions[pc]
                    .inputs
                    .first()
                    .and_then(|input| input.value)
                {
                    raise(values, id, EscapeState::Function);
                }
            }
            _ => {}
        }
    }

    let definitions = values
        .iter()
        .filter_map(|value| value.defined_at.map(|pc| (value.id, pc)))
        .collect::<Vec<_>>();
    for (value_id, definition) in definitions {
        for region in regions {
            let inside = region_contains_pc(region, definition, blocks);
            if inside
                && instructions.iter().any(|instruction| {
                    !region_contains_pc(region, instruction.pc, blocks)
                        && instruction
                            .inputs
                            .iter()
                            .any(|input| input.value == Some(value_id))
                })
            {
                raise(values, value_id, EscapeState::Region);
            }
        }
    }

    loop {
        let before = values.iter().map(|value| value.escape).collect::<Vec<_>>();
        for instruction in instructions {
            if !instruction
                .effects
                .proven()
                .is_some_and(|effects| effects.may_mutate)
            {
                continue;
            }
            let Some(target) = instruction
                .mutated_values
                .proven()
                .and_then(|targets| targets.first())
                .copied()
            else {
                continue;
            };
            let target_escape = escape_of(values, target);
            if target_escape <= EscapeState::Local {
                continue;
            }
            for input in instruction.inputs.iter().skip(1) {
                if let Some(id) = input.value {
                    raise(values, id, target_escape);
                }
            }
        }
        if values.iter().map(|value| value.escape).eq(before) {
            break;
        }
    }
}

fn region_contains_pc(region: &Region, pc: usize, blocks: &[BasicBlock]) -> bool {
    region.blocks.iter().any(|id| {
        blocks
            .get(id.0 as usize)
            .is_some_and(|block| block.start_pc <= pc && pc < block.end_pc)
    })
}

fn escape_of(values: &[ValueFact], id: ValueId) -> EscapeState {
    values
        .get(id.0 as usize)
        .and_then(|value| value.escape.candidate().copied())
        .unwrap_or(EscapeState::Unknown)
}

fn raise(values: &mut [ValueFact], id: ValueId, state: EscapeState) {
    let root = identity_root(values, id);
    for index in 0..values.len() {
        let candidate = ValueId(index as u32);
        if identity_root(values, candidate) == root {
            let current = escape_of(values, candidate);
            values[index].escape = Fact::Proven(current.max(state));
        }
    }
}

fn identity_root(values: &[ValueFact], mut id: ValueId) -> ValueId {
    for _ in 0..=values.len() {
        let Some(value) = values.get(id.0 as usize) else {
            return id;
        };
        match value.identity {
            Fact::Proven(IdentityFact::AliasOf(source)) => id = source,
            Fact::Proven(IdentityFact::Fresh | IdentityFact::Unknown)
            | Fact::Guardable(_)
            | Fact::Unknown => return id,
        }
    }
    id
}

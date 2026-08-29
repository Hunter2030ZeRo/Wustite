use std::collections::HashSet;

use crate::bytecode::Instruction;

use super::super::super::{
    BasicBlock, BlockId, ControlDependency, Fact, InstructionFact, TypeFact, ValueFact, ValueId,
    ValueOrigin,
};

pub(super) fn classify(
    values: &[ValueFact],
    instructions: &mut [InstructionFact],
    code: &[Instruction],
    blocks: &[BasicBlock],
    block_by_pc: &[BlockId],
) {
    for (branch_pc, instruction) in code.iter().enumerate() {
        let Instruction::Branch { yes, no, .. } = instruction else {
            continue;
        };
        let Some(condition) = instructions[branch_pc].inputs.first().copied() else {
            continue;
        };
        let branch_block = block_by_pc[branch_pc];
        let yes_blocks = reachable(block_by_pc[*yes], branch_block, blocks);
        let no_blocks = reachable(block_by_pc[*no], branch_block, blocks);
        let hoistable = condition.value.map_or(Fact::Unknown, |id| {
            guard_fact(values, instructions, id, &mut HashSet::new())
        });
        add_dependencies(
            instructions,
            blocks,
            &yes_blocks,
            &no_blocks,
            ControlDependency {
                branch_pc,
                condition,
                expected: true,
                hoistable,
            },
        );
        add_dependencies(
            instructions,
            blocks,
            &no_blocks,
            &yes_blocks,
            ControlDependency {
                branch_pc,
                condition,
                expected: false,
                hoistable,
            },
        );
    }
}

fn reachable(start: BlockId, excluded: BlockId, blocks: &[BasicBlock]) -> HashSet<BlockId> {
    let mut found = HashSet::new();
    let mut pending = vec![start];
    while let Some(block) = pending.pop() {
        if block == excluded || !found.insert(block) {
            continue;
        }
        if let Some(block) = blocks.get(block.0 as usize) {
            pending.extend(block.successors.iter().map(|edge| edge.target));
        }
    }
    found
}

fn add_dependencies(
    instructions: &mut [InstructionFact],
    blocks: &[BasicBlock],
    included: &HashSet<BlockId>,
    excluded: &HashSet<BlockId>,
    dependency: ControlDependency,
) {
    for block_id in included.difference(excluded) {
        let Some(block) = blocks.get(block_id.0 as usize) else {
            continue;
        };
        for instruction in &mut instructions[block.start_pc..block.end_pc] {
            if instruction.pc != dependency.branch_pc
                && !instruction.control_dependencies.contains(&dependency)
            {
                instruction.control_dependencies.push(dependency);
            }
        }
    }
}

fn guard_fact(
    values: &[ValueFact],
    instructions: &[InstructionFact],
    id: ValueId,
    visiting: &mut HashSet<ValueId>,
) -> Fact<bool> {
    if !visiting.insert(id) {
        return Fact::Unknown;
    }
    let Some(value) = values.get(id.0 as usize) else {
        return Fact::Unknown;
    };
    let own_type = match value.ty {
        TypeFact::Proven(_) => Fact::Proven(true),
        TypeFact::Guardable(_) => Fact::Guardable(true),
        TypeFact::Unknown => Fact::Unknown,
    };
    let source = match &value.origin {
        Fact::Proven(ValueOrigin::Parameter { .. } | ValueOrigin::Immediate { .. }) => {
            Fact::Proven(true)
        }
        Fact::Proven(ValueOrigin::ConstantPool { .. }) => Fact::Proven(true),
        Fact::Proven(ValueOrigin::Alias { source, .. }) => {
            source.value.map_or(Fact::Unknown, |id| {
                guard_fact(values, instructions, id, visiting)
            })
        }
        Fact::Proven(ValueOrigin::Allocation { .. }) if value.is_virtualizable() => {
            composition_fact(values, value, instructions, visiting)
        }
        Fact::Proven(ValueOrigin::Operation { pc }) => {
            operation_fact(values, instructions, *pc, visiting)
        }
        Fact::Guardable(_) => Fact::Guardable(true),
        Fact::Proven(
            ValueOrigin::CurrentFunction { .. }
            | ValueOrigin::Projection { .. }
            | ValueOrigin::Call { .. }
            | ValueOrigin::Unknown { .. },
        )
        | Fact::Unknown => Fact::Proven(false),
        Fact::Proven(ValueOrigin::Allocation { .. }) => Fact::Proven(false),
    };
    visiting.remove(&id);
    combine(own_type, source)
}

fn operation_fact(
    values: &[ValueFact],
    instructions: &[InstructionFact],
    pc: usize,
    visiting: &mut HashSet<ValueId>,
) -> Fact<bool> {
    let Some(instruction) = instructions.get(pc) else {
        return Fact::Unknown;
    };
    if !instruction
        .effects
        .proven()
        .is_some_and(|effects| effects.is_pure())
    {
        return Fact::Proven(false);
    }
    match instruction.failures.as_ref() {
        Fact::Proven(failures) if failures.is_empty() => {}
        Fact::Guardable(failures) if failures.is_empty() => {}
        Fact::Proven(_) | Fact::Guardable(_) => return Fact::Proven(false),
        Fact::Unknown => return Fact::Unknown,
    }
    instruction
        .inputs
        .iter()
        .fold(Fact::Proven(true), |fact, input| {
            combine(
                fact,
                input.value.map_or(Fact::Unknown, |id| {
                    guard_fact(values, instructions, id, visiting)
                }),
            )
        })
}

fn composition_fact(
    values: &[ValueFact],
    value: &ValueFact,
    instructions: &[InstructionFact],
    visiting: &mut HashSet<ValueId>,
) -> Fact<bool> {
    let uses = match value.composition.proven() {
        Some(super::super::super::ValueComposition::Sequence(items)) => items.clone(),
        Some(super::super::super::ValueComposition::Mapping(entries)) => entries
            .iter()
            .flat_map(|(key, value)| [*key, *value])
            .collect(),
        Some(super::super::super::ValueComposition::None) => Vec::new(),
        None if matches!(value.composition, Fact::Guardable(_)) => {
            return Fact::Guardable(true);
        }
        None => return Fact::Unknown,
    };
    uses.iter().fold(Fact::Proven(true), |fact, input| {
        combine(
            fact,
            input.value.map_or(Fact::Unknown, |id| {
                guard_fact(values, instructions, id, visiting)
            }),
        )
    })
}

const fn combine(lhs: Fact<bool>, rhs: Fact<bool>) -> Fact<bool> {
    match (lhs, rhs) {
        (Fact::Proven(false), _) | (_, Fact::Proven(false)) => Fact::Proven(false),
        (Fact::Unknown, _) | (_, Fact::Unknown) => Fact::Unknown,
        (Fact::Guardable(_), _) | (_, Fact::Guardable(_)) => Fact::Guardable(true),
        (Fact::Proven(true), Fact::Proven(true)) => Fact::Proven(true),
    }
}

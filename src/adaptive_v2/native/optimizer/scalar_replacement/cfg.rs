use std::collections::{BTreeMap, BTreeSet};

use crate::adaptive_v2::wxir_v2::ir::{
    Constant, Effect, InstructionKind, SafepointId, SnapshotBody, Terminator, ValueId,
};

use super::virtual_deopt::{self, PendingVirtual};

pub(super) fn run(body: &mut SnapshotBody) -> bool {
    if body.blocks.iter().any(|block| {
        matches!(
            block.terminator,
            Terminator::Backedge { .. } | Terminator::IrreducibleBackedge
        )
    }) {
        return false;
    }
    let Some(shape) = virtual_deopt::object_shape(body) else {
        return false;
    };
    let live = virtual_deopt::live_points(body);
    for block_index in 0..body.blocks.len() {
        for allocate in 0..body.blocks[block_index].instructions.len() {
            let instruction = &body.blocks[block_index].instructions[allocate];
            let Some(handle) = instruction.output.map(|output| output.id) else {
                continue;
            };
            if !matches!(instruction.kind.semantic(), InstructionKind::Allocate) {
                continue;
            }
            let aliases = phi_aliases(body, handle);
            if aliases.len() == 1 {
                continue;
            }
            let Some(plan) = plan(body, block_index, allocate, aliases, &live) else {
                continue;
            };
            apply(body, plan, shape);
            return true;
        }
    }
    false
}

struct Plan {
    aliases: BTreeSet<ValueId>,
    fields: Vec<(u32, ValueId)>,
    replacements: Vec<(usize, usize, ValueId)>,
    removals: BTreeMap<usize, BTreeSet<usize>>,
    removed_point: Option<SafepointId>,
    live_points: BTreeSet<SafepointId>,
}

fn plan(
    body: &SnapshotBody,
    allocation_block: usize,
    allocation: usize,
    aliases: BTreeSet<ValueId>,
    live: &BTreeMap<ValueId, BTreeSet<SafepointId>>,
) -> Option<Plan> {
    let mut fields = BTreeMap::new();
    let mut replacements = Vec::new();
    let mut removals = BTreeMap::<usize, BTreeSet<usize>>::new();
    let mut last_set = None;
    for (block_index, block) in body.blocks.iter().enumerate() {
        for (index, instruction) in block.instructions.iter().enumerate() {
            let uses_alias = instruction
                .inputs
                .iter()
                .any(|input| aliases.contains(input));
            match instruction.kind.semantic() {
                InstructionKind::Allocate
                    if block_index == allocation_block && index == allocation =>
                {
                    removals.entry(block_index).or_default().insert(index);
                }
                InstructionKind::ObjectSet
                    if block_index == allocation_block
                        && instruction.inputs.len() == 3
                        && aliases.contains(&instruction.inputs[0]) =>
                {
                    let key = constant_key(body, instruction.inputs[1])?;
                    fields.insert(key, instruction.inputs[2]);
                    removals.entry(block_index).or_default().insert(index);
                    last_set = Some(index);
                }
                InstructionKind::ObjectGet
                    if instruction.inputs.len() == 2
                        && aliases.contains(&instruction.inputs[0]) =>
                {
                    let key = constant_key(body, instruction.inputs[1])?;
                    replacements.push((block_index, index, *fields.get(&key)?));
                }
                _ if uses_alias => return None,
                _ if instruction.effect.is_barrier() || instruction.effect == Effect::Write => {
                    return None;
                }
                _ => {}
            }
        }
        if terminator_escapes(&block.terminator, &aliases) {
            return None;
        }
    }
    if replacements.is_empty() || fields.is_empty() {
        return None;
    }
    let live_points = aliases
        .iter()
        .filter_map(|alias| live.get(alias))
        .flatten()
        .copied()
        .collect::<BTreeSet<_>>();
    if live_before_last_set(body, allocation_block, last_set?, &live_points) {
        return None;
    }
    Some(Plan {
        aliases,
        fields: fields.into_iter().collect(),
        replacements,
        removals,
        removed_point: body.blocks[allocation_block].instructions[allocation].safepoint,
        live_points,
    })
}

fn phi_aliases(body: &SnapshotBody, handle: ValueId) -> BTreeSet<ValueId> {
    let mut aliases = BTreeSet::from([handle]);
    loop {
        let mut added = false;
        for target in &body.blocks {
            for parameter_index in 0..target.parameters.len() {
                let incoming = body
                    .blocks
                    .iter()
                    .filter_map(|predecessor| match &predecessor.terminator {
                        Terminator::Jump {
                            target: id,
                            arguments,
                        } if *id == target.id => arguments.get(parameter_index).copied(),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if !incoming.is_empty()
                    && incoming.iter().all(|value| aliases.contains(value))
                    && aliases.insert(target.parameters[parameter_index].id)
                {
                    added = true;
                }
            }
        }
        if !added {
            return aliases;
        }
    }
}

fn terminator_escapes(terminator: &Terminator, aliases: &BTreeSet<ValueId>) -> bool {
    match terminator {
        Terminator::Jump { .. } => false,
        Terminator::Branch { condition, .. } => aliases.contains(condition),
        Terminator::Return { values } | Terminator::SideExit { values, .. } => {
            values.iter().any(|value| aliases.contains(value))
        }
        Terminator::Backedge { .. } | Terminator::IrreducibleBackedge => true,
    }
}

fn constant_key(body: &SnapshotBody, key: ValueId) -> Option<u32> {
    body.blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .find_map(|instruction| {
            (instruction.output.is_some_and(|output| output.id == key))
                .then(|| match instruction.kind.semantic() {
                    InstructionKind::Constant(Constant::Integer(value)) => {
                        u32::try_from(*value).ok()
                    }
                    _ => None,
                })
                .flatten()
        })
}

fn live_before_last_set(
    body: &SnapshotBody,
    allocation_block: usize,
    last_set: usize,
    points: &BTreeSet<SafepointId>,
) -> bool {
    body.blocks[allocation_block].instructions[..=last_set]
        .iter()
        .any(|instruction| {
            instruction
                .safepoint
                .is_some_and(|point| points.contains(&point))
        })
}

fn apply(body: &mut SnapshotBody, plan: Plan, shape: virtual_deopt::ObjectShape) {
    for (block, index, value) in plan.replacements {
        let instruction = &mut body.blocks[block].instructions[index];
        instruction.kind = InstructionKind::Copy;
        instruction.inputs = vec![value];
        instruction.effect = Effect::Pure;
        instruction.effect_sequence = None;
        instruction.safepoint = None;
    }
    remove_phi_aliases(body, &plan.aliases);
    for (block, removals) in plan.removals {
        let mut index = 0;
        body.blocks[block].instructions.retain(|_| {
            let keep = !removals.contains(&index);
            index += 1;
            keep
        });
    }
    let snapshots = plan
        .live_points
        .iter()
        .filter(|point| Some(**point) != plan.removed_point)
        .map(|point| (*point, plan.fields.clone()))
        .collect();
    let virtual_id = virtual_deopt::next_id(body);
    virtual_deopt::apply(
        body,
        &PendingVirtual {
            handles: plan.aliases,
            id: virtual_id,
            shape,
            snapshots,
        },
    );
    if let Some(point) = plan.removed_point {
        super::remove_safepoints(body, &BTreeSet::from([point]));
    }
}

fn remove_phi_aliases(body: &mut SnapshotBody, aliases: &BTreeSet<ValueId>) {
    let removed = body
        .blocks
        .iter()
        .map(|block| {
            block
                .parameters
                .iter()
                .enumerate()
                .filter(|(_, parameter)| aliases.contains(&parameter.id))
                .map(|(index, _)| index)
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let block_indices = body
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id, index))
        .collect::<BTreeMap<_, _>>();
    for (block, positions) in body.blocks.iter_mut().zip(&removed) {
        let mut index = 0;
        block.parameters.retain(|_| {
            let keep = !positions.contains(&index);
            index += 1;
            keep
        });
    }
    for predecessor in &mut body.blocks {
        if let Terminator::Jump { target, arguments } = &mut predecessor.terminator
            && let Some(target_index) = block_indices.get(target).copied()
        {
            let positions = &removed[target_index];
            let mut index = 0;
            arguments.retain(|_| {
                let keep = !positions.contains(&index);
                index += 1;
                keep
            });
        }
    }
}

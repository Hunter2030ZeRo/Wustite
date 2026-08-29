use std::collections::{BTreeMap, BTreeSet};

use crate::adaptive_v2::wxir_v2::ir::{
    Constant, Effect, InstructionKind, SafepointId, SnapshotBody,
};

mod cfg;
mod virtual_deopt;

pub(super) fn run(body: &mut SnapshotBody) -> bool {
    if body.blocks.iter().any(|block| {
        matches!(
            block.terminator,
            crate::adaptive_v2::wxir_v2::ir::Terminator::Backedge { .. }
                | crate::adaptive_v2::wxir_v2::ir::Terminator::IrreducibleBackedge
        )
    }) {
        return false;
    }
    let mut changed = cfg::run(body);
    let mut removed_points = BTreeSet::new();
    let virtual_points = virtual_deopt::live_points(body);
    let mut pending_virtuals = Vec::new();
    let shape = virtual_deopt::object_shape(body);
    let mut next_virtual = virtual_deopt::next_id(body);
    for block in &mut body.blocks {
        let mut remove = BTreeSet::new();
        for allocate in 0..block.instructions.len() {
            let Some(handle) = block.instructions[allocate].output.map(|output| output.id) else {
                continue;
            };
            if !matches!(
                block.instructions[allocate].kind.semantic(),
                InstructionKind::Allocate
            ) {
                continue;
            }
            let uses = block
                .instructions
                .iter()
                .enumerate()
                .filter(|(_, instruction)| instruction.inputs.contains(&handle))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if uses.is_empty() || super::terminator_uses(&block.terminator, handle) {
                continue;
            }
            let Some(last_use) = uses.last().copied() else {
                continue;
            };
            let points = virtual_points.get(&handle).cloned().unwrap_or_default();
            if !points.is_empty() && shape.is_none() {
                continue;
            }
            let point_positions = block
                .instructions
                .iter()
                .enumerate()
                .filter_map(|(index, instruction)| {
                    instruction
                        .safepoint
                        .filter(|point| points.contains(point))
                        .map(|point| (point, index))
                })
                .collect::<BTreeMap<_, _>>();
            if point_positions.len() != points.len() {
                continue;
            }
            let last_observation = point_positions.values().copied().max().unwrap_or(last_use);
            if crosses_barrier(block, allocate, last_observation.max(last_use), &uses) {
                continue;
            }
            let mut fields = BTreeMap::new();
            let mut replacements = Vec::new();
            let mut snapshots = BTreeMap::new();
            let mut supported = true;
            for index in (allocate + 1)..=last_observation.max(last_use) {
                let instruction = &block.instructions[index];
                match instruction.kind.semantic() {
                    InstructionKind::ObjectSet
                        if instruction.inputs.len() == 3 && instruction.inputs[0] == handle =>
                    {
                        fields.insert(instruction.inputs[1], instruction.inputs[2]);
                    }
                    InstructionKind::ObjectGet
                        if instruction.inputs.len() == 2 && instruction.inputs[0] == handle =>
                    {
                        if let Some(value) = fields.get(&instruction.inputs[1]).copied() {
                            replacements.push((index, value));
                        } else {
                            supported = false;
                        }
                    }
                    _ if instruction.inputs.contains(&handle) => supported = false,
                    _ => {}
                }
                if let Some(point) = instruction.safepoint.filter(|point| points.contains(point)) {
                    let Some(snapshot) = resolve_fields(block, &fields) else {
                        supported = false;
                        break;
                    };
                    snapshots.insert(point, snapshot);
                }
            }
            if !supported || replacements.is_empty() {
                continue;
            }
            if let Some(point) = block.instructions[allocate].safepoint {
                removed_points.insert(point);
            }
            for (get, value) in replacements {
                block.instructions[get].kind = InstructionKind::Copy;
                block.instructions[get].inputs = vec![value];
                block.instructions[get].effect = Effect::Pure;
                block.instructions[get].effect_sequence = None;
                block.instructions[get].safepoint = None;
            }
            remove.insert(allocate);
            remove.extend(uses.into_iter().filter(|use_index| {
                matches!(
                    block.instructions[*use_index].kind.semantic(),
                    InstructionKind::ObjectSet
                )
            }));
            if !points.is_empty() {
                pending_virtuals.push(virtual_deopt::PendingVirtual {
                    handles: BTreeSet::from([handle]),
                    id: next_virtual,
                    shape: shape.expect("live virtual object has a current shape"),
                    snapshots,
                });
                next_virtual += 1;
            }
            changed = true;
        }
        let mut index = 0;
        block.instructions.retain(|_| {
            let keep = !remove.contains(&index);
            index += 1;
            keep
        });
    }
    for pending in pending_virtuals {
        virtual_deopt::apply(body, &pending);
    }
    remove_safepoints(body, &removed_points);
    changed
}

fn resolve_fields(
    block: &crate::adaptive_v2::wxir_v2::ir::Block,
    fields: &BTreeMap<
        crate::adaptive_v2::wxir_v2::ir::ValueId,
        crate::adaptive_v2::wxir_v2::ir::ValueId,
    >,
) -> Option<Vec<(u32, crate::adaptive_v2::wxir_v2::ir::ValueId)>> {
    fields
        .iter()
        .map(|(key, value)| {
            block
                .instructions
                .iter()
                .find(|instruction| instruction.output.is_some_and(|output| output.id == *key))
                .and_then(|instruction| match instruction.kind.semantic() {
                    InstructionKind::Constant(Constant::Integer(key)) => u32::try_from(*key).ok(),
                    _ => None,
                })
                .map(|key| (key, *value))
        })
        .collect()
}

fn crosses_barrier(
    block: &crate::adaptive_v2::wxir_v2::ir::Block,
    allocate: usize,
    last_use: usize,
    uses: &[usize],
) -> bool {
    block.instructions[allocate..=last_use]
        .iter()
        .enumerate()
        .any(|(offset, instruction)| {
            offset != 0
                && (instruction.effect.is_barrier() || instruction.effect == Effect::Write)
                && !uses.contains(&(allocate + offset))
        })
}

pub(super) fn remove_safepoints(body: &mut SnapshotBody, removed: &BTreeSet<SafepointId>) {
    body.deopts
        .retain(|recipe| !removed.contains(&recipe.root_point));
    body.root_maps.retain(|map| !removed.contains(&map.point));
}

#[cfg(test)]
mod tests;

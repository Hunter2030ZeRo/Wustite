use std::collections::{BTreeMap, BTreeSet};

use crate::adaptive_v2::wxir_v2::deopt::{RegisterSource, VirtualKind, VirtualRecipe};
use crate::adaptive_v2::wxir_v2::dependency::DependencyKind;
use crate::adaptive_v2::wxir_v2::ir::{RootLocation, SafepointId, SnapshotBody, ValueId};

#[derive(Clone, Copy)]
pub(super) struct ObjectShape {
    identity: u64,
    dependency_epoch: u64,
    layout_epoch: u64,
}

pub(super) struct PendingVirtual {
    pub(super) handles: BTreeSet<ValueId>,
    pub(super) id: u32,
    pub(super) shape: ObjectShape,
    pub(super) snapshots: BTreeMap<SafepointId, Vec<(u32, ValueId)>>,
}

pub(super) fn live_points(body: &SnapshotBody) -> BTreeMap<ValueId, BTreeSet<SafepointId>> {
    let mut points = BTreeMap::<ValueId, BTreeSet<SafepointId>>::new();
    for map in &body.root_maps {
        for root in &map.roots {
            if let RootLocation::Ssa(value) = root {
                points.entry(*value).or_default().insert(map.point);
            }
        }
    }
    for recipe in &body.deopts {
        for frame in &recipe.frames {
            for register in &frame.registers {
                if let RegisterSource::Ssa(value) = register.source {
                    points.entry(value).or_default().insert(recipe.root_point);
                }
            }
        }
        for root in &recipe.explicit_roots {
            if let RootLocation::Ssa(value) = root {
                points.entry(*value).or_default().insert(recipe.root_point);
            }
        }
    }
    points
}

pub(super) fn object_shape(body: &SnapshotBody) -> Option<ObjectShape> {
    body.dependencies
        .iter()
        .find(|dependency| dependency.kind == DependencyKind::Shape && dependency.is_current())
        .map(|dependency| ObjectShape {
            identity: dependency.identity,
            dependency_epoch: dependency.expected_epoch,
            layout_epoch: dependency.expected_epoch,
        })
}

pub(super) fn next_id(body: &SnapshotBody) -> u32 {
    body.deopts
        .iter()
        .flat_map(|recipe| recipe.virtuals.iter().map(|virtual_| virtual_.id))
        .max()
        .map_or(0, |id| id.saturating_add(1))
}

pub(super) fn apply(body: &mut SnapshotBody, pending: &PendingVirtual) {
    for recipe in &mut body.deopts {
        let Some(fields) = pending.snapshots.get(&recipe.root_point) else {
            continue;
        };
        for frame in &mut recipe.frames {
            for register in &mut frame.registers {
                if matches!(register.source, RegisterSource::Ssa(value) if pending.handles.contains(&value))
                {
                    register.source = RegisterSource::Virtual(pending.id);
                }
            }
        }
        replace_roots(&mut recipe.explicit_roots, &pending.handles, pending.id);
        recipe.virtuals.push(VirtualRecipe {
            id: pending.id,
            kind: VirtualKind::Object {
                shape_identity: pending.shape.identity,
                shape_dependency_epoch: pending.shape.dependency_epoch,
                shape_layout_epoch: pending.shape.layout_epoch,
                fields: fields
                    .iter()
                    .map(|(key, value)| (*key, RegisterSource::Ssa(*value)))
                    .collect(),
            },
        });
    }
    for map in &mut body.root_maps {
        if !pending.snapshots.contains_key(&map.point) {
            continue;
        }
        let replaced = pending
            .handles
            .iter()
            .any(|handle| map.roots.remove(&RootLocation::Ssa(*handle)));
        if replaced {
            map.roots.insert(RootLocation::Virtual(pending.id));
            map.roots.insert(RootLocation::DeoptWorklist);
        }
    }
}

fn replace_roots(roots: &mut [RootLocation], handles: &BTreeSet<ValueId>, virtual_id: u32) {
    for root in roots {
        if matches!(*root, RootLocation::Ssa(handle) if handles.contains(&handle)) {
            *root = RootLocation::Virtual(virtual_id);
        }
    }
}

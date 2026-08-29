use std::collections::{BTreeMap, BTreeSet};

use super::super::SnapshotError;
use super::super::deopt::RegisterSource;
use super::super::ir::{InstructionKind, RootLocation, SnapshotBody, Terminator};
use super::DefinitionMap;

pub(super) fn verify(
    body: &SnapshotBody,
    definitions: &DefinitionMap,
) -> Result<(), SnapshotError> {
    let mut deopts = BTreeMap::new();
    for recipe in &body.deopts {
        if deopts.insert(recipe.id, recipe).is_some() {
            return Err(SnapshotError::DuplicateDeopt { id: recipe.id });
        }
        if recipe.frames.is_empty()
            || recipe.executable != body.executable
            || recipe.dependencies != body.dependencies
            || recipe.explicit_roots.iter().any(|root| {
                matches!(
                    root,
                    RootLocation::Ssa(_)
                        | RootLocation::Spill(_)
                        | RootLocation::Virtual(_)
                        | RootLocation::DeoptWorklist
                )
            })
        {
            return Err(SnapshotError::InvalidDeopt { id: recipe.id });
        }
        let virtual_ids = recipe
            .virtuals
            .iter()
            .map(|item| item.id)
            .collect::<BTreeSet<_>>();
        if virtual_ids.len() != recipe.virtuals.len() {
            return Err(SnapshotError::InvalidDeopt { id: recipe.id });
        }
        for virtual_recipe in &recipe.virtuals {
            for source in virtual_sources(&virtual_recipe.kind) {
                match source {
                    RegisterSource::Ssa(value) if !definitions.contains_key(value) => {
                        return Err(SnapshotError::InvalidDeopt { id: recipe.id });
                    }
                    RegisterSource::Virtual(id) if !virtual_ids.contains(id) => {
                        return Err(SnapshotError::InvalidDeopt { id: recipe.id });
                    }
                    RegisterSource::UndefinedDead
                    | RegisterSource::Constant(
                        super::super::ir::Constant::UndefinedDead
                        | super::super::ir::Constant::HandleBits(_),
                    ) => {
                        return Err(SnapshotError::InvalidDeopt { id: recipe.id });
                    }
                    RegisterSource::Ssa(_)
                    | RegisterSource::Constant(_)
                    | RegisterSource::Spill { .. }
                    | RegisterSource::Virtual(_) => {}
                }
            }
        }
        for frame in &recipe.frames {
            let mut registers = BTreeSet::new();
            let mut undefined = BTreeSet::new();
            for register in &frame.registers {
                if !registers.insert(register.register) {
                    return Err(SnapshotError::InvalidDeopt { id: recipe.id });
                }
                if let RegisterSource::Ssa(value) = register.source
                    && definitions.get(&value).map(|definition| definition.2) != Some(register.ty)
                {
                    return Err(SnapshotError::InvalidDeopt { id: recipe.id });
                }
                if matches!(
                    register.source,
                    RegisterSource::Constant(
                        super::super::ir::Constant::HandleBits(_)
                            | super::super::ir::Constant::UndefinedDead
                    )
                ) {
                    return Err(SnapshotError::InvalidDeopt { id: recipe.id });
                }
                if matches!(register.source, RegisterSource::UndefinedDead) {
                    undefined.insert(register.register);
                }
            }
            if undefined != frame.dead_registers {
                return Err(SnapshotError::InvalidDeopt { id: recipe.id });
            }
            if registers
                .iter()
                .enumerate()
                .any(|(index, register)| usize::from(*register) != index)
            {
                return Err(SnapshotError::InvalidDeopt { id: recipe.id });
            }
        }
    }
    let mut required = BTreeSet::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            match instruction.kind.semantic() {
                InstructionKind::Guard { guard } => {
                    required.insert(*guard);
                }
                InstructionKind::BranchGuard { side_exit, .. } => {
                    required.insert(*side_exit);
                }
                _ => {}
            }
            if let Some(point) = instruction.safepoint
                && !body.deopts.iter().any(|recipe| recipe.root_point == point)
            {
                return Err(SnapshotError::MissingDeopt { id: point.get() });
            }
        }
        if let Terminator::SideExit { id, .. } = block.terminator {
            required.insert(id);
        }
        if let Terminator::Backedge { safepoint, .. } = block.terminator
            && !body
                .deopts
                .iter()
                .any(|recipe| recipe.root_point == safepoint)
        {
            return Err(SnapshotError::MissingDeopt {
                id: safepoint.get(),
            });
        }
    }
    for id in required {
        if !deopts.contains_key(&id) {
            return Err(SnapshotError::MissingDeopt { id });
        }
    }
    verify_root_maps(body, definitions)
}

fn virtual_sources(kind: &super::super::deopt::VirtualKind) -> Vec<&RegisterSource> {
    match kind {
        super::super::deopt::VirtualKind::Object { fields, .. } => {
            fields.iter().map(|(_, source)| source).collect()
        }
        super::super::deopt::VirtualKind::List { items }
        | super::super::deopt::VirtualKind::Tuple { items } => items.iter().collect(),
    }
}

fn verify_root_maps(body: &SnapshotBody, definitions: &DefinitionMap) -> Result<(), SnapshotError> {
    let mut maps = BTreeMap::new();
    for map in &body.root_maps {
        if maps.insert(map.point, map).is_some() {
            return Err(SnapshotError::DuplicateRootMap {
                point: map.point.get(),
            });
        }
    }
    for recipe in &body.deopts {
        let map = maps
            .get(&recipe.root_point)
            .ok_or(SnapshotError::MissingRootMap {
                point: recipe.root_point.get(),
            })?;
        let mut expected = recipe
            .explicit_roots
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        expected.extend(
            body.blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter_map(|instruction| match instruction.kind.semantic() {
                    InstructionKind::OwnedList { identity, .. } => {
                        Some(RootLocation::OwnedList(*identity))
                    }
                    _ => None,
                }),
        );
        for frame in &recipe.frames {
            for register in &frame.registers {
                match register.source {
                    RegisterSource::Ssa(value)
                        if definitions
                            .get(&value)
                            .is_some_and(|definition| definition.2.is_handle()) =>
                    {
                        expected.insert(RootLocation::Ssa(value));
                    }
                    RegisterSource::Spill { ty, .. } if ty != register.ty => {
                        return Err(SnapshotError::InvalidDeopt { id: recipe.id });
                    }
                    RegisterSource::Spill { slot, ty } if ty.is_handle() => {
                        expected.insert(RootLocation::Spill(slot));
                    }
                    RegisterSource::Virtual(id) if register.ty.is_handle() => {
                        expected.insert(RootLocation::Virtual(id));
                    }
                    RegisterSource::Ssa(_)
                    | RegisterSource::Constant(_)
                    | RegisterSource::Spill { .. }
                    | RegisterSource::Virtual(_)
                    | RegisterSource::UndefinedDead => {}
                }
            }
        }
        if !recipe.virtuals.is_empty() {
            expected.insert(RootLocation::DeoptWorklist);
            expected.extend(
                recipe
                    .virtuals
                    .iter()
                    .map(|item| RootLocation::Virtual(item.id)),
            );
            for source in recipe
                .virtuals
                .iter()
                .flat_map(|item| virtual_sources(&item.kind))
            {
                if let RegisterSource::Ssa(value) = source
                    && definitions
                        .get(value)
                        .is_some_and(|definition| definition.2.is_handle())
                {
                    expected.insert(RootLocation::Ssa(*value));
                }
                if let RegisterSource::Spill { slot, ty } = source
                    && ty.is_handle()
                {
                    expected.insert(RootLocation::Spill(*slot));
                }
            }
        }
        if expected.difference(&map.roots).next().is_some() {
            return Err(SnapshotError::MissingRoot {
                point: recipe.root_point.get(),
            });
        }
        if map.roots.difference(&expected).next().is_some() {
            return Err(SnapshotError::SurplusRoot {
                point: recipe.root_point.get(),
            });
        }
    }
    if let Some(point) = maps.keys().find(|point| {
        !body
            .deopts
            .iter()
            .any(|recipe| recipe.root_point == **point)
    }) {
        return Err(SnapshotError::SurplusRoot { point: point.get() });
    }
    Ok(())
}

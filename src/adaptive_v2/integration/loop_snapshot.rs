use crate::executable::ExecutableFunction;
use crate::structure_map::{RegionId, RegionKind};
use crate::value::Value;
use crate::{bytecode::Instruction, bytecode::Register};

use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity, LoopPreheader};
use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::adaptive_v2::wxir_v2::ir::{BlockId, SnapshotDraft};
use std::collections::BTreeMap;

mod cfg;

#[derive(Clone)]
pub(super) struct CallTarget {
    pub(super) function: ExecutableFunction,
    pub(super) handle: u32,
    pub(super) argument_element_paths: Vec<Vec<crate::adaptive_v2::wxir_v2::ir::ValueType>>,
    pub(super) argument_indexed_element_types:
        Vec<std::collections::BTreeMap<Vec<usize>, crate::adaptive_v2::wxir_v2::ir::ValueType>>,
}

pub(super) type ConstantCallTargets = BTreeMap<(u64, Register), CallTarget>;

pub(super) struct PreparedLoop {
    pub(super) values: Vec<(Register, Value)>,
    pub(super) prefix: (usize, usize),
}

pub(super) fn prepared_value_is_live(
    code: &[Instruction],
    start: usize,
    register: Register,
) -> bool {
    cfg::prepared_value_is_live(code, start, register)
}

pub(super) fn verified_preheader_entry(
    executable: &ExecutableFunction,
    region_id: RegionId,
) -> Option<LoopPreheader> {
    let region = executable.structure_map().region(region_id)?;
    if !matches!(region.kind, RegionKind::Loop { .. }) || region.entry == 0 {
        return None;
    }
    let header = executable.structure_map().block_by_pc(region.entry)?;
    if header.start_pc != region.entry {
        return None;
    }
    let mut branches = executable.bytecode().code[header.start_pc..header.end_pc]
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::Branch { yes, no, .. } => Some((*yes, *no)),
            _ => None,
        });
    let (body_pc, exit_pc) = branches.next()?;
    if branches.next().is_some() {
        return None;
    }
    let region_starts = region
        .blocks
        .iter()
        .filter_map(|block| executable.structure_map().block(*block))
        .map(|block| block.start_pc)
        .collect::<std::collections::BTreeSet<_>>();
    if !region_starts.contains(&body_pc) || region_starts.contains(&exit_pc) {
        return None;
    }
    let edge_pc = region.entry.checked_sub(1)?;
    let edge_block = executable.structure_map().block_by_pc(edge_pc)?;
    if region.blocks.contains(&edge_block.id)
        || edge_block.end_pc != region.entry
        || !matches!(
            executable.bytecode().code.get(edge_pc),
            Some(Instruction::Jump { target }) if *target == body_pc
        )
    {
        return None;
    }
    let body = executable.structure_map().block_by_pc(body_pc)?;
    if body.start_pc != body_pc
        || !edge_block
            .successors
            .iter()
            .any(|edge| edge.target == body.id && edge.kind == crate::structure_map::EdgeKind::Jump)
    {
        return None;
    }
    Some(LoopPreheader {
        edge_pc: pc(edge_pc, "preheader").ok()?,
        body_pc: pc(body_pc, "body entry").ok()?,
    })
}

pub(super) fn storage_live_destinations(
    executable: &ExecutableFunction,
    region_id: RegionId,
) -> Vec<Register> {
    let Some(region) = executable.structure_map().region(region_id) else {
        return Vec::new();
    };
    let ordinary = region
        .entry_summary
        .iter()
        .map(|slot| slot.register)
        .collect::<std::collections::BTreeSet<_>>();
    region
        .blocks
        .iter()
        .filter_map(|block| executable.structure_map().block(*block))
        .flat_map(|block| {
            executable.bytecode().code[block.start_pc..block.end_pc]
                .windows(2)
                .filter_map(|pair| match pair {
                    [
                        Instruction::Call { dst, .. },
                        Instruction::Move { dst: target, src },
                    ] if dst == src && !ordinary.contains(target) => Some(*target),
                    _ => None,
                })
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn loop_draft(
    executable: &ExecutableFunction,
    region_id: RegionId,
    inputs: &[Value],
    prepared: Option<&PreparedLoop>,
    element_types: &BTreeMap<u16, crate::adaptive_v2::wxir_v2::ir::ValueType>,
    call_targets: &BTreeMap<u16, CallTarget>,
    constant_call_targets: &ConstantCallTargets,
    schema_epoch: u64,
) -> Result<SnapshotDraft, String> {
    let region = executable
        .structure_map()
        .region(region_id)
        .ok_or_else(|| "adaptive-v2 loop region is missing".to_owned())?;
    let RegionKind::Loop { backedge } = region.kind else {
        return Err("adaptive-v2 OSR entry is not a loop header".to_owned());
    };
    let epoch = executable.id().as_u64();
    let identity = ExecutableIdentity::new(epoch, epoch);
    let dependencies = dependencies(executable, region, call_targets, schema_epoch);
    let lowered = cfg::lower(
        executable,
        region_id,
        region,
        backedge,
        inputs,
        prepared,
        element_types,
        call_targets,
        constant_call_targets,
        identity,
        &dependencies,
    )?;
    Ok(SnapshotDraft::new(
        identity,
        EntryKind::LoopHeader {
            header_pc: pc(region.entry, "header")?,
            backedge_pc: pc(backedge, "backedge")?,
            preheader: verified_preheader_entry(executable, region_id),
        },
        BlockId::new(0),
        lowered.blocks,
        lowered.root_maps,
        lowered.deopts,
        dependencies,
    )
    .with_schema_epoch(schema_epoch))
}

fn pc(value: usize, name: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("adaptive-v2 loop {name} pc overflow"))
}

fn dependencies(
    executable: &ExecutableFunction,
    region: &crate::structure_map::Region,
    call_targets: &BTreeMap<u16, CallTarget>,
    schema_epoch: u64,
) -> Vec<Dependency> {
    let epoch = executable.id().as_u64();
    let mut dependencies = vec![
        Dependency::current(DependencyKind::Executable, epoch, epoch),
        Dependency::current(DependencyKind::Schema, epoch, schema_epoch),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
        Dependency::current(DependencyKind::ListLayout, epoch, 1),
    ];
    let loaded_constants = region
        .blocks
        .iter()
        .filter_map(|block| executable.structure_map().block(*block))
        .flat_map(|block| &executable.bytecode().code[block.start_pc..block.end_pc])
        .filter_map(|instruction| match instruction {
            crate::bytecode::Instruction::LoadConstant { constant, .. } => Some(constant.0),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    for constant in loaded_constants {
        if let Some(crate::executable::ExecutableConstant::Function(callee)) =
            executable.constants().get(constant)
        {
            dependencies.push(Dependency::current(
                DependencyKind::Callee,
                callee.id().as_u64(),
                callee.id().as_u64(),
            ));
        }
    }
    for target in call_targets.values() {
        let callee = &target.function;
        dependencies.push(Dependency::current(
            DependencyKind::Callee,
            callee.id().as_u64(),
            callee.id().as_u64(),
        ));
        for constant in callee.constants() {
            if let crate::executable::ExecutableConstant::Function(nested) = constant {
                dependencies.push(Dependency::current(
                    DependencyKind::Callee,
                    nested.id().as_u64(),
                    nested.id().as_u64(),
                ));
            }
        }
    }
    dependencies.sort();
    dependencies.dedup();
    dependencies
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{loop_draft, verified_preheader_entry};
    use crate::adaptive_v2::wxir_v2::deopt::RegisterSource;
    use crate::{ExecutionMode, Runtime, RuntimeConfig};

    #[test]
    fn loop_cfg_tracks_scalar_backedge() {
        // Given: a divergent scalar loop with one natural backedge.
        let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
            execution_mode: ExecutionMode::AdaptiveJit,
            hot_threshold: 1,
        });
        let executable = runtime
            .compile_function(
                include_str!("../../../tests/fixtures/adaptive_loop_branch.py"),
                "main",
            )
            .unwrap();
        let (region_id, region) = executable.structure_map().loop_regions().next().unwrap();
        let live = region
            .entry_summary
            .iter()
            .map(|slot| slot.register)
            .collect::<BTreeSet<_>>();

        // When: the WVM region is lowered into an immutable WXIR draft.
        let inputs = region
            .entry_summary
            .iter()
            .map(|slot| match slot.ty {
                crate::structure_map::SlotType::SmallInt => crate::value::Value::SmallInt(0),
                crate::structure_map::SlotType::Float => crate::value::Value::Float(0.0),
                crate::structure_map::SlotType::Bool => crate::value::Value::Bool(false),
                crate::structure_map::SlotType::Object(_) | crate::structure_map::SlotType::Any => {
                    crate::value::Value::Uninitialized
                }
            })
            .collect::<Vec<_>>();
        let draft = loop_draft(
            &executable,
            region_id,
            &inputs,
            None,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
            1,
        )
        .unwrap();

        // Then: each scalar backedge has one empty root map and exact live/dead recipes.
        assert!(draft.body.blocks.len() > 3);
        assert!(!draft.body.deopts.is_empty());
        assert_eq!(draft.body.root_maps.len(), draft.body.deopts.len());
        assert!(draft.body.root_maps.iter().all(|map| map.roots.is_empty()));
        for recipe in &draft.body.deopts {
            let frame = &recipe.frames[0];
            assert!(frame.registers.iter().all(|register| {
                live.contains(&register.register)
                    == matches!(register.source, RegisterSource::Ssa(_))
            }));
        }
    }

    #[test]
    fn production_loop_declares_only_cfg_verified_preheader_body_edge() {
        // Given: the production spectral outer loop has an initial edge into its body.
        let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
            execution_mode: ExecutionMode::AdaptiveJit,
            hot_threshold: 1,
        });
        let executable = runtime
            .compile_function(include_str!("../../../examples/spectral_norm.py"), "main")
            .unwrap();
        let (region_id, _) = executable
            .structure_map()
            .loop_regions()
            .find(|(_, region)| region.entry == 11)
            .unwrap();

        // When: the alternate entry is derived from the authoritative CFG.
        let preheader = verified_preheader_entry(&executable, region_id).unwrap();

        // Then: only the real pc10-to-pc13 edge is declared for the pc11 snapshot.
        assert_eq!(preheader.edge_pc, 10);
        assert_eq!(preheader.body_pc, 13);
        assert!(!preheader.matches(9, 13));
        assert!(!preheader.matches(10, 12));
    }
}

use std::cmp::Reverse;

use crate::profiler::{Profile, ReadyRegionProfile};
use crate::structure_map::{
    BlockId, RegionExit, RegionId, RegionKind, RegionSummary, StateSlot, StructureMap,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitPolicy {
    Profile,
    StructureMap,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RegionPlanRequest {
    pub policy: JitPolicy,
    pub threshold: u64,
    pub region_id: RegionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitPlan {
    pub region_id: RegionId,
    pub header: usize,
    pub backedge: usize,
    pub blocks: Vec<BlockId>,
    pub exits: Vec<RegionExit>,
    pub live_slots: Vec<StateSlot>,
    pub summary: RegionSummary,
}

pub fn select_hot_loop(
    structure_map: &StructureMap,
    profile: &Profile,
    threshold: u64,
) -> Option<JitPlan> {
    structure_map
        .regions()
        .iter()
        .enumerate()
        .filter_map(|(index, region)| match region.kind {
            RegionKind::Loop { backedge } if profile.is_hot(RegionId(index), threshold) => {
                Some((index, region, backedge))
            }
            RegionKind::Loop { .. } | RegionKind::Branch => None,
        })
        .max_by_key(|(index, region, _)| {
            (
                profile.entry_count(RegionId(*index)),
                Reverse(region.summary.optimization_penalty()),
            )
        })
        .map(|(index, region, backedge)| JitPlan {
            region_id: RegionId(index),
            header: region.entry,
            backedge,
            blocks: region.blocks.clone(),
            exits: region.exits.clone(),
            live_slots: region.entry_summary.clone(),
            summary: region.summary,
        })
}

pub(crate) fn plan_region(
    structure_map: &StructureMap,
    profile: ReadyRegionProfile<'_>,
    request: RegionPlanRequest,
) -> Option<JitPlan> {
    if profile.region_id() != request.region_id {
        return None;
    }
    let region = structure_map.region(request.region_id)?;
    let RegionKind::Loop { backedge } = region.kind else {
        return None;
    };
    let eligible = match request.policy {
        JitPolicy::Profile => profile
            .profile()
            .is_hot(request.region_id, request.threshold),
        JitPolicy::StructureMap => {
            profile
                .profile()
                .is_hot(request.region_id, request.threshold)
                && region.summary.is_compiler_usable()
        }
    };
    eligible.then(|| JitPlan {
        region_id: request.region_id,
        header: region.entry,
        backedge,
        blocks: region.blocks.clone(),
        exits: region.exits.clone(),
        live_slots: region.entry_summary.clone(),
        summary: region.summary,
    })
}

/// Builds a plan for one specific region after it reaches the hot threshold.
pub fn plan_hot_region(
    structure_map: &StructureMap,
    profile: &Profile,
    threshold: u64,
    region_id: RegionId,
) -> Option<JitPlan> {
    let ready = profile.ready_region(structure_map, region_id, threshold)?;
    plan_region(
        structure_map,
        ready,
        RegionPlanRequest {
            policy: JitPolicy::Profile,
            threshold,
            region_id,
        },
    )
}

#[cfg(test)]
mod tests;

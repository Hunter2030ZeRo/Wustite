use crate::profiler::Profile;
use crate::structure_map::{
    BlockId, RegionExit, RegionId, RegionKind, RegionSummary, StateSlot, StructureMap,
};

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
        .max_by_key(|(index, _, _)| profile.entry_count(RegionId(*index)))
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

/// Builds a plan for one specific region after it reaches the hot threshold.
pub fn plan_hot_region(
    structure_map: &StructureMap,
    profile: &Profile,
    threshold: u64,
    region_id: RegionId,
) -> Option<JitPlan> {
    let region = structure_map.region(region_id)?;
    let RegionKind::Loop { backedge } = region.kind else {
        return None;
    };
    profile.is_hot(region_id, threshold).then(|| JitPlan {
        region_id,
        header: region.entry,
        backedge,
        blocks: region.blocks.clone(),
        exits: region.exits.clone(),
        live_slots: region.entry_summary.clone(),
        summary: region.summary,
    })
}

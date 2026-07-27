use crate::profiler::Profile;
use crate::structure_map::{LiveSlot, RegionExit, RegionId, StructureMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitPlan {
    pub region_id: RegionId,
    pub header: usize,
    pub backedge: usize,
    pub exits: Vec<RegionExit>,
    pub live_slots: Vec<LiveSlot>,
}

pub fn select_hot_loop(
    structure_map: &StructureMap,
    profile: &Profile,
    threshold: u64,
) -> Option<JitPlan> {
    structure_map
        .loops
        .iter()
        .enumerate()
        .filter(|(index, _)| profile.is_hot(RegionId(*index), threshold))
        .max_by_key(|(index, _)| profile.entry_count(RegionId(*index)))
        .map(|(index, region)| JitPlan {
            region_id: RegionId(index),
            header: region.header,
            backedge: region.backedge,
            exits: region.exits.clone(),
            live_slots: region.live_slots.clone(),
        })
}

/// Builds a plan for one specific region after it reaches the hot threshold.
pub fn plan_hot_region(
    structure_map: &StructureMap,
    profile: &Profile,
    threshold: u64,
    region_id: RegionId,
) -> Option<JitPlan> {
    let region = structure_map.loops.get(region_id.0)?;
    profile.is_hot(region_id, threshold).then(|| JitPlan {
        region_id,
        header: region.header,
        backedge: region.backedge,
        exits: region.exits.clone(),
        live_slots: region.live_slots.clone(),
    })
}

use crate::profiler::Profile;
use crate::structure_map::{LiveSlot, RegionExit, StructureMap, RegionID};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitPlan {
    pub region_id: RegionID,
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
        .filter(|(_, region)| profile.is_hot(region.header, threshold))
        .max_by_key(|(_, region)| profile.count(region.header))
        .map(|(index, region)| JitPlan {
            region_id: RegionID(index as u32),
            header: region.header,
            backedge: region.backedge,
            exits: region.exits.clone(),
            live_slots: region.live_slots.clone(),
        })
}

use crate::profiler::Profile;
use crate::structure_map::{LiveSlot, RegionExit, StructureMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitPlan {
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
        .filter(|region| profile.is_hot(region.header, threshold))
        .max_by_key(|region| profile.count(region.header))
        .map(|region| JitPlan {
            header: region.header,
            backedge: region.backedge,
            exits: region.exits.clone(),
            live_slots: region.live_slots.clone(),
        })
}

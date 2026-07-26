use crate::bytecode::Register;
use crate::profiler::Profile;
use crate::structure_map::StructureMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitPlan {
    pub header: usize, 
    pub backedge: usize, 
    pub exit: usize, 
    pub live_registers: Vec<Register>,
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
            exit: region.exit, 
            live_registers: region.live_registers.clone(),
        })
}
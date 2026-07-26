use crate::bytecode::Register;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotType {
    I64,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSlot {
    pub register: Register,
    pub ty: SlotType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionExit {
    pub target: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopRegion {
    pub header: usize,
    pub backedge: usize,
    pub exits: Vec<RegionExit>,
    pub live_slots: Vec<LiveSlot>,
}

#[derive(Debug, Clone, Default)]
pub struct StructureMap {
    pub loops: Vec<LoopRegion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionID(pub u32);

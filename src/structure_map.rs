use crate::bytecode::Register;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopRegion {
    pub header: usize, 
    pub backedge: usize, 
    pub exit: usize, 
    pub live_registers: Vec<Register>,
}

#[derive(Debug, Clone, Default)]
pub struct StructureMap {
    pub loops: Vec<LoopRegion>,
}
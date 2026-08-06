use crate::bytecode::Register;
use crate::object::ObjectKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotType {
    SmallInt,
    Float,
    Bool,
    Object(ObjectKind),
    Any,
}

/// Stable identifier for one semantic operation site in WVM bytecode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationSiteId(pub u32);

/// A statically proven or currently unknown fact about a WVM value.
///
/// Runtime observations belong to Profile rather than the immutable
/// StructureMap. Only facts established while lowering or verifying the
/// executable may be stored here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFact {
    Unknown,
    Exact(SlotType),
}

/// Static facts associated with one generic WVM operation.
///
/// The bytecode retains source-language semantics while this side table keeps
/// facts that can later justify quickening or typed WXIR specialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSite {
    pub pc: usize,
    pub lhs: TypeFact,
    pub rhs: TypeFact,
    pub result: TypeFact,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructureMap {
    pub loops: Vec<LoopRegion>,
    pub operation_sites: Vec<OperationSite>,
}

impl StructureMap {
    pub fn operation_site(&self, id: OperationSiteId) -> Option<&OperationSite> {
        self.operation_sites.get(id.0 as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Stable identifier for a WVM region in a StructureMap.
pub struct RegionId(pub usize);

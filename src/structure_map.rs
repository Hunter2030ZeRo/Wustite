use crate::bytecode::{Instruction, Register};
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
pub struct StateSlot {
    pub register: Register,
    pub ty: SlotType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionExit {
    pub target: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEdge {
    pub target: BlockId,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Fallthrough,
    Jump,
    BranchTrue,
    BranchFalse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub id: BlockId,

    pub start_pc: usize,
    pub end_pc: usize,

    pub successors: Vec<BlockEdge>,
    pub predecessors: Vec<BlockId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Loop { backedge: usize },
    Branch,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RegionSummary {
    pub instruction_count: usize,
    pub block_count: usize,
    pub operation_count: usize,
    pub call_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub kind: RegionKind,

    pub entry: usize,
    pub blocks: Vec<BlockId>,

    pub exits: Vec<RegionExit>,

    pub entry_summary: Vec<StateSlot>,
    pub summary: RegionSummary,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StructureMap {
    blocks: Vec<BasicBlock>,
    regions: Vec<Region>,
    operation_sites: Vec<OperationSite>,

    block_by_pc: Vec<BlockId>,
    region_by_entry_pc: Vec<Option<RegionId>>,
}

impl StructureMap {
    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    pub fn block(&self, id: BlockId) -> Option<&BasicBlock> {
        self.blocks.get(id.0 as usize)
    }

    pub fn region(&self, id: RegionId) -> Option<&Region> {
        self.regions.get(id.0)
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn operation_site(&self, id: OperationSiteId) -> Option<&OperationSite> {
        self.operation_sites.get(id.0 as usize)
    }

    pub fn operation_sites(&self) -> &[OperationSite] {
        &self.operation_sites
    }

    pub fn block_by_pc(&self, pc: usize) -> Option<&BasicBlock> {
        self.block_by_pc.get(pc).and_then(|id| self.block(*id))
    }

    pub fn region_by_entry_pc(&self, pc: usize) -> Option<RegionId> {
        self.region_by_entry_pc.get(pc).copied().flatten()
    }

    pub fn loop_regions(&self) -> impl Iterator<Item = (RegionId, &Region)> {
        self.regions
            .iter()
            .enumerate()
            .filter(|(_, region)| matches!(region.kind, RegionKind::Loop { .. }))
            .map(|(id, region)| (RegionId(id), region))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Stable identifier for a WVM region in a StructureMap.
pub struct RegionId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegionDraft {
    entry: usize,
    entry_summary: Vec<StateSlot>,
    completion: Option<(RegionKind, Vec<RegionExit>)>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct StructureMapBuilder {
    operation_sites: Vec<OperationSite>,
    regions: Vec<RegionDraft>,
}

impl StructureMapBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_operation(
        &mut self,
        pc: usize,
        lhs: TypeFact,
        rhs: TypeFact,
        result: TypeFact,
    ) -> Result<OperationSiteId, String> {
        let id = u32::try_from(self.operation_sites.len())
            .map_err(|_| "StructureMap contains too many operation sites".to_string())?;
        self.operation_sites.push(OperationSite {
            pc,
            lhs,
            rhs,
            result,
        });
        Ok(OperationSiteId(id))
    }

    pub fn begin_region(&mut self, entry: usize, entry_summary: Vec<StateSlot>) -> RegionId {
        let id = RegionId(self.regions.len());
        self.regions.push(RegionDraft {
            entry,
            entry_summary,
            completion: None,
        });
        id
    }

    pub fn finish_region(
        &mut self,
        id: RegionId,
        kind: RegionKind,
        exits: Vec<RegionExit>,
    ) -> Result<(), String> {
        let draft = self
            .regions
            .get_mut(id.0)
            .ok_or_else(|| format!("unknown region {}", id.0))?;
        if draft.completion.is_some() {
            return Err(format!("region {} is already finished", id.0));
        }
        draft.completion = Some((kind, exits));
        Ok(())
    }

    pub fn finish(
        self,
        code: &[Instruction],
        register_count: usize,
    ) -> Result<StructureMap, String> {
        builder::finish(self, code, register_count)
    }
}

mod builder;

#[cfg(test)]
mod tests;

use crate::bytecode::Register;

use super::{EffectSummary, Fact, SlotType};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionSummary {
    pub instruction_count: usize,
    pub block_count: usize,
    pub operation_count: usize,
    pub call_count: usize,
    pub effects: Fact<EffectSummary>,
    pub escaping_allocation_count: usize,
    pub virtualizable_allocation_count: usize,
    pub failure_site_count: usize,
    pub guardable_fact_count: usize,
}

impl Default for RegionSummary {
    fn default() -> Self {
        Self {
            instruction_count: 0,
            block_count: 0,
            operation_count: 0,
            call_count: 0,
            effects: Fact::Proven(EffectSummary::default()),
            escaping_allocation_count: 0,
            virtualizable_allocation_count: 0,
            failure_site_count: 0,
            guardable_fact_count: 0,
        }
    }
}

impl RegionSummary {
    pub fn optimization_penalty(self) -> usize {
        let certainty = match self.effects {
            Fact::Proven(_) => 0usize,
            Fact::Guardable(_) => 32,
            Fact::Unknown => 64,
        };
        let effects = self.effects.candidate().copied().unwrap_or_default();
        certainty
            .saturating_add(usize::from(effects.may_mutate) * 4)
            .saturating_add(usize::from(effects.may_allocate) * 2)
            .saturating_add(usize::from(effects.may_call_unknown) * 16)
            .saturating_add(usize::from(effects.may_access_global_state) * 8)
            .saturating_add(self.escaping_allocation_count * 8)
            .saturating_add(self.failure_site_count)
            .saturating_add(self.guardable_fact_count * 2)
    }

    pub const fn is_compiler_usable(self) -> bool {
        !matches!(self.effects, Fact::Unknown)
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RegionDraft {
    pub entry: usize,
    pub entry_summary: Vec<StateSlot>,
    pub completion: Option<(RegionKind, Vec<RegionExit>)>,
}

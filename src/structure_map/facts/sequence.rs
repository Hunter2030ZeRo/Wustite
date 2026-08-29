use crate::object::SequenceStrategy;

use super::{Fact, TypeFact, ValueId};
use crate::structure_map::BlockId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceKind {
    List,
    Tuple,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceMutability {
    Mutable,
    Immutable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceFacts {
    pub kind: Fact<SequenceKind>,
    pub strategy: Fact<SequenceStrategy>,
    pub element_type: TypeFact,
    pub exact_length: Fact<usize>,
    pub mutability: Fact<SequenceMutability>,
    pub layout_stable: Fact<bool>,
}

impl SequenceFacts {
    pub const fn unknown() -> Self {
        Self {
            kind: Fact::Unknown,
            strategy: Fact::Unknown,
            element_type: Fact::Unknown,
            exact_length: Fact::Unknown,
            mutability: Fact::Unknown,
            layout_stable: Fact::Unknown,
        }
    }
}

impl Default for SequenceFacts {
    fn default() -> Self {
        Self::unknown()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    Content,
    Layout,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationEffect {
    pub identity_root: ValueId,
    pub kind: MutationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardPlacement {
    RegionEntry,
    BlockEntry(BlockId),
    AccessSite(usize),
}

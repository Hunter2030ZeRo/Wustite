use serde::{Deserialize, Serialize};

use super::deopt::DeoptRecipe;
use super::dependency::Dependency;
use super::ir::{Block, BlockId, RootMap, WxIrAbi};
use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SnapshotDraft {
    pub(crate) body: SnapshotBody,
}

impl SnapshotDraft {
    pub(crate) fn new(
        executable: ExecutableIdentity,
        entry_kind: EntryKind,
        entry: BlockId,
        blocks: Vec<Block>,
        root_maps: Vec<RootMap>,
        deopts: Vec<DeoptRecipe>,
        dependencies: Vec<Dependency>,
    ) -> Self {
        Self {
            body: SnapshotBody {
                abi: WxIrAbi::V2,
                executable,
                schema_epoch: 0,
                entry_kind,
                entry,
                parent: None,
                blocks,
                root_maps,
                deopts,
                dependencies,
            },
        }
    }

    pub(crate) fn with_schema_epoch(mut self, schema_epoch: u64) -> Self {
        self.body.schema_epoch = schema_epoch;
        self
    }

    pub(crate) fn verify(&self) -> Result<(), super::SnapshotError> {
        super::verifier::verify(&self.body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SnapshotBody {
    pub(crate) abi: WxIrAbi,
    pub(crate) executable: ExecutableIdentity,
    pub(crate) schema_epoch: u64,
    pub(crate) entry_kind: EntryKind,
    pub(crate) entry: BlockId,
    pub(crate) parent: Option<(super::SnapshotId, u32, u8)>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) root_maps: Vec<RootMap>,
    pub(crate) deopts: Vec<DeoptRecipe>,
    pub(crate) dependencies: Vec<Dependency>,
}

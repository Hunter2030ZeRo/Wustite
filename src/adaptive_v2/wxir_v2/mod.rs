pub(crate) mod deopt;
pub(crate) mod dependency;
pub(crate) mod ir;
pub(crate) mod materialize;
pub(crate) mod replay;
mod seal;
mod snapshot;
pub(crate) mod verifier;

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use self::ir::{SnapshotBody, SnapshotDraft, WxIrAbi};
use super::profile::CompilePermit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct SnapshotId([u8; 32]);

impl SnapshotId {
    pub(crate) const fn as_bytes(self) -> [u8; 32] {
        self.0
    }

    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedSnapshot {
    id: SnapshotId,
    body: Arc<SnapshotBody>,
}

impl VerifiedSnapshot {
    pub(crate) fn seal(draft: SnapshotDraft, permit: CompilePermit) -> Result<Self, SnapshotError> {
        seal::seal(draft, permit)
    }

    pub(crate) const fn id(&self) -> SnapshotId {
        self.id
    }

    pub(crate) fn abi(&self) -> WxIrAbi {
        self.body.abi
    }

    pub(crate) fn body(&self) -> &SnapshotBody {
        &self.body
    }

    pub(crate) fn derive_optimized(&self, body: SnapshotBody) -> Result<Self, SnapshotError> {
        if body.abi != self.body.abi
            || body.executable != self.body.executable
            || body.schema_epoch != self.body.schema_epoch
            || body.entry_kind != self.body.entry_kind
            || body.dependencies != self.body.dependencies
            || body.parent != self.body.parent
        {
            return Err(SnapshotError::DanglingDependency);
        }
        verifier::verify(&body)?;
        let canonical = serde_json::to_vec(&body).map_err(|_| SnapshotError::Serialization)?;
        Ok(Self::verified(
            SnapshotId(*blake3::hash(&canonical).as_bytes()),
            body,
        ))
    }

    pub(crate) fn derive_bridge(
        &self,
        guard: u32,
        observed_case: u8,
        mut body: SnapshotBody,
    ) -> Result<Self, SnapshotError> {
        if body.abi != self.body.abi
            || body.executable != self.body.executable
            || body.schema_epoch != self.body.schema_epoch
            || body.dependencies != self.body.dependencies
            || body.parent.is_some()
        {
            return Err(SnapshotError::DanglingDependency);
        }
        body.parent = Some((self.id, guard, observed_case));
        verifier::verify(&body)?;
        let canonical = serde_json::to_vec(&body).map_err(|_| SnapshotError::Serialization)?;
        Ok(Self::verified(
            SnapshotId(*blake3::hash(&canonical).as_bytes()),
            body,
        ))
    }

    pub(super) fn verified(id: SnapshotId, body: SnapshotBody) -> Self {
        Self {
            id,
            body: Arc::new(body),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SnapshotError {
    SchemaPermitMismatch { permit: u64, snapshot: u64 },
    EmptyBlocks,
    MissingEntry,
    DuplicateBlock { block: u32 },
    DuplicateDefinition { value: u32 },
    UndefinedValue { value: u32 },
    UseBeforeDefinition { value: u32 },
    NonDominatingUse { value: u32, block: u32 },
    TypeMismatch { value: u32 },
    InvalidCfg { block: u32 },
    InvalidPhi { block: u32 },
    BadEffectOrdering { block: u32 },
    MissingDeopt { id: u32 },
    MissingSafepoint { block: u32 },
    DuplicateDeopt { id: u32 },
    InvalidDeopt { id: u32 },
    MissingRootMap { point: u32 },
    DuplicateRootMap { point: u32 },
    MissingRoot { point: u32 },
    SurplusRoot { point: u32 },
    StaleDependency { kind: dependency::DependencyKind },
    MissingDependency { kind: dependency::DependencyKind },
    BorrowAcrossSafepoint { value: u32 },
    InvalidOwnedList { identity: u32 },
    DanglingDependency,
    Serialization,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "WXIR v2 verification failed: {self:?}")
    }
}

impl std::error::Error for SnapshotError {}

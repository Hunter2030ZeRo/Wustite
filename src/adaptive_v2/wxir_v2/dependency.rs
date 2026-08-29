use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum DependencyKind {
    Executable,
    Schema,
    Shape,
    Class,
    ListLayout,
    Callee,
    HelperAbi,
    GcAbi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Dependency {
    pub(crate) kind: DependencyKind,
    pub(crate) identity: u64,
    pub(crate) expected_epoch: u64,
    pub(crate) observed_epoch: u64,
}

impl Dependency {
    pub(crate) const fn current(kind: DependencyKind, identity: u64, epoch: u64) -> Self {
        Self::observed(kind, identity, epoch, epoch)
    }

    pub(crate) const fn observed(
        kind: DependencyKind,
        identity: u64,
        expected_epoch: u64,
        observed_epoch: u64,
    ) -> Self {
        Self {
            kind,
            identity,
            expected_epoch,
            observed_epoch,
        }
    }

    pub(crate) const fn is_current(self) -> bool {
        self.expected_epoch == self.observed_epoch
    }
}

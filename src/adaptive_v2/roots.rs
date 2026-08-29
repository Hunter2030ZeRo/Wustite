use super::handles::StableHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootKind {
    FrameRegister,
    FunctionConstant,
    CurrentFunction,
    Argument,
    Result,
    InlineCache,
    PreparedLeafCallTarget,
    NativeSpill,
    DeoptMaterialization,
    HostPinned,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RootInventory {
    entries: Vec<(RootKind, StableHandle)>,
}

impl RootInventory {
    pub(crate) const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub(crate) fn insert(&mut self, kind: RootKind, handle: StableHandle) {
        self.entries.push((kind, handle));
    }

    pub(crate) fn handles(&self) -> impl Iterator<Item = StableHandle> + '_ {
        self.entries.iter().map(|(_, handle)| *handle)
    }

    pub(crate) fn kinds(&self) -> impl Iterator<Item = RootKind> + '_ {
        self.entries.iter().map(|(kind, _)| *kind)
    }
}

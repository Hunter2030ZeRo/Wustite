use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::dependency::Dependency;
use super::ir::{Constant, RootLocation, SafepointId, ValueId, ValueType};
use crate::adaptive_v2::trace::ExecutableIdentity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ResumeMode {
    ReplayBeforePc,
    ResumeAfterPc,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum RegisterSource {
    Ssa(ValueId),
    Constant(Constant),
    Spill { slot: u32, ty: ValueType },
    Virtual(u32),
    UndefinedDead,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct RegisterRecipe {
    pub(crate) register: u16,
    pub(crate) source: RegisterSource,
    pub(crate) ty: ValueType,
}

impl RegisterRecipe {
    pub(crate) const fn new(register: u16, source: RegisterSource, ty: ValueType) -> Self {
        Self {
            register,
            source,
            ty,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ExceptionState {
    Clear,
    Pending { class: u64, message: String },
    ErrorCode(i32),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct FrameRecipe {
    pub(crate) function: u64,
    pub(crate) resume_pc: u32,
    pub(crate) registers: Vec<RegisterRecipe>,
    pub(crate) dead_registers: BTreeSet<u16>,
    pub(crate) exception: ExceptionState,
}

impl FrameRecipe {
    pub(crate) fn new(function: u64, resume_pc: u32, registers: Vec<RegisterRecipe>) -> Self {
        Self {
            function,
            resume_pc,
            registers,
            dead_registers: BTreeSet::new(),
            exception: ExceptionState::Clear,
        }
    }

    pub(crate) fn with_exception(mut self, exception: ExceptionState) -> Self {
        self.exception = exception;
        self
    }

    pub(crate) fn with_dead_registers(mut self, registers: impl IntoIterator<Item = u16>) -> Self {
        self.dead_registers = registers.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum VirtualKind {
    Object {
        shape_identity: u64,
        shape_dependency_epoch: u64,
        shape_layout_epoch: u64,
        fields: Vec<(u32, RegisterSource)>,
    },
    List {
        items: Vec<RegisterSource>,
    },
    Tuple {
        items: Vec<RegisterSource>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct VirtualRecipe {
    pub(crate) id: u32,
    pub(crate) kind: VirtualKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct DeoptRecipe {
    pub(crate) id: u32,
    pub(crate) executable: ExecutableIdentity,
    pub(crate) resume_pc: u32,
    pub(crate) mode: ResumeMode,
    pub(crate) frames: Vec<FrameRecipe>,
    pub(crate) virtuals: Vec<VirtualRecipe>,
    pub(crate) root_point: SafepointId,
    pub(crate) explicit_roots: Vec<RootLocation>,
    pub(crate) dependencies: Vec<Dependency>,
}

impl DeoptRecipe {
    pub(crate) fn new(
        id: u32,
        executable: ExecutableIdentity,
        resume_pc: u32,
        mode: ResumeMode,
        frames: Vec<FrameRecipe>,
        root_point: SafepointId,
    ) -> Self {
        Self {
            id,
            executable,
            resume_pc,
            mode,
            frames,
            virtuals: Vec::new(),
            root_point,
            explicit_roots: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    pub(crate) fn with_virtuals(mut self, virtuals: Vec<VirtualRecipe>) -> Self {
        self.virtuals = virtuals;
        self
    }

    pub(crate) fn with_dependencies(mut self, dependencies: Vec<Dependency>) -> Self {
        self.dependencies = dependencies;
        self
    }
}

use std::collections::BTreeMap;
use std::fmt;

use crate::adaptive_v2::handles::StableHandle;
use crate::adaptive_v2::heap::{GcError, GcHeap};
use crate::adaptive_v2::lists::ListStrategy;
use crate::adaptive_v2::shapes::ShapeTable;

use super::deopt::{ExceptionState, ResumeMode};
use super::ir::{ValueId, ValueType};

mod engine;
mod validate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeAtom {
    Integer(i64),
    FloatBits(u64),
    Boolean(bool),
    Handle(StableHandle),
    UndefinedDead,
}

impl RuntimeAtom {
    const fn ty(self) -> Option<ValueType> {
        match self {
            Self::Integer(_) => Some(ValueType::I64),
            Self::FloatBits(_) => Some(ValueType::F64),
            Self::Boolean(_) => Some(ValueType::Bool),
            Self::Handle(_) => Some(ValueType::Handle),
            Self::UndefinedDead => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MaterializedKind {
    Object {
        shape_identity: u64,
        shape_dependency_epoch: u64,
        shape_layout_epoch: u64,
        fields: Vec<(u32, RuntimeAtom)>,
    },
    List {
        strategy: ListStrategy,
        items: Vec<RuntimeAtom>,
    },
    Tuple {
        items: Vec<RuntimeAtom>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializedVirtual {
    pub(crate) id: u32,
    pub(crate) handle: StableHandle,
    pub(crate) kind: MaterializedKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconstructedFrame {
    pub(crate) function: u64,
    pub(crate) resume_pc: u32,
    pub(crate) registers: Vec<RuntimeAtom>,
    pub(crate) exception: ExceptionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconstructedState {
    pub(crate) resume_pc: u32,
    pub(crate) mode: ResumeMode,
    pub(crate) frames: Vec<ReconstructedFrame>,
    pub(crate) virtuals: Vec<MaterializedVirtual>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeoptError {
    StaleDependency,
    IncompleteDependency,
    InvalidConstant,
    MissingValue { value: u32 },
    MissingSpill { spill: u32 },
    MissingVirtual { virtual_id: u32 },
    DuplicateVirtual { virtual_id: u32 },
    TypeMismatch { register: u16 },
    NonContiguousRegisters { function: u64 },
    StaleShape { shape: u64 },
    HelperFailure { helper: u64 },
    InvalidHandle(GcError),
}

impl fmt::Display for DeoptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "forced deoptimization failed: {self:?}")
    }
}

impl std::error::Error for DeoptError {}

impl From<GcError> for DeoptError {
    fn from(value: GcError) -> Self {
        Self::InvalidHandle(value)
    }
}

pub(crate) struct DeoptEngine<'a> {
    pub(super) heap: &'a GcHeap,
    pub(super) values: &'a BTreeMap<ValueId, RuntimeAtom>,
    pub(super) spills: &'a BTreeMap<u32, RuntimeAtom>,
    pub(super) shapes: Option<&'a ShapeTable>,
    pub(super) forced_helper_failure: Option<u64>,
}

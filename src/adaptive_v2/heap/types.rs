use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::adaptive_v2::handles::{HandleError, StableHandle, StableHandleTable};
use crate::adaptive_v2::safepoint::{SafepointCoordinator, SafepointError};
use crate::adaptive_v2::value_word::ScalarValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GcConfig {
    pub(crate) collect_every_allocation: bool,
    pub(crate) promotion_age: u8,
    pub(crate) allocation_limit: Option<usize>,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            collect_every_allocation: false,
            promotion_age: 2,
            allocation_limit: None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeapMetrics {
    pub(crate) allocations: u64,
    pub(crate) allocated_bytes: u64,
    pub(crate) minor_collections: u64,
    pub(crate) major_collections: u64,
    pub(crate) pause_micros: u64,
    pub(crate) promotions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GcError {
    AllocationLimit,
    InvalidHandle(HandleError),
    NotScalar,
    Safepoint(SafepointError),
    MarkerPanicked,
    HostPinOverflow,
}

impl fmt::Display for GcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllocationLimit => formatter.write_str("adaptive heap allocation limit reached"),
            Self::InvalidHandle(error) => error.fmt(formatter),
            Self::NotScalar => formatter.write_str("heap object is not a boxed scalar"),
            Self::Safepoint(error) => error.fmt(formatter),
            Self::MarkerPanicked => formatter.write_str("concurrent marker panicked"),
            Self::HostPinOverflow => formatter.write_str("adaptive host-root pin count overflowed"),
        }
    }
}

impl std::error::Error for GcError {}

impl From<HandleError> for GcError {
    fn from(value: HandleError) -> Self {
        Self::InvalidHandle(value)
    }
}

impl From<SafepointError> for GcError {
    fn from(value: SafepointError) -> Self {
        Self::Safepoint(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GcObject {
    pub(super) scalar: Option<ScalarValue>,
    pub(super) references: Vec<StableHandle>,
}

impl GcObject {
    pub(crate) const fn new() -> Self {
        Self {
            scalar: None,
            references: Vec::new(),
        }
    }

    pub(crate) const fn boxed_scalar(value: ScalarValue) -> Self {
        Self {
            scalar: Some(value),
            references: Vec::new(),
        }
    }

    pub(crate) fn with_references(references: Vec<StableHandle>) -> Self {
        Self {
            scalar: None,
            references,
        }
    }

    pub(crate) fn references(&self) -> &[StableHandle] {
        &self.references
    }

    pub(crate) fn owned_bytes(&self) -> u64 {
        let references = self
            .references
            .capacity()
            .saturating_mul(std::mem::size_of::<StableHandle>());
        let bytes = std::mem::size_of::<Self>().saturating_add(references);
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }
}

pub(super) fn completed_pause_micros(started: Instant) -> u64 {
    match u64::try_from(started.elapsed().as_micros()) {
        Ok(micros) => micros.max(1),
        Err(_) => u64::MAX,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Location {
    Nursery(usize),
    Old(usize),
}

#[derive(Debug, Clone)]
pub(super) struct NurseryCell {
    pub(super) object: GcObject,
    pub(super) age: u8,
}

#[derive(Debug)]
pub(super) struct State {
    pub(super) handles: StableHandleTable<Location>,
    pub(super) nursery: Vec<Option<NurseryCell>>,
    pub(super) old: Vec<Option<GcObject>>,
    pub(super) host_pins: HashMap<StableHandle, usize>,
    pub(super) remembered: HashSet<StableHandle>,
    pub(super) concurrent_barrier: HashSet<StableHandle>,
    pub(super) marking: bool,
    pub(super) live: usize,
    pub(super) metrics: HeapMetrics,
}

#[derive(Debug)]
pub(super) struct HeapInner {
    pub(super) config: GcConfig,
    pub(super) state: Mutex<State>,
    pub(super) safepoints: SafepointCoordinator,
}

#[derive(Debug, Clone)]
pub(crate) struct GcHeap {
    pub(super) inner: Arc<HeapInner>,
}

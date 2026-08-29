mod borrow;
mod collection;
mod major;
mod transaction;
mod types;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use super::handles::{HandleError, RuntimeId, StableHandle, StableHandleTable};
use super::roots::{RootInventory, RootKind};
use super::safepoint::{Mutator, SafepointCoordinator};
use super::value_word::ScalarValue;
use types::{HeapInner, Location, NurseryCell, State};

pub(crate) use major::MajorCycle;
pub(crate) use transaction::{BatchObject, BatchReference};
pub(crate) use types::{GcConfig, GcError, GcHeap, GcObject, HeapMetrics};

impl GcHeap {
    pub(crate) fn new(config: GcConfig) -> Self {
        let runtime = next_runtime_id();
        let capacity = config.allocation_limit.unwrap_or(u32::MAX as usize);
        Self {
            inner: Arc::new(HeapInner {
                config,
                state: Mutex::new(State {
                    handles: StableHandleTable::new(runtime, capacity),
                    nursery: Vec::new(),
                    old: Vec::new(),
                    host_pins: HashMap::new(),
                    remembered: HashSet::new(),
                    concurrent_barrier: HashSet::new(),
                    marking: false,
                    live: 0,
                    metrics: HeapMetrics::default(),
                }),
                safepoints: SafepointCoordinator::new(),
            }),
        }
    }

    pub(crate) fn register_mutator(&self) -> Mutator {
        self.inner.safepoints.register()
    }

    pub(crate) fn allocate(&self, object: GcObject) -> Result<StableHandle, GcError> {
        self.allocate_with_roots(object, &RootInventory::new())
    }

    pub(crate) fn allocate_with_roots(
        &self,
        object: GcObject,
        roots: &RootInventory,
    ) -> Result<StableHandle, GcError> {
        let allocated_bytes = object.owned_bytes();
        if self.inner.config.collect_every_allocation {
            self.minor_collect(roots)?;
        }
        let mut state = self.lock();
        if self
            .inner
            .config
            .allocation_limit
            .is_some_and(|limit| state.live >= limit)
        {
            return Err(GcError::AllocationLimit);
        }
        let index = state.nursery.len();
        let handle = state.handles.allocate(Location::Nursery(index))?;
        state.nursery.push(Some(NurseryCell { object, age: 0 }));
        state.live += 1;
        state.metrics.allocations += 1;
        state.metrics.allocated_bytes = state
            .metrics
            .allocated_bytes
            .saturating_add(allocated_bytes);
        Ok(handle)
    }

    pub(crate) fn resolve(&self, handle: StableHandle) -> Result<GcObject, GcError> {
        let state = self.lock();
        object_at(&state, handle).cloned()
    }

    pub(crate) fn scalar(&self, handle: StableHandle) -> Result<ScalarValue, GcError> {
        self.resolve(handle)?.scalar.ok_or(GcError::NotScalar)
    }

    pub(crate) fn pin_host(&self, handle: StableHandle) -> Result<(), GcError> {
        let mut state = self.lock();
        state.handles.resolve(handle)?;
        let count = state.host_pins.entry(handle).or_insert(0);
        *count = count.checked_add(1).ok_or(GcError::HostPinOverflow)?;
        Ok(())
    }

    pub(crate) fn unpin_host(&self, handle: StableHandle) -> Result<(), GcError> {
        let mut state = self.lock();
        state.handles.resolve(handle)?;
        match state.host_pins.get_mut(&handle) {
            Some(count) if *count > 1 => *count -= 1,
            Some(_) => {
                state.host_pins.remove(&handle);
            }
            None => {}
        }
        Ok(())
    }

    pub(crate) fn host_root_inventory(&self) -> RootInventory {
        let state = self.lock();
        let mut roots = RootInventory::new();
        for handle in state.host_pins.keys().copied() {
            roots.insert(RootKind::HostPinned, handle);
        }
        roots
    }

    pub(crate) fn is_old(&self, handle: StableHandle) -> bool {
        self.lock()
            .handles
            .resolve(handle)
            .is_ok_and(|location| matches!(location, Location::Old(_)))
    }

    pub(crate) fn store_reference(
        &self,
        owner: StableHandle,
        target: StableHandle,
    ) -> Result<(), GcError> {
        let mut state = self.lock();
        let owner_location = *state.handles.resolve(owner)?;
        let target_location = *state.handles.resolve(target)?;
        object_at_mut(&mut state, owner)?.references.push(target);
        if matches!(owner_location, Location::Old(_))
            && matches!(target_location, Location::Nursery(_))
        {
            state.remembered.insert(owner);
        }
        if state.marking {
            state.concurrent_barrier.insert(target);
        }
        Ok(())
    }

    pub(crate) fn start_major(&self, roots: &RootInventory) -> Result<MajorCycle, GcError> {
        MajorCycle::start(self.clone(), roots)
    }

    pub(crate) fn metrics(&self) -> HeapMetrics {
        self.lock().metrics
    }

    pub(super) fn handle_from_local(&self, packed: u64) -> StableHandle {
        self.lock().handles.local_handle(packed)
    }

    pub(in crate::adaptive_v2::heap) fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(in crate::adaptive_v2::heap) fn object_at(
    state: &State,
    handle: StableHandle,
) -> Result<&GcObject, GcError> {
    match *state.handles.resolve(handle)? {
        Location::Nursery(index) => state
            .nursery
            .get(index)
            .and_then(Option::as_ref)
            .map(|cell| &cell.object),
        Location::Old(index) => state.old.get(index).and_then(Option::as_ref),
    }
    .ok_or(GcError::InvalidHandle(HandleError::Stale))
}

pub(in crate::adaptive_v2::heap) fn object_at_mut(
    state: &mut State,
    handle: StableHandle,
) -> Result<&mut GcObject, GcError> {
    match *state.handles.resolve(handle)? {
        Location::Nursery(index) => state
            .nursery
            .get_mut(index)
            .and_then(Option::as_mut)
            .map(|cell| &mut cell.object),
        Location::Old(index) => state.old.get_mut(index).and_then(Option::as_mut),
    }
    .ok_or(GcError::InvalidHandle(HandleError::Stale))
}

fn next_runtime_id() -> RuntimeId {
    static NEXT: OnceLock<Mutex<u64>> = OnceLock::new();
    let mut next = NEXT
        .get_or_init(|| Mutex::new(1))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let id = RuntimeId::new(*next);
    *next = next.wrapping_add(1);
    id
}

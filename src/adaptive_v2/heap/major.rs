use std::collections::{HashMap, HashSet, VecDeque};
use std::thread::{self, JoinHandle};

use super::types::completed_pause_micros;
use super::{GcError, GcHeap, GcObject, Location, object_at};
use crate::adaptive_v2::handles::StableHandle;
use crate::adaptive_v2::roots::RootInventory;

pub(crate) struct MajorCycle {
    heap: GcHeap,
    roots: RootInventory,
    marker: Option<JoinHandle<HashSet<StableHandle>>>,
}

impl MajorCycle {
    pub(super) fn start(heap: GcHeap, roots: &RootInventory) -> Result<Self, GcError> {
        let (objects, seed) = {
            let mut state = heap.lock();
            state.marking = true;
            state.concurrent_barrier.clear();
            let mut objects = HashMap::new();
            for handle in state.handles.live_handles() {
                objects.insert(handle, object_at(&state, handle)?.clone());
            }
            let seed = roots
                .handles()
                .chain(state.host_pins.keys().copied())
                .collect::<Vec<_>>();
            (objects, seed)
        };
        let marker = thread::spawn(move || mark_snapshot(&objects, &seed));
        Ok(Self {
            heap,
            roots: roots.clone(),
            marker: Some(marker),
        })
    }

    pub(crate) fn finish(mut self) -> Result<(), GcError> {
        let marker = self.marker.take().ok_or(GcError::MarkerPanicked)?;
        let snapshot_marks = marker.join().map_err(|_| GcError::MarkerPanicked)?;
        let mut state = self.heap.lock();
        let started = std::time::Instant::now();
        let seed = self
            .roots
            .handles()
            .chain(state.host_pins.keys().copied())
            .chain(state.concurrent_barrier.iter().copied())
            .collect::<Vec<_>>();
        let mut live = trace_current(&state, &seed);
        live.extend(snapshot_marks);
        let handles: Vec<_> = state.handles.live_handles().collect();
        for handle in handles {
            if matches!(state.handles.resolve(handle), Ok(Location::Old(_)))
                && !live.contains(&handle)
            {
                let Location::Old(index) = state.handles.release(handle)? else {
                    continue;
                };
                if let Some(slot) = state.old.get_mut(index) {
                    *slot = None;
                }
                state.live -= 1;
            }
        }
        state.marking = false;
        state.concurrent_barrier.clear();
        let valid: HashSet<_> = state.handles.live_handles().collect();
        state.remembered.retain(|handle| valid.contains(handle));
        state.metrics.major_collections += 1;
        state.metrics.pause_micros = state
            .metrics
            .pause_micros
            .saturating_add(completed_pause_micros(started));
        Ok(())
    }
}

impl Drop for MajorCycle {
    fn drop(&mut self) {
        if let Some(marker) = self.marker.take() {
            let _ = marker.join();
        }
        let mut state = self.heap.lock();
        state.marking = false;
        state.concurrent_barrier.clear();
    }
}

fn mark_snapshot(
    objects: &HashMap<StableHandle, GcObject>,
    seed: &[StableHandle],
) -> HashSet<StableHandle> {
    let mut marked = HashSet::new();
    let mut queue = VecDeque::from(seed.to_vec());
    while let Some(handle) = queue.pop_front() {
        if !marked.insert(handle) {
            continue;
        }
        if let Some(object) = objects.get(&handle) {
            queue.extend(object.references().iter().copied());
        }
    }
    marked
}

fn trace_current(state: &super::State, seed: &[StableHandle]) -> HashSet<StableHandle> {
    let mut marked = HashSet::new();
    let mut queue = VecDeque::from(seed.to_vec());
    while let Some(handle) = queue.pop_front() {
        if !marked.insert(handle) {
            continue;
        }
        if let Ok(object) = object_at(state, handle) {
            queue.extend(object.references().iter().copied());
        }
    }
    marked
}

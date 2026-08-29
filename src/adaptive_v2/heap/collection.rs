use std::collections::{HashSet, VecDeque};

use super::types::completed_pause_micros;
use super::{GcError, GcHeap, Location, NurseryCell, object_at};
use crate::adaptive_v2::roots::RootInventory;
use crate::adaptive_v2::safepoint::Mutator;

impl GcHeap {
    pub(crate) fn minor_collect(&self, roots: &RootInventory) -> Result<(), GcError> {
        let mut initiator = self.register_mutator();
        self.minor_collect_at(&mut initiator, roots)
    }

    pub(crate) fn minor_collect_at(
        &self,
        initiator: &mut Mutator,
        roots: &RootInventory,
    ) -> Result<(), GcError> {
        let heap = self.clone();
        let roots = roots.clone();
        self.inner
            .safepoints
            .request_with(initiator, move || heap.copy_nursery(&roots))??;
        initiator.epoch();
        Ok(())
    }

    fn copy_nursery(&self, roots: &RootInventory) -> Result<(), GcError> {
        let mut state = self.lock();
        let started = std::time::Instant::now();
        let mut queue: VecDeque<_> = roots
            .handles()
            .chain(state.host_pins.keys().copied())
            .chain(state.remembered.iter().copied())
            .collect();
        let mut visited = HashSet::new();
        let mut reachable = HashSet::new();
        while let Some(handle) = queue.pop_front() {
            if !visited.insert(handle) {
                continue;
            }
            let location = match state.handles.resolve(handle) {
                Ok(location) => *location,
                Err(_) => continue,
            };
            if matches!(location, Location::Nursery(_)) {
                reachable.insert(handle);
            }
            for child in object_at(&state, handle)?.references() {
                queue.push_back(*child);
            }
        }

        let live_handles: Vec<_> = state.handles.live_handles().collect();
        let mut to_space = Vec::new();
        let mut promotions = 0;
        for handle in live_handles {
            let Location::Nursery(index) = *state.handles.resolve(handle)? else {
                continue;
            };
            let Some(cell) = state.nursery.get_mut(index).and_then(Option::take) else {
                continue;
            };
            if !reachable.contains(&handle) {
                state.handles.release(handle)?;
                state.live -= 1;
                continue;
            }
            let age = cell.age.saturating_add(1);
            if age >= self.inner.config.promotion_age {
                let old_index = state.old.len();
                state.old.push(Some(cell.object));
                *state.handles.resolve_mut(handle)? = Location::Old(old_index);
                promotions += 1;
            } else {
                let new_index = to_space.len();
                to_space.push(Some(NurseryCell {
                    object: cell.object,
                    age,
                }));
                *state.handles.resolve_mut(handle)? = Location::Nursery(new_index);
            }
        }
        state.nursery = to_space;
        let valid: HashSet<_> = state.handles.live_handles().collect();
        state.remembered.retain(|handle| valid.contains(handle));
        state.metrics.minor_collections += 1;
        state.metrics.promotions += promotions;
        state.metrics.pause_micros = state
            .metrics
            .pause_micros
            .saturating_add(completed_pause_micros(started));
        Ok(())
    }
}

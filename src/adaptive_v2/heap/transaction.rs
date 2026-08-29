use crate::adaptive_v2::handles::StableHandle;
use crate::adaptive_v2::roots::RootInventory;

use super::types::{Location, NurseryCell};
use super::{GcError, GcHeap, GcObject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchReference {
    Object(usize),
    External(StableHandle),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchObject {
    references: Vec<BatchReference>,
}

impl BatchObject {
    pub(crate) const fn new(references: Vec<BatchReference>) -> Self {
        Self { references }
    }
}

impl GcHeap {
    pub(crate) fn allocate_graph_with_roots(
        &self,
        objects: &[BatchObject],
        roots: &RootInventory,
    ) -> Result<Vec<StableHandle>, GcError> {
        if self.inner.config.collect_every_allocation {
            self.minor_collect(roots)?;
        }
        let mut state = self.lock();
        let requested = objects.len();
        let within_limit = self
            .inner
            .config
            .allocation_limit
            .is_none_or(|limit| state.live.saturating_add(requested) <= limit);
        if !within_limit || state.handles.remaining_capacity() < requested {
            return Err(GcError::AllocationLimit);
        }
        for object in objects {
            for reference in &object.references {
                match reference {
                    BatchReference::Object(index) if *index < requested => {}
                    BatchReference::External(handle) => {
                        state.handles.resolve(*handle)?;
                    }
                    BatchReference::Object(_) => {
                        return Err(GcError::InvalidHandle(
                            crate::adaptive_v2::handles::HandleError::Stale,
                        ));
                    }
                }
            }
        }

        let mut handles = Vec::with_capacity(requested);
        for _ in objects {
            let index = state.nursery.len();
            let handle = state.handles.allocate(Location::Nursery(index))?;
            state.nursery.push(Some(NurseryCell {
                object: GcObject::new(),
                age: 0,
            }));
            handles.push(handle);
        }
        let batch_start = state.nursery.len() - requested;
        for (index, object) in objects.iter().enumerate() {
            let references = object
                .references
                .iter()
                .map(|reference| match reference {
                    BatchReference::Object(target) => handles[*target],
                    BatchReference::External(handle) => *handle,
                })
                .collect();
            if let Some(cell) = state
                .nursery
                .get_mut(batch_start + index)
                .and_then(Option::as_mut)
            {
                cell.object = GcObject::with_references(references);
            }
        }
        state.live += requested;
        Ok(handles)
    }
}

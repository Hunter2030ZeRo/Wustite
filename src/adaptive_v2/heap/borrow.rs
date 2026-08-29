use super::{GcError, GcHeap, GcObject, object_at};
use crate::adaptive_v2::handles::StableHandle;
use crate::adaptive_v2::safepoint::Mutator;

pub(crate) struct BorrowedObject<'scope> {
    epoch: u64,
    object: &'scope GcObject,
}

impl BorrowedObject<'_> {
    pub(crate) const fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn references(&self) -> &[StableHandle] {
        self.object.references()
    }
}

impl GcHeap {
    pub(crate) fn with_borrow<R>(
        &self,
        mutator: &mut Mutator,
        handle: StableHandle,
        operation: impl for<'scope> FnOnce(BorrowedObject<'scope>) -> R,
    ) -> Result<R, GcError> {
        let epoch = mutator.epoch();
        let state = self.lock();
        let object = object_at(&state, handle)?;
        Ok(operation(BorrowedObject { epoch, object }))
    }
}

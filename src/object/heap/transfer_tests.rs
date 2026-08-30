use super::{Object, ObjectError, ObjectHeap};

#[test]
fn adaptive_transfer_exact_one_semantic_owner() {
    // Given: one list stored behind a stable public object handle.
    let mut heap = ObjectHeap::new();
    let reference = heap.allocate(Object::list(Vec::new())).unwrap();

    // When: the adaptive adapter takes ownership of the object.
    let object = heap.transfer_out(reference).unwrap();

    // Then: the compatibility heap cannot concurrently observe a shadow copy.
    assert_eq!(
        heap.get(reference),
        Err(ObjectError::VacantSlot { slot: 0 })
    );

    // When: ownership is handed back through the same stable handle.
    heap.transfer_in(reference, object).unwrap();

    // Then: the public handle resolves again without changing identity.
    assert!(matches!(heap.get(reference), Ok(Object::List(_))));
}

#[test]
fn adaptive_transfer_rejects_double_handoff() {
    // Given: a live public object and one detached owner.
    let mut heap = ObjectHeap::new();
    let reference = heap.allocate(Object::list(Vec::new())).unwrap();
    let object = heap.transfer_out(reference).unwrap();
    heap.transfer_in(reference, object.clone()).unwrap();

    // When: a stale adapter attempts to restore the same object twice.
    let error = heap.transfer_in(reference, object).unwrap_err();

    // Then: the live compatibility owner is not overwritten.
    assert_eq!(error, ObjectError::TransferTargetOccupied { slot: 0 });
}

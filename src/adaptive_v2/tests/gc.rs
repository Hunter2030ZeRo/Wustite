use std::thread;

use super::super::handles::HandleError;
use super::super::heap::{GcConfig, GcError, GcHeap, GcObject};
use super::super::roots::{RootInventory, RootKind};

#[test]
fn collector_keeps_cycles_aliases_old_to_young_edges() {
    let heap = GcHeap::new(GcConfig {
        collect_every_allocation: true,
        promotion_age: 1,
        allocation_limit: Some(8),
    });
    let mut roots = RootInventory::new();
    let left = heap
        .allocate(GcObject::new())
        .expect("first allocation should fit");
    roots.insert(RootKind::FrameRegister, left);
    let right = heap
        .allocate_with_roots(GcObject::new(), &roots)
        .expect("second allocation should fit");
    roots.insert(RootKind::HostPinned, right);
    heap.store_reference(left, right)
        .expect("left should be live");
    heap.store_reference(right, left)
        .expect("right should be live");
    heap.minor_collect(&roots)
        .expect("minor collection should finish");
    assert!(heap.is_old(left));
    let young = heap
        .allocate_with_roots(GcObject::new(), &roots)
        .expect("young allocation should fit");
    heap.store_reference(left, young)
        .expect("old-to-young store should succeed");
    heap.minor_collect(&roots)
        .expect("remembered edge should retain young");
    assert!(heap.resolve(young).is_ok());
}

#[test]
fn alloc_limit_unreachable_collection_explicit() {
    let heap = GcHeap::new(GcConfig {
        collect_every_allocation: false,
        promotion_age: 2,
        allocation_limit: Some(1),
    });
    let dead = heap
        .allocate(GcObject::new())
        .expect("one allocation should fit");
    assert_eq!(
        heap.allocate(GcObject::new()),
        Err(GcError::AllocationLimit)
    );
    heap.minor_collect(&RootInventory::new())
        .expect("unrooted objects should collect");
    assert_eq!(
        heap.resolve(dead),
        Err(GcError::InvalidHandle(HandleError::Stale))
    );
}

#[test]
fn collect_every_alloc_uses_only_supplied_roots() {
    let heap = GcHeap::new(GcConfig {
        collect_every_allocation: true,
        ..GcConfig::default()
    });
    let unrooted = heap
        .allocate(GcObject::new())
        .expect("first allocation should fit");
    let rooted = heap
        .allocate(GcObject::new())
        .expect("second allocation should fit");
    assert_eq!(
        heap.resolve(unrooted),
        Err(GcError::InvalidHandle(HandleError::Stale))
    );
    let mut roots = RootInventory::new();
    roots.insert(RootKind::FrameRegister, rooted);
    heap.allocate_with_roots(GcObject::new(), &roots)
        .expect("rooted allocation should fit");
    assert!(heap.resolve(rooted).is_ok());
}

#[test]
fn concurrent_mark_tracks_barrier_and_sweeps_dead() {
    let heap = GcHeap::new(GcConfig {
        promotion_age: 1,
        ..GcConfig::default()
    });
    let owner = heap
        .allocate(GcObject::new())
        .expect("owner allocation should fit");
    let dead = heap
        .allocate(GcObject::new())
        .expect("dead allocation should fit");
    let mut promotion_roots = RootInventory::new();
    promotion_roots.insert(RootKind::FrameRegister, owner);
    promotion_roots.insert(RootKind::FrameRegister, dead);
    heap.minor_collect(&promotion_roots)
        .expect("promotion should finish");
    let mut roots = RootInventory::new();
    roots.insert(RootKind::FrameRegister, owner);
    let cycle = heap
        .start_major(&roots)
        .expect("concurrent mark should start");
    let target = heap
        .allocate(GcObject::new())
        .expect("concurrent target should allocate");
    let mutator_heap = heap.clone();
    thread::spawn(move || mutator_heap.store_reference(owner, target))
        .join()
        .expect("mutator thread should not panic")
        .expect("barrier store should succeed");
    cycle.finish().expect("major cycle should finish");
    assert_eq!(
        heap.resolve(dead),
        Err(GcError::InvalidHandle(HandleError::Stale))
    );
    heap.minor_collect(&roots)
        .expect("remembered target should survive minor collection");
    assert!(heap.resolve(target).is_ok());
}

#[test]
fn repeat_minor_major_cycles_reclaim_only_unreachable_objects() {
    let heap = GcHeap::new(GcConfig {
        promotion_age: 1,
        ..GcConfig::default()
    });
    let live = heap
        .allocate(GcObject::new())
        .expect("live allocation should fit");
    let dead = heap
        .allocate(GcObject::new())
        .expect("dead allocation should fit");
    let mut both = RootInventory::new();
    both.insert(RootKind::FrameRegister, live);
    both.insert(RootKind::FrameRegister, dead);
    heap.minor_collect(&both).expect("promotion should finish");
    let mut live_only = RootInventory::new();
    live_only.insert(RootKind::FrameRegister, live);
    for _ in 0..3 {
        heap.minor_collect(&live_only)
            .expect("repeated minor should finish");
        heap.start_major(&live_only)
            .expect("major should start")
            .finish()
            .expect("major should finish");
    }
    assert!(heap.resolve(live).is_ok());
    assert_eq!(
        heap.resolve(dead),
        Err(GcError::InvalidHandle(HandleError::Stale))
    );
}

#[test]
fn interrupted_major_cycle_clears_marker_state_on_drop() {
    let heap = GcHeap::new(GcConfig {
        promotion_age: 1,
        ..GcConfig::default()
    });
    let live = heap
        .allocate(GcObject::new())
        .expect("allocation should fit");
    let mut roots = RootInventory::new();
    roots.insert(RootKind::FrameRegister, live);
    heap.minor_collect(&roots).expect("promotion should finish");
    drop(heap.start_major(&roots).expect("first major should start"));
    heap.start_major(&roots)
        .expect("replacement major should start")
        .finish()
        .expect("replacement major should finish");
    assert!(heap.resolve(live).is_ok());
}

use std::sync::{Arc, mpsc};
use std::thread;

use super::super::heap::{GcConfig, GcHeap, GcObject};
use super::super::roots::{RootInventory, RootKind};
use super::super::safepoint::SafepointCoordinator;

#[test]
fn safepoint_handshake_invalidates_scoped_borrows() {
    let coordinator = SafepointCoordinator::new();
    let mut mutator = coordinator.register();
    mutator.poll().expect("idle poll should succeed");
    let epoch = mutator.epoch();
    coordinator
        .request_with(&mutator, || {})
        .expect("single-mutator stop should finish");
    assert_ne!(mutator.epoch(), epoch);
}

#[test]
fn scoped_borrow_cannot_escape_collection() {
    let heap = GcHeap::new(GcConfig::default());
    let handle = heap
        .allocate(GcObject::new())
        .expect("allocation should fit");
    let mut mutator = heap.register_mutator();
    let observed = heap
        .with_borrow(&mut mutator, handle, |borrowed| {
            assert!(borrowed.references().is_empty());
            borrowed.epoch()
        })
        .expect("live object should borrow");
    let mut roots = RootInventory::new();
    roots.insert(RootKind::FrameRegister, handle);
    heap.minor_collect_at(&mut mutator, &roots)
        .expect("safepoint should finish");
    assert_ne!(mutator.epoch(), observed);
}

#[test]
fn root_inventory_names_every_current_future_root_surface() {
    let heap = GcHeap::new(GcConfig::default());
    let handle = heap
        .allocate(GcObject::new())
        .expect("allocation should fit");
    let expected = [
        RootKind::FrameRegister,
        RootKind::FunctionConstant,
        RootKind::CurrentFunction,
        RootKind::Argument,
        RootKind::Result,
        RootKind::InlineCache,
        RootKind::PreparedLeafCallTarget,
        RootKind::NativeSpill,
        RootKind::DeoptMaterialization,
        RootKind::HostPinned,
    ];
    let mut roots = RootInventory::new();
    for kind in expected {
        roots.insert(kind, handle);
    }
    assert_eq!(roots.kinds().collect::<Vec<_>>(), expected);
}

#[test]
fn handshake_parks_peer_without_atomics() {
    let coordinator = Arc::new(SafepointCoordinator::new());
    let initiator = coordinator.register();
    let mut peer = coordinator.register();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        ready_tx.send(()).expect("ready signal should send");
        peer.park_for_next_request()
            .expect("peer should park and resume");
        peer.epoch()
    });
    ready_rx.recv().expect("peer should become ready");
    let requested_epoch = coordinator
        .request_with(&initiator, || 1_u64)
        .expect("coordinated stop should finish");
    assert_eq!(requested_epoch, 1);
    assert_eq!(worker.join().expect("peer should not panic"), 1);
}

#[cfg(loom)]
#[test]
fn safepoint_waits_peer_and_releases_after_stop() {
    use loom::sync::Arc as LoomArc;
    use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    loom::model(|| {
        let coordinator = LoomArc::new(SafepointCoordinator::new());
        let initiator = coordinator.register();
        let mut peer = coordinator.register();
        let started = LoomArc::new(AtomicBool::new(false));
        let phase = LoomArc::new(AtomicUsize::new(0));

        let peer_started = LoomArc::clone(&started);
        let peer_phase = LoomArc::clone(&phase);
        let worker = loom::thread::spawn(move || {
            peer_started.store(true, Ordering::Release);
            peer.park_for_next_request()
                .expect("peer should park and resume");
            assert_eq!(peer_phase.load(Ordering::Acquire), 1);
            peer.epoch()
        });

        while !started.load(Ordering::Acquire) {
            loom::thread::yield_now();
        }
        coordinator
            .request_with(&initiator, || {
                assert_eq!(phase.load(Ordering::Acquire), 0);
                phase.store(1, Ordering::Release);
            })
            .expect("coordinated stop should finish");

        assert_eq!(worker.join().expect("peer should not panic"), 1);
    });
}

use super::super::runtime::AdaptiveHeapRuntime;
use super::super::types::HeapAdapterError;
use crate::adaptive_v2::handles::StableHandle;
use crate::adaptive_v2::heap::{GcConfig, GcError, GcHeap, GcObject};
use crate::adaptive_v2::roots::RootInventory;

#[test]
fn heap_metrics_aggregate_runtime_clones() {
    // Given: collection-before-allocation and a rooted object that reaches promotion age.
    let runtime = AdaptiveHeapRuntime::new(GcConfig {
        collect_every_allocation: true,
        promotion_age: 2,
        allocation_limit: None,
    });
    let clone = runtime.clone();
    let initial = runtime.heap_metrics();
    assert_eq!(initial.allocations, 0);
    assert_eq!(initial.minor_collections, 0);
    assert_eq!(initial.major_collections, 0);
    assert_eq!(initial.promotions, 0);

    // When: allocation triggers one implicit collection, then explicit cycles promote and sweep.
    let object = runtime.allocate_object().expect("object allocation");
    runtime
        .collect_minor()
        .expect("first explicit minor collection");
    runtime.collect_minor().expect("promotion minor collection");
    runtime.collect_major().expect("major collection");

    // Then: only completed heap events are counted, independent of adapter payload inventory.
    let metrics = clone.heap_metrics();
    assert_eq!(metrics.allocations, 1);
    assert_eq!(metrics.minor_collections, 3);
    assert_eq!(metrics.major_collections, 1);
    assert_eq!(metrics.promotions, 1);
    assert_eq!(runtime.payload_counts(), (1, 0, 0));
    println!(
        "heap_metrics allocations={} minor_collections={} major_collections={} promotions={}",
        metrics.allocations,
        metrics.minor_collections,
        metrics.major_collections,
        metrics.promotions
    );
    drop(object);
}

#[test]
fn heap_metrics_exclude_failed_allocations() {
    // Given: a heap whose allocation limit permits exactly one payload.
    let runtime = AdaptiveHeapRuntime::new(GcConfig {
        allocation_limit: Some(1),
        ..GcConfig::default()
    });
    let _object = runtime.allocate_object().expect("first allocation");

    // When: a second allocation is rejected by the heap.
    let failure = runtime.allocate_list();

    // Then: only the successful completed allocation is recorded.
    assert!(matches!(
        failure,
        Err(HeapAdapterError::Heap(GcError::AllocationLimit))
    ));
    assert_eq!(runtime.heap_metrics().allocations, 1);
}

#[test]
fn heap_metrics_share_allocs_across_threads() {
    // Given: four independent clones of one runtime.
    let runtime = AdaptiveHeapRuntime::new(GcConfig::default());
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let clone = runtime.clone();
            std::thread::spawn(move || clone.allocate_object())
        })
        .collect();

    // When: each clone allocates concurrently.
    for worker in workers {
        let _root = worker
            .join()
            .expect("worker should not panic")
            .expect("allocation");
    }

    // Then: the shared snapshot records every completed allocation exactly once.
    assert_eq!(runtime.heap_metrics().allocations, 4);
}

#[test]
fn heap_metrics_count_managed_bytes() {
    // Given: a heap with room for exactly one managed object.
    let runtime = AdaptiveHeapRuntime::new(GcConfig {
        allocation_limit: Some(1),
        ..GcConfig::default()
    });
    let expected_bytes = u64::try_from(std::mem::size_of::<GcObject>())
        .expect("GcObject size fits the metrics counter");

    // When: one allocation commits and the next allocation is rejected.
    let _object = runtime.allocate_object().expect("first allocation");
    let failure = runtime.allocate_list();

    // Then: bytes describe only the committed managed object.
    assert_eq!(runtime.heap_metrics().allocated_bytes, expected_bytes);
    assert!(matches!(
        failure,
        Err(HeapAdapterError::Heap(GcError::AllocationLimit))
    ));
    assert_eq!(runtime.heap_metrics().allocated_bytes, expected_bytes);
}

#[test]
fn heap_metrics_include_owned_ref_capacity() {
    // Given: a managed object with owned but currently empty reference capacity.
    let references = Vec::<StableHandle>::with_capacity(4);
    let expected_bytes = u64::try_from(
        std::mem::size_of::<GcObject>()
            + references.capacity() * std::mem::size_of::<StableHandle>(),
    )
    .expect("managed bytes fit the metrics counter");
    let heap = GcHeap::new(GcConfig::default());

    // When: the object allocation commits to the heap.
    heap.allocate(GcObject::with_references(references))
        .expect("object allocation");

    // Then: the snapshot includes exactly its owned reference capacity.
    assert_eq!(heap.metrics().allocated_bytes, expected_bytes);
}

#[test]
fn heap_metrics_share_pause_across_clones() {
    // Given: a rooted managed object and a clone sharing its heap snapshot.
    let runtime = AdaptiveHeapRuntime::new(GcConfig::default());
    let clone = runtime.clone();
    let _object = runtime.allocate_object().expect("object allocation");
    assert!(runtime.heap_metrics().allocated_bytes > 0);
    assert_eq!(runtime.heap_metrics().pause_micros, 0);

    // When: completed minor and major collection cycles run.
    runtime.collect_minor().expect("minor collection");
    let after_minor = runtime.heap_metrics().pause_micros;
    runtime.collect_major().expect("major collection");

    // Then: each successful collection contributes a nonzero pause visible to clones.
    let metrics = clone.heap_metrics();
    assert!(after_minor >= 1);
    assert!(metrics.pause_micros > after_minor);
    assert!(metrics.allocated_bytes > 0);
    println!(
        "heap_metrics allocated_bytes={} pause_micros={}",
        metrics.allocated_bytes, metrics.pause_micros
    );
}

#[test]
fn heap_metrics_exclude_interrupted_gc() {
    // Given: a started major cycle that is never finished.
    let heap = GcHeap::new(GcConfig::default());
    let cycle = heap
        .start_major(&RootInventory::new())
        .expect("major cycle start");

    // When: the cycle is interrupted by dropping its owner.
    drop(cycle);

    // Then: no completed major event or pause is published.
    let metrics = heap.metrics();
    assert_eq!(metrics.major_collections, 0);
    assert_eq!(metrics.pause_micros, 0);
}

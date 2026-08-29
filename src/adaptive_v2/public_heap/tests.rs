use std::sync::Arc;
use std::thread;

use super::operations::NativeHeapContext;
use super::runtime::AdaptiveHeapRuntime;
use super::types::{HeapAdapterError, HeapValue, PayloadKind};
use crate::adaptive_v2::heap::{GcConfig, GcError};
use crate::adaptive_v2::value_word::ScalarValue;

mod heap_metrics;
mod implicit_collection;

fn integer(runtime: &AdaptiveHeapRuntime, value: i64) -> HeapValue {
    runtime
        .scalar(ScalarValue::Integer(value))
        .expect("integer encoding should fit")
        .value()
}

#[test]
fn rooted_clone_keeps_payload_alive_until_last_clone_drops() {
    // Given: a rooted object and a cloned host-root lease.
    let runtime = AdaptiveHeapRuntime::new(GcConfig::default());
    let rooted = runtime.allocate_object().expect("object allocation");
    let copied = rooted.value();
    let clone = rooted.clone();

    // When: the original root is dropped and a collection runs.
    drop(rooted);
    runtime.collect_minor().expect("minor collection");

    // Then: the clone preserves the payload, and its final drop permits reclamation.
    assert_eq!(
        runtime.object_set(&clone, "answer", integer(&runtime, 42)),
        Ok(())
    );
    drop(clone);
    runtime.collect_minor().expect("final collection");
    assert!(matches!(
        runtime.root(copied),
        Err(HeapAdapterError::StaleHandle)
    ));
    assert_eq!(runtime.payload_counts(), (0, 0, 0));
}

#[test]
fn independent_roots_are_counted_and_cross_runtime_values_are_rejected() {
    // Given: two independent leases for one object and a foreign runtime.
    let runtime = AdaptiveHeapRuntime::new(GcConfig::default());
    let first = runtime.allocate_object().expect("object allocation");
    let second = runtime.root(first.value()).expect("second lease");
    let foreign = AdaptiveHeapRuntime::new(GcConfig::default());

    // When: one independent lease is released and collection runs.
    drop(first);
    runtime.collect_minor().expect("minor collection");

    // Then: the second lease remains live and the foreign runtime rejects it.
    assert_eq!(
        runtime.object_set(&second, "x", integer(&runtime, 1)),
        Ok(())
    );
    assert!(matches!(
        foreign.root(second.value()),
        Err(HeapAdapterError::WrongRuntime)
    ));
    drop(second);
    runtime.collect_minor().expect("unrooted collection");
    assert_eq!(runtime.payload_counts(), (0, 0, 0));
}

#[test]
fn allocation_limit_failure_leaves_existing_payload_usable() {
    // Given: a heap whose single slot is occupied by a rooted object.
    let runtime = AdaptiveHeapRuntime::new(GcConfig {
        allocation_limit: Some(1),
        ..GcConfig::default()
    });
    let object = runtime.allocate_object().expect("first allocation");

    // When: a second payload allocation exceeds the configured limit.
    let failure = runtime.allocate_list();

    // Then: failure is typed and the existing object is unchanged and usable.
    assert!(matches!(
        failure,
        Err(HeapAdapterError::Heap(GcError::AllocationLimit))
    ));
    assert_eq!(runtime.payload_counts(), (1, 0, 0));
    assert_eq!(
        runtime.object_set(&object, "x", integer(&runtime, 7)),
        Ok(())
    );
}

#[test]
fn collect_every_allocation_preserves_rooted_graph_and_reclaims_dead_payloads() {
    // Given: collect-before-allocation stress mode and rooted object/list payloads.
    let runtime = AdaptiveHeapRuntime::new(GcConfig {
        collect_every_allocation: true,
        promotion_age: 1,
        allocation_limit: None,
    });
    let object = runtime.allocate_object().expect("object allocation");
    let list = runtime.allocate_list().expect("list allocation");
    runtime
        .list_append(&list, object.value())
        .expect("list reference append");

    // When: repeated minor/major cycles run before and after dropping both roots.
    runtime.collect_minor().expect("minor collection");
    runtime.collect_major().expect("major collection");
    assert_eq!(
        runtime.list_get(&list, 0).map(|value| value.value()),
        Ok(object.value())
    );
    drop(object);
    drop(list);
    runtime.collect_minor().expect("dead nursery collection");
    runtime.collect_major().expect("dead old collection");

    // Then: no detached object/list/call payload survives its stable handle.
    assert_eq!(runtime.payload_counts(), (0, 0, 0));
}

#[test]
fn typed_object_list_and_call_helpers_share_value_words_without_pc_dispatch() {
    // Given: actual dense-object, typed-list, and registered-call payloads.
    let runtime = AdaptiveHeapRuntime::new(GcConfig::default());
    let object = runtime.allocate_object().expect("object allocation");
    let list = runtime.allocate_list().expect("list allocation");
    let callable = runtime
        .register_binary_callable(|left, right| left.saturating_add(right))
        .expect("callable allocation");
    let forty = integer(&runtime, 40);
    let two = integer(&runtime, 2);

    // When: helpers mutate/read/call through the concrete adapter context.
    NativeHeapContext::object_store(&runtime, object.value(), "answer", forty)
        .expect("object helper store");
    NativeHeapContext::list_append_value(&runtime, list.value(), two).expect("list helper append");
    let answer = NativeHeapContext::call_binary_value(
        &runtime,
        callable.value(),
        NativeHeapContext::object_load(&runtime, object.value(), "answer")
            .expect("object helper load")
            .value(),
        NativeHeapContext::list_load(&runtime, list.value(), 0)
            .expect("list helper load")
            .value(),
    )
    .expect("call helper");

    // Then: the result decodes through the same heap-backed ValueWord path.
    let decoded = runtime.decode_scalar(answer.value());
    assert_eq!(decoded, Ok(ScalarValue::Integer(42)));
    println!(
        "adaptive_heap_probe decoded={decoded:?} payloads={:?} roots={}",
        runtime.payload_counts(),
        runtime.root_inventory().handles().count()
    );
    assert!(matches!(
        NativeHeapContext::list_load(&runtime, object.value(), 0),
        Err(HeapAdapterError::MissingPayload(PayloadKind::List))
    ));
}

#[test]
fn list_set_insert_pop_and_len_share_one_rooted_payload() {
    // Given: a collect-every-allocation runtime and one rooted integer list.
    let runtime = AdaptiveHeapRuntime::new(GcConfig {
        collect_every_allocation: true,
        ..GcConfig::default()
    });
    let list = runtime.allocate_list().expect("list allocation");
    runtime
        .list_append(&list, integer(&runtime, 10))
        .expect("seed append");

    // When: all non-append list operations mutate the same adapter payload.
    runtime
        .list_insert(&list, 0, integer(&runtime, 5))
        .expect("insert");
    runtime
        .list_set(&list, 1, integer(&runtime, 20))
        .expect("set");
    let popped = runtime.list_pop(&list, 0).expect("pop");
    runtime.collect_minor().expect("collection");

    // Then: order, length, and the rooted popped result survive collection exactly.
    assert_eq!(runtime.list_len(&list), Ok(1));
    assert_eq!(
        runtime
            .list_get(&list, 0)
            .and_then(|value| runtime.decode_scalar(value.value())),
        Ok(ScalarValue::Integer(20))
    );
    assert_eq!(
        runtime.decode_scalar(popped.value()),
        Ok(ScalarValue::Integer(5))
    );
}

#[test]
fn concurrent_payload_operations_use_scoped_locks_without_corrupting_gc_edges() {
    // Given: one runtime with independent rooted lists shared by worker threads.
    let runtime = Arc::new(AdaptiveHeapRuntime::new(GcConfig::default()));
    let lists: Vec<_> = (0..8)
        .map(|_| runtime.allocate_list().expect("list allocation"))
        .collect();

    // When: each thread mutates a distinct payload while collections run between joins.
    let workers: Vec<_> = lists
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, list)| {
            let runtime = Arc::clone(&runtime);
            thread::spawn(move || {
                for offset in 0..64 {
                    let value = integer(&runtime, i64::try_from(index * 64 + offset).unwrap_or(0));
                    runtime.list_append(&list, value)?;
                }
                Ok::<_, HeapAdapterError>(list)
            })
        })
        .collect();
    let completed: Vec<_> = workers
        .into_iter()
        .map(|worker| {
            worker
                .join()
                .expect("worker should not panic")
                .expect("append")
        })
        .collect();
    runtime.collect_minor().expect("minor collection");

    // Then: each scoped payload retains its complete independent sequence.
    for (index, list) in completed.iter().enumerate() {
        assert_eq!(
            runtime
                .list_get(list, 63)
                .and_then(|value| runtime.decode_scalar(value.value())),
            Ok(ScalarValue::Integer(
                i64::try_from(index * 64 + 63).unwrap_or(0)
            ))
        );
    }
}

#[test]
fn reclaimed_handle_reports_stale_not_a_detached_payload() {
    // Given: a list value retained after its only root is dropped.
    let runtime = AdaptiveHeapRuntime::new(GcConfig::default());
    let list = runtime.allocate_list().expect("list allocation");
    let stale = list.value();
    drop(list);

    // When: collection releases the stable handle and its payload.
    runtime.collect_minor().expect("minor collection");

    // Then: the exact stale-handle class is preserved at the adapter boundary.
    assert!(matches!(
        runtime.root(stale),
        Err(HeapAdapterError::StaleHandle)
    ));
}

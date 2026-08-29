use super::super::runtime::AdaptiveHeapRuntime;
use super::super::types::{HeapAdapterError, PayloadKind};
use crate::adaptive_v2::heap::GcConfig;

#[test]
fn collect_before_allocation_prunes_payload_that_died_in_implicit_collection() {
    // Given: collect-before-allocation mode and dead object, list, and callable roots.
    let runtime = AdaptiveHeapRuntime::new(GcConfig {
        collect_every_allocation: true,
        ..GcConfig::default()
    });
    let object = runtime.allocate_object().expect("object allocation");
    let stale_object = object.value();
    drop(object);

    // When: successive allocations implicitly collect and prune each prior payload kind.
    let list = runtime.allocate_list().expect("list allocation");
    assert_eq!(runtime.payload_counts(), (0, 1, 0));
    let stale_list = list.value();
    drop(list);
    let callable = runtime
        .register_binary_callable(i64::saturating_add)
        .expect("callable allocation");
    assert_eq!(runtime.payload_counts(), (0, 0, 1));
    let stale_callable = callable.value();
    drop(callable);
    let replacement = runtime.allocate_object().expect("replacement allocation");

    // Then: every detached payload is gone in the allocation that killed its handle.
    assert_eq!(runtime.payload_counts(), (1, 0, 0));
    for (value, kind) in [
        (stale_object, PayloadKind::Object),
        (stale_list, PayloadKind::List),
        (stale_callable, PayloadKind::Callable),
    ] {
        assert!(
            matches!(runtime.root(value), Err(HeapAdapterError::StaleHandle)),
            "{kind}"
        );
    }
    drop(replacement);
}

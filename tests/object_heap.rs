use num_bigint::BigInt;
use wustite::object::{Object, ObjectError, ObjectHeap, ObjectKind};
use wustite::value::Value;

fn string_object(value: &str) -> Object {
    Object::String(value.to_owned())
}

fn assert_rejected<T>(result: Result<T, ObjectError>) {
    assert!(result.is_err());
}

#[test]
fn alloc_returns_readable_handle_kind() {
    // Given: an empty object heap and a string object.
    let mut heap = ObjectHeap::new();

    // When: the object is allocated into the heap.
    let reference = heap.allocate(string_object("hello")).unwrap();

    // Then: reading the handle returns the original object and its kind.
    assert!(matches!(heap.get(reference), Ok(Object::String(value)) if value == "hello"));
    assert_eq!(heap.kind(reference), Ok(ObjectKind::String));
}

#[test]
fn stale_handles_rejected_after_slot_reuse() {
    // Given: a live object and its generational handle.
    let mut heap = ObjectHeap::new();
    let stale = heap.allocate(string_object("old")).unwrap();

    // When: the object is removed and another object is allocated.
    assert!(matches!(heap.remove(stale), Ok(Object::String(value)) if value == "old"));
    let current = heap.allocate(string_object("new")).unwrap();

    // Then: the freed slot can be reused, but its generation changes.
    assert_eq!(current.slot(), stale.slot());
    assert_ne!(current.generation(), stale.generation());
    assert_rejected(heap.get(stale));
    assert_rejected(heap.kind(stale));
    assert_rejected(heap.remove(stale));
    assert!(matches!(heap.get(current), Ok(Object::String(value)) if value == "new"));
}

#[test]
fn handles_reject_foreign_heap() {
    // Given: a reference allocated by one heap and a separate heap.
    let mut owner = ObjectHeap::new();
    let foreign = owner.allocate(string_object("private")).unwrap();
    let mut other = ObjectHeap::new();
    let local = other.allocate(string_object("local")).unwrap();

    // When: the foreign handle is presented to the other heap.
    assert_ne!(foreign.heap_id(), local.heap_id());
    // Then: every operation rejects the mismatched heap identity.
    assert_rejected(other.get(foreign));
    assert_rejected(other.kind(foreign));
    assert_rejected(other.remove(foreign));
}

#[test]
fn object_kinds_containers_keep_nested_refs() {
    // Given: a heap containing a child string and values of each container kind.
    let mut heap = ObjectHeap::new();
    let child = heap.allocate(string_object("child")).unwrap();
    let tuple = heap
        .allocate(Object::tuple(vec![
            Value::Object(child),
            Value::SmallInt(7),
        ]))
        .unwrap();
    let list = heap
        .allocate(Object::list(vec![Value::Object(child)]))
        .unwrap();
    let dict = heap
        .allocate(Object::Dict(vec![(
            Value::Object(child),
            Value::Bool(true),
        )]))
        .unwrap();
    let bigint = heap
        .allocate(Object::BigInt(BigInt::from(123_i64)))
        .unwrap();

    // When: the container objects are read back through their handles.
    let tuple_value = heap.get(tuple).unwrap();
    let list_value = heap.get(list).unwrap();
    let dict_value = heap.get(dict).unwrap();

    // Then: nested ObjectRefs remain intact and point at the original child.
    assert!(
        matches!(tuple_value, Object::Tuple(values) if values.to_vec() == vec![Value::Object(child), Value::SmallInt(7)])
    );
    assert!(
        matches!(list_value, Object::List(values) if values.to_vec() == vec![Value::Object(child)])
    );
    assert!(
        matches!(dict_value, Object::Dict(entries) if entries == &vec![(Value::Object(child), Value::Bool(true))])
    );

    // And: every allocated shape reports the corresponding ObjectKind.
    assert_eq!(heap.kind(child), Ok(ObjectKind::String));
    assert_eq!(heap.kind(tuple), Ok(ObjectKind::Tuple));
    assert_eq!(heap.kind(list), Ok(ObjectKind::List));
    assert_eq!(heap.kind(dict), Ok(ObjectKind::Dict));
    assert_eq!(heap.kind(bigint), Ok(ObjectKind::BigInt));
}

use super::super::heap::{GcConfig, GcHeap, GcObject};
use super::super::lists::{ListError, ListStrategy, TypedList};
use super::super::pic::{CallKey, ListKey, ObjectGetKey, ObjectSetKey, Pic, PicState};
use super::super::roots::{RootInventory, RootKind};
use super::super::value_word::{ScalarValue, ValueWord};

#[test]
fn typed_lists_widen_without_losing_bits_handles_order_or_barrier_edges() {
    let heap = GcHeap::new(GcConfig {
        promotion_age: 1,
        ..GcConfig::default()
    });
    let mut list = TypedList::new(&heap).expect("list allocation");
    assert_eq!(list.strategy(), ListStrategy::Empty);
    let integer =
        ValueWord::encode_scalar(ScalarValue::Integer(i64::MIN), &heap).expect("boxed int");
    let small = ValueWord::encode_scalar(ScalarValue::Integer(7), &heap).expect("small int");
    list.push(&heap, small).expect("integer push");
    assert_eq!(list.strategy(), ListStrategy::ImmediateInteger);
    list.push(&heap, integer).expect("boxed widening");
    assert_eq!(list.strategy(), ListStrategy::Generic);

    let target = heap.allocate(GcObject::new()).expect("target allocation");
    let target_word = ValueWord::from_handle(target);
    list.push(&heap, target_word).expect("handle push");
    assert_eq!(list.get(&heap, 0), Ok(small));
    assert_eq!(list.get(&heap, 1), Ok(integer));
    assert_eq!(list.get(&heap, 2), Ok(target_word));
    assert_eq!(list.get(&heap, 3), Err(ListError::IndexOutOfBounds));

    let mut roots = RootInventory::new();
    roots.insert(RootKind::FrameRegister, list.handle());
    heap.minor_collect(&roots).expect("promote owner");
    let young = heap.allocate(GcObject::new()).expect("young target");
    list.set(&heap, 2, ValueWord::from_handle(young))
        .expect("old to young set");
    heap.minor_collect(&roots)
        .expect("barrier preserves target");
    assert!(heap.resolve(young).is_ok());
}

#[test]
fn f64_lists_preserve_nan_bits_and_mutations_invalidate_layout_keys() {
    let heap = GcHeap::new(GcConfig::default());
    let mut list = TypedList::new(&heap).expect("list allocation");
    let nan_bits = 0x7ff8_1234_5678_9abc;
    let nan = ValueWord::encode_scalar(ScalarValue::FloatBits(nan_bits), &heap).expect("nan");
    list.push(&heap, nan).expect("float push");
    assert_eq!(list.strategy(), ListStrategy::F64);
    assert_eq!(
        list.get(&heap, 0).expect("float read").decode_scalar(&heap),
        Ok(ScalarValue::FloatBits(nan_bits))
    );
    let old = list.key();
    list.set(&heap, 0, nan).expect("float set");
    assert_ne!(old, list.key());
}

#[test]
fn every_pic_specializes_four_cases_then_uses_working_generic_fallback() {
    let mut object_gets = Pic::<ObjectGetKey, u32>::new();
    let mut object_sets = Pic::<ObjectSetKey, u32>::new();
    let mut calls = Pic::<CallKey, u32>::new();
    let mut lists = Pic::<ListKey, u32>::new();
    for case in 0..4 {
        object_gets.observe(ObjectGetKey::test(case), case);
        object_sets.observe(ObjectSetKey::test(case), case);
        calls.observe(CallKey::test(case), case);
        lists.observe(ListKey::test(case), case);
    }
    assert_eq!(object_gets.state(), PicState::Specialized { cases: 4 });
    assert_eq!(object_sets.state(), PicState::Specialized { cases: 4 });
    assert_eq!(calls.state(), PicState::Specialized { cases: 4 });
    assert_eq!(lists.state(), PicState::Specialized { cases: 4 });
    object_gets.observe(ObjectGetKey::test(4), 4);
    object_sets.observe(ObjectSetKey::test(4), 4);
    calls.observe(CallKey::test(4), 4);
    lists.observe(ListKey::test(4), 4);
    assert_eq!(object_gets.resolve_or(ObjectGetKey::test(99), || 99), 99);
    assert_eq!(object_sets.resolve_or(ObjectSetKey::test(99), || 99), 99);
    assert_eq!(calls.resolve_or(CallKey::test(99), || 99), 99);
    assert_eq!(lists.resolve_or(ListKey::test(99), || 99), 99);
    assert_eq!(object_gets.state(), PicState::Generic);
    assert_eq!(object_sets.state(), PicState::Generic);
    assert_eq!(calls.state(), PicState::Generic);
    assert_eq!(lists.state(), PicState::Generic);
    assert_eq!(object_gets.counters().generic_fallbacks, 1);
    assert_eq!(object_sets.counters().generic_fallbacks, 1);
}

#[test]
fn epoch_changes_miss_instead_of_returning_stale_pic_values() {
    let mut object = Pic::<ObjectGetKey, u32>::new();
    let old = ObjectGetKey::new(7, 1, 1);
    object.observe(old, 41);
    assert_eq!(object.resolve_or(old, || 0), 41);
    assert_eq!(object.resolve_or(ObjectGetKey::new(7, 2, 1), || 42), 42);
    assert_eq!(object.resolve_or(ObjectGetKey::new(7, 1, 2), || 43), 43);
    assert_eq!(object.counters().hits, 1);
    assert_eq!(object.counters().misses, 2);
}

#[test]
fn list_allocation_and_cross_heap_stable_handle_failures_are_typed() {
    let limited = GcHeap::new(GcConfig {
        allocation_limit: Some(0),
        ..GcConfig::default()
    });
    assert!(matches!(
        TypedList::new(&limited),
        Err(super::super::heap::GcError::AllocationLimit)
    ));

    let first = GcHeap::new(GcConfig::default());
    let second = GcHeap::new(GcConfig::default());
    let foreign = first.allocate(GcObject::new()).expect("foreign object");
    let list = TypedList::new(&second).expect("local list");
    assert!(matches!(
        second.store_reference(list.handle(), foreign),
        Err(super::super::heap::GcError::InvalidHandle(_))
    ));
}

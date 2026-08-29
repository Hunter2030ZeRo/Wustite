use super::*;

#[test]
fn smallint_add_returns_immediate_when_in_range() {
    let mut heap = ObjectHeap::new();
    let mut ops = ValueOps::new(&mut heap);

    assert_eq!(ops.smallint_add(40, 2).unwrap(), Value::SmallInt(42));
    assert_eq!(ops.smallint_add(-40, -2).unwrap(), Value::SmallInt(-42));
}

#[test]
fn smallint_add_promotes_both_overflow_directions() {
    let mut heap = ObjectHeap::new();
    let upper = ValueOps::new(&mut heap).smallint_add(i64::MAX, 1).unwrap();
    let lower = ValueOps::new(&mut heap).smallint_add(i64::MIN, -1).unwrap();

    let Value::Object(upper) = upper else {
        panic!("upper overflow did not allocate a BigInt")
    };
    let Value::Object(lower) = lower else {
        panic!("lower overflow did not allocate a BigInt")
    };
    assert_eq!(
        heap.get(upper).unwrap(),
        &Object::BigInt(BigInt::from(i64::MAX) + 1)
    );
    assert_eq!(
        heap.get(lower).unwrap(),
        &Object::BigInt(BigInt::from(i64::MIN) - 1)
    );
}

use wustite::object::{SequenceObject, SequenceStrategy};
use wustite::value::Value;

#[test]
fn homogeneous_values_select_typed_strategy() {
    // Given: a sequence containing only small integers.
    let sequence = SequenceObject::from_values(vec![Value::SmallInt(3), Value::SmallInt(5)]);

    // When: its storage strategy and elements are inspected.
    let values = sequence.iter().collect::<Vec<_>>();

    // Then: the sequence uses unboxed i64 storage without changing Value identity.
    assert_eq!(sequence.strategy(), SequenceStrategy::I64);
    assert_eq!(values, vec![Value::SmallInt(3), Value::SmallInt(5)]);
}

#[test]
fn type_mismatch_generalizes_storage_invalidates_layout() {
    // Given: a typed integer sequence with a stable layout version.
    let mut sequence = SequenceObject::from_values(vec![Value::SmallInt(7)]);
    let initial_version = sequence.layout_version();

    // When: a float replaces the integer element.
    assert_eq!(sequence.set(0, Value::Float(7.0)), Some(Value::SmallInt(7)));

    // Then: storage generalizes rather than coercing the value, and the layout changes once.
    assert_eq!(sequence.strategy(), SequenceStrategy::Object);
    assert_eq!(sequence.get(0), Some(Value::Float(7.0)));
    assert_eq!(sequence.layout_version(), initial_version + 1);
}

#[test]
fn strategy_write_keeps_layout_length_change_invalidates() {
    // Given: a homogeneous boolean sequence.
    let mut sequence = SequenceObject::from_values(vec![Value::Bool(false)]);
    let initial_version = sequence.layout_version();

    // When: an element is replaced with the same strategy and another is appended.
    assert_eq!(sequence.set(0, Value::Bool(true)), Some(Value::Bool(false)));
    let after_set = sequence.layout_version();
    sequence.push(Value::Bool(false));

    // Then: scalar replacement keeps the borrowed layout valid, while growth invalidates it.
    assert_eq!(after_set, initial_version);
    assert_eq!(sequence.layout_version(), initial_version + 1);
    assert_eq!(
        sequence.iter().collect::<Vec<_>>(),
        vec![Value::Bool(true), Value::Bool(false)]
    );
}

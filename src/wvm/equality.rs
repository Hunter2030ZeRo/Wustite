use std::collections::HashSet;

use num_bigint::BigInt;

use crate::object::{Object, ObjectHeap, ObjectRef, SequenceObject};
use crate::value::Value;

use super::arithmetic::numeric_semantics;

pub(super) fn values_equal(heap: &ObjectHeap, lhs: Value, rhs: Value) -> Result<bool, String> {
    values_equal_with(heap, lhs, rhs, &mut HashSet::new())
}

fn values_equal_with(
    heap: &ObjectHeap,
    lhs: Value,
    rhs: Value,
    visiting: &mut HashSet<(ObjectRef, ObjectRef)>,
) -> Result<bool, String> {
    match (lhs, rhs) {
        (Value::SmallInt(lhs), Value::SmallInt(rhs)) => Ok(lhs == rhs),
        (Value::Float(lhs), Value::Float(rhs)) => Ok(lhs == rhs),
        (Value::SmallInt(lhs), Value::Float(rhs)) => Ok(numeric_semantics::integer_float_equal(
            &BigInt::from(lhs),
            rhs,
        )),
        (Value::Float(lhs), Value::SmallInt(rhs)) => Ok(numeric_semantics::integer_float_equal(
            &BigInt::from(rhs),
            lhs,
        )),
        (Value::Bool(lhs), Value::Bool(rhs)) => Ok(lhs == rhs),
        (Value::Object(lhs), Value::Object(rhs)) if lhs == rhs => heap
            .get(lhs)
            .map(|_| true)
            .map_err(|error| error.to_string()),
        (Value::Object(lhs), Value::Object(rhs)) => object_equal(heap, lhs, rhs, visiting),
        (Value::SmallInt(value), Value::Object(reference))
        | (Value::Object(reference), Value::SmallInt(value)) => match heap.get(reference) {
            Ok(Object::BigInt(big)) => Ok(big == &value.into()),
            Ok(_) => Ok(false),
            Err(error) => Err(error.to_string()),
        },
        (Value::Float(value), Value::Object(reference))
        | (Value::Object(reference), Value::Float(value)) => match heap.get(reference) {
            Ok(Object::BigInt(big)) => Ok(numeric_semantics::integer_float_equal(big, value)),
            Ok(_) => Ok(false),
            Err(error) => Err(error.to_string()),
        },
        (Value::Uninitialized, Value::Uninitialized) => Ok(true),
        _ => Ok(false),
    }
}

fn object_equal(
    heap: &ObjectHeap,
    lhs_ref: ObjectRef,
    rhs_ref: ObjectRef,
    visiting: &mut HashSet<(ObjectRef, ObjectRef)>,
) -> Result<bool, String> {
    match (heap.get(lhs_ref), heap.get(rhs_ref)) {
        (Ok(Object::String(lhs)), Ok(Object::String(rhs))) => Ok(lhs == rhs),
        (Ok(Object::BigInt(lhs)), Ok(Object::BigInt(rhs))) => Ok(lhs == rhs),
        (Ok(Object::Tuple(lhs)), Ok(Object::Tuple(rhs)))
        | (Ok(Object::List(lhs)), Ok(Object::List(rhs))) => {
            with_visited_pair(visiting, (lhs_ref, rhs_ref), |visiting| {
                sequence_equal(heap, lhs, rhs, visiting)
            })
        }
        (Ok(Object::Dict(lhs)), Ok(Object::Dict(rhs))) => {
            with_visited_pair(visiting, (lhs_ref, rhs_ref), |visiting| {
                dict_equal(heap, lhs, rhs, visiting)
            })
        }
        (Ok(Object::Function(lhs)), Ok(Object::Function(rhs))) => Ok(lhs.id() == rhs.id()),
        (Err(error), _) | (_, Err(error)) => Err(error.to_string()),
        _ => Ok(false),
    }
}

fn with_visited_pair<F>(
    visiting: &mut HashSet<(ObjectRef, ObjectRef)>,
    pair: (ObjectRef, ObjectRef),
    compare: F,
) -> Result<bool, String>
where
    F: FnOnce(&mut HashSet<(ObjectRef, ObjectRef)>) -> Result<bool, String>,
{
    if visiting.contains(&pair) || visiting.contains(&(pair.1, pair.0)) {
        return Ok(true);
    }
    visiting.insert(pair);
    let result = compare(visiting);
    visiting.remove(&pair);
    result
}

fn sequence_equal(
    heap: &ObjectHeap,
    lhs: &SequenceObject,
    rhs: &SequenceObject,
    visiting: &mut HashSet<(ObjectRef, ObjectRef)>,
) -> Result<bool, String> {
    if lhs.len() != rhs.len() {
        return Ok(false);
    }
    for (lhs, rhs) in lhs.iter().zip(rhs.iter()) {
        if !values_equal_with(heap, lhs, rhs, visiting)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn dict_equal(
    heap: &ObjectHeap,
    lhs: &[(Value, Value)],
    rhs: &[(Value, Value)],
    visiting: &mut HashSet<(ObjectRef, ObjectRef)>,
) -> Result<bool, String> {
    if lhs.len() != rhs.len() {
        return Ok(false);
    }
    for (key, value) in lhs {
        let Some(index) = find_key_with(heap, rhs, *key, visiting)? else {
            return Ok(false);
        };
        if !values_equal_with(heap, *value, rhs[index].1, visiting)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn find_key(
    heap: &ObjectHeap,
    entries: &[(Value, Value)],
    key: Value,
) -> Result<Option<usize>, String> {
    find_key_with(heap, entries, key, &mut HashSet::new())
}

fn find_key_with(
    heap: &ObjectHeap,
    entries: &[(Value, Value)],
    key: Value,
    visiting: &mut HashSet<(ObjectRef, ObjectRef)>,
) -> Result<Option<usize>, String> {
    for (index, (candidate, _)) in entries.iter().enumerate() {
        if values_equal_with(heap, *candidate, key, visiting)? {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

pub(super) fn ensure_hashable(heap: &ObjectHeap, value: Value) -> Result<(), String> {
    match value {
        Value::SmallInt(_) | Value::Float(_) | Value::Bool(_) | Value::None => Ok(()),
        Value::Uninitialized => Err("uninitialized value is not hashable".to_string()),
        Value::Object(reference) => match heap.get(reference) {
            Ok(
                Object::String(_)
                | Object::BigInt(_)
                | Object::Function(_)
                | Object::Class(_)
                | Object::Instance(_)
                | Object::BoundMethod(_),
            ) => Ok(()),
            Ok(Object::Tuple(values)) => {
                for value in values.iter() {
                    ensure_hashable(heap, value)?;
                }
                Ok(())
            }
            Ok(Object::List(_) | Object::Dict(_)) => {
                Err("list and dict values are not hashable".to_string())
            }
            Err(error) => Err(error.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_stale_object_references_are_validated_before_equality() {
        // Given: an object reference whose heap slot has been vacated.
        let mut heap = ObjectHeap::new();
        let reference = heap.allocate(Object::String("stale".into())).unwrap();
        heap.remove(reference).unwrap();

        // When: the same stale handle is compared to itself.
        let error =
            values_equal(&heap, Value::Object(reference), Value::Object(reference)).unwrap_err();

        // Then: equality reports the invalid handle instead of returning true.
        assert!(error.contains("vacant"));
    }
}

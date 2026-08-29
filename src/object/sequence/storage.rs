use crate::value::{RuntimeSlot, Value};

#[derive(Clone)]
pub(super) enum SequenceStorage {
    Empty,
    Bool(Vec<u8>),
    I64(Vec<i64>),
    F64(Vec<f64>),
    Object(Vec<RuntimeSlot>),
}

pub(super) fn storage_from_values(values: Vec<Value>) -> SequenceStorage {
    if values.is_empty() {
        SequenceStorage::Empty
    } else if values.iter().all(|value| matches!(value, Value::Bool(_))) {
        SequenceStorage::Bool(
            values
                .into_iter()
                .map(|value| match value {
                    Value::Bool(value) => u8::from(value),
                    _ => unreachable!(),
                })
                .collect(),
        )
    } else if values
        .iter()
        .all(|value| matches!(value, Value::SmallInt(_)))
    {
        SequenceStorage::I64(
            values
                .into_iter()
                .map(|value| match value {
                    Value::SmallInt(value) => value,
                    _ => unreachable!(),
                })
                .collect(),
        )
    } else if values.iter().all(|value| matches!(value, Value::Float(_))) {
        SequenceStorage::F64(
            values
                .into_iter()
                .map(|value| match value {
                    Value::Float(value) => value,
                    _ => unreachable!(),
                })
                .collect(),
        )
    } else {
        SequenceStorage::Object(values.into_iter().map(RuntimeSlot::from_value).collect())
    }
}

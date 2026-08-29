use std::collections::HashMap;

use crate::adaptive_v2::native::{AdaptiveNativeContext, NativeValue};
use crate::object::{Object, ObjectHeap, ObjectRef};
use crate::value::Value;

use super::Operation;

pub(super) struct Binding {
    pub(super) native: NativeValue,
    legacy: Object,
    fields: Vec<String>,
}

enum Snapshot {
    Unchanged,
    InstanceFields(Vec<(String, i64)>),
    List(Vec<Value>),
}

pub(super) fn ensure(
    context: &mut AdaptiveNativeContext,
    bindings: &mut HashMap<ObjectRef, Binding>,
    reference: ObjectRef,
    heap: &mut ObjectHeap,
) -> Result<NativeValue, String> {
    if let Some(binding) = bindings.get(&reference) {
        return Ok(binding.native);
    }
    let legacy = heap
        .transfer_out(reference)
        .map_err(|error| error.to_string())?;
    let imported = import(context, &legacy);
    let (native, fields) = match imported {
        Ok(binding) => binding,
        Err(error) => {
            heap.transfer_in(reference, legacy)
                .map_err(|restore| restore.to_string())?;
            return Err(error);
        }
    };
    bindings.insert(
        reference,
        Binding {
            native,
            legacy,
            fields,
        },
    );
    Ok(native)
}

pub(super) fn hand_back(
    context: &mut AdaptiveNativeContext,
    bindings: &mut HashMap<ObjectRef, Binding>,
    reference: ObjectRef,
    operation: &Operation,
    heap: &mut ObjectHeap,
) -> Result<(), String> {
    let Binding {
        native,
        legacy,
        mut fields,
    } = bindings
        .remove(&reference)
        .ok_or_else(|| "adaptive object transfer is missing".to_owned())?;
    if let Operation::ObjectSet { field, .. } = operation
        && !fields.contains(field)
    {
        fields.push(field.clone());
    }
    let snapshot = snapshot(context, native, operation, &fields);
    let result = match snapshot {
        Ok(Snapshot::Unchanged) => heap
            .transfer_in(reference, legacy)
            .map_err(|error| error.to_string()),
        Ok(Snapshot::List(values)) => heap
            .transfer_in(reference, Object::list(values))
            .map_err(|error| error.to_string()),
        Ok(Snapshot::InstanceFields(values)) => heap
            .transfer_in(reference, legacy)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                for (field, value) in values {
                    heap.set_attribute(reference, field, Value::SmallInt(value))
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            }),
        Err(error) => heap
            .transfer_in(reference, legacy)
            .map_err(|restore| restore.to_string())
            .and(Err(error)),
    };
    let discarded = context
        .discard_value(native)
        .map_err(|error| error.to_string());
    result.and(discarded)
}

pub(super) fn hand_back_entry(
    context: &mut AdaptiveNativeContext,
    bindings: &mut HashMap<ObjectRef, Binding>,
    reference: ObjectRef,
    heap: &mut ObjectHeap,
) -> Result<(), String> {
    let Binding {
        native,
        legacy,
        fields,
    } = bindings
        .remove(&reference)
        .ok_or_else(|| "adaptive object transfer is missing".to_owned())?;
    let snapshot = match &legacy {
        Object::Instance(_) => fields
            .iter()
            .map(|field| {
                context
                    .get_integer_field(native, 0, field)
                    .map(|value| (field.clone(), value))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Snapshot::InstanceFields),
        Object::List(_) => {
            let length = context
                .list_len(native)
                .map_err(|error| error.to_string())?;
            (0..length)
                .map(|index| {
                    context
                        .integer_at(native, index)
                        .map(Value::SmallInt)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Snapshot::List)
        }
        _ => Ok(Snapshot::Unchanged),
    };
    let restored = match snapshot {
        Ok(Snapshot::Unchanged) => heap
            .transfer_in(reference, legacy)
            .map_err(|error| error.to_string()),
        Ok(Snapshot::List(values)) => heap
            .transfer_in(reference, Object::list(values))
            .map_err(|error| error.to_string()),
        Ok(Snapshot::InstanceFields(values)) => heap
            .transfer_in(reference, legacy)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                for (field, value) in values {
                    heap.set_attribute(reference, field, Value::SmallInt(value))
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            }),
        Err(error) => heap
            .transfer_in(reference, legacy)
            .map_err(|restore| restore.to_string())
            .and(Err(error)),
    };
    let discarded = context
        .discard_value(native)
        .map_err(|error| error.to_string());
    restored.and(discarded)
}

fn import(
    context: &mut AdaptiveNativeContext,
    object: &Object,
) -> Result<(NativeValue, Vec<String>), String> {
    match object {
        Object::Instance(instance) => {
            let fields = instance
                .fields()
                .iter()
                .map(|(name, value)| match value {
                    Value::SmallInt(value) => Ok((name.clone(), *value)),
                    Value::Float(_)
                    | Value::Bool(_)
                    | Value::None
                    | Value::Object(_)
                    | Value::Uninitialized => {
                        Err("adaptive object fields require integers".to_owned())
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            let object = context
                .allocate_object()
                .map_err(|error| error.to_string())?;
            for (index, (name, value)) in fields.iter().enumerate() {
                let key = i64::MIN.saturating_add(i64::try_from(index).unwrap_or(i64::MAX));
                context
                    .set_integer_field(object, key, name, *value)
                    .map_err(|error| error.to_string())?;
            }
            Ok((object, fields.into_iter().map(|(name, _)| name).collect()))
        }
        Object::List(values) => {
            let list = context.allocate_list().map_err(|error| error.to_string())?;
            for value in values.iter() {
                let Value::SmallInt(value) = value else {
                    return Err("adaptive list transfer requires integers".to_owned());
                };
                context
                    .append_integer(list, value)
                    .map_err(|error| error.to_string())?;
            }
            Ok((list, Vec::new()))
        }
        Object::String(_)
        | Object::Tuple(_)
        | Object::BigInt(_)
        | Object::Dict(_)
        | Object::Function(_)
        | Object::Class(_)
        | Object::BoundMethod(_) => Err("unsupported adaptive object transfer".to_owned()),
    }
}

fn snapshot(
    context: &mut AdaptiveNativeContext,
    native: NativeValue,
    operation: &Operation,
    fields: &[String],
) -> Result<Snapshot, String> {
    match operation {
        Operation::ObjectSet { .. } => fields
            .iter()
            .map(|field| {
                context
                    .get_integer_field(native, 0, field)
                    .map(|value| (field.clone(), value))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Snapshot::InstanceFields),
        Operation::ListAppend { .. }
        | Operation::ListSet { .. }
        | Operation::ListInsert { .. }
        | Operation::ListPop { .. } => {
            let length = context
                .list_len(native)
                .map_err(|error| error.to_string())?;
            (0..length)
                .map(|index| {
                    context
                        .integer_at(native, index)
                        .map(Value::SmallInt)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Snapshot::List)
        }
        Operation::ObjectGet { .. }
        | Operation::ListGet { .. }
        | Operation::ListLength { .. }
        | Operation::DirectCall { .. } => Ok(Snapshot::Unchanged),
    }
}

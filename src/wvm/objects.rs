use crate::object::{Object, ObjectHeap};
use crate::value::Value;
use num_traits::ToPrimitive;

use super::equality::{ensure_hashable, find_key};

pub(super) struct ObjectOps<'a> {
    heap: &'a mut ObjectHeap,
}

impl<'a> ObjectOps<'a> {
    pub(super) const fn new(heap: &'a mut ObjectHeap) -> Self {
        Self { heap }
    }

    pub(super) fn tuple(&mut self, values: Vec<Value>) -> Result<Value, String> {
        self.allocate(Object::Tuple(values))
    }

    pub(super) fn list(&mut self, values: Vec<Value>) -> Result<Value, String> {
        self.allocate(Object::List(values))
    }

    pub(super) fn dict(&mut self, entries: Vec<(Value, Value)>) -> Result<Value, String> {
        let mut result: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            ensure_hashable(self.heap, key)?;
            if let Some(index) = find_key(self.heap, &result, key)? {
                result[index].1 = value;
            } else {
                result.push((key, value));
            }
        }
        self.heap
            .allocate_runtime_dict(result)
            .map(Value::Object)
            .map_err(|error| error.to_string())
    }

    pub(super) fn get_item(&mut self, object: Value, key: Value) -> Result<Value, String> {
        let Value::Object(reference) = object else {
            return Err("item access requires an object".to_string());
        };
        match self
            .heap
            .get(reference)
            .map_err(|error| error.to_string())?
        {
            Object::Tuple(values) | Object::List(values) => {
                let index = sequence_index(self.heap, key, values.len())?;
                values
                    .get(index)
                    .copied()
                    .ok_or_else(|| "sequence index out of range".to_string())
            }
            Object::String(value) => {
                let length = value.chars().count();
                let index = sequence_index(self.heap, key, length)?;
                let selected = value
                    .chars()
                    .nth(index)
                    .ok_or_else(|| "string index out of range".to_string())?;
                self.allocate(Object::String(selected.to_string()))
            }
            Object::Dict(entries) => {
                ensure_hashable(self.heap, key)?;
                let index = find_key(self.heap, entries, key)?
                    .ok_or_else(|| "dictionary key not found".to_string())?;
                Ok(entries[index].1)
            }
            Object::BigInt(_) | Object::Function(_) => {
                Err("object does not support item access".to_string())
            }
        }
    }

    pub(super) fn set_item(
        &mut self,
        object: Value,
        key: Value,
        value: Value,
    ) -> Result<(), String> {
        let Value::Object(reference) = object else {
            return Err("item assignment requires an object".to_string());
        };
        let kind = self
            .heap
            .kind(reference)
            .map_err(|error| error.to_string())?;
        match kind {
            crate::object::ObjectKind::List => {
                let length = match self.heap.get(reference) {
                    Ok(Object::List(values)) => values.len(),
                    Ok(_) => return Err("item assignment requires list or dict".to_string()),
                    Err(error) => return Err(error.to_string()),
                };
                let index = sequence_index(self.heap, key, length)?;
                match self.heap.get_mut(reference) {
                    Ok(Object::List(values)) => {
                        let slot = values
                            .get_mut(index)
                            .ok_or_else(|| "list index out of range".to_string())?;
                        *slot = value;
                        Ok(())
                    }
                    Ok(_) => Err("item assignment requires list or dict".to_string()),
                    Err(error) => Err(error.to_string()),
                }
            }
            crate::object::ObjectKind::Dict => {
                ensure_hashable(self.heap, key)?;
                let index = match self.heap.get(reference) {
                    Ok(Object::Dict(entries)) => find_key(self.heap, entries, key)?,
                    Ok(_) => return Err("item assignment requires list or dict".to_string()),
                    Err(error) => return Err(error.to_string()),
                };
                match self.heap.get_mut(reference) {
                    Ok(Object::Dict(entries)) => {
                        if let Some(index) = index {
                            entries[index].1 = value;
                        } else {
                            entries.push((key, value));
                        }
                        Ok(())
                    }
                    Ok(_) => Err("item assignment requires list or dict".to_string()),
                    Err(error) => Err(error.to_string()),
                }
            }
            crate::object::ObjectKind::String
            | crate::object::ObjectKind::Tuple
            | crate::object::ObjectKind::BigInt
            | crate::object::ObjectKind::Function => {
                Err("item assignment requires list or dict".to_string())
            }
        }
    }

    pub(super) fn length(&self, value: Value) -> Result<Value, String> {
        let Value::Object(reference) = value else {
            return Err("length requires an object".to_string());
        };
        let length = match self
            .heap
            .get(reference)
            .map_err(|error| error.to_string())?
        {
            Object::String(value) => value.chars().count(),
            Object::Tuple(values) | Object::List(values) => values.len(),
            Object::Dict(entries) => entries.len(),
            Object::BigInt(_) | Object::Function(_) => {
                return Err("object has no length".to_string());
            }
        };
        i64::try_from(length)
            .map(Value::SmallInt)
            .map_err(|_| "object length exceeds SmallInt range".to_string())
    }

    fn allocate(&mut self, object: Object) -> Result<Value, String> {
        self.heap
            .allocate(object)
            .map(Value::Object)
            .map_err(|error| error.to_string())
    }
}

fn sequence_index(heap: &ObjectHeap, key: Value, length: usize) -> Result<usize, String> {
    let index = match key {
        Value::SmallInt(index) => index,
        Value::Object(reference) => match heap.get(reference).map_err(|error| error.to_string())? {
            Object::BigInt(index) => index
                .to_i64()
                .ok_or_else(|| "sequence index out of range".to_string())?,
            Object::String(_)
            | Object::Tuple(_)
            | Object::List(_)
            | Object::Dict(_)
            | Object::Function(_) => return Err("sequence index must be an integer".to_string()),
        },
        Value::Float(_) | Value::Bool(_) | Value::Uninitialized => {
            return Err("sequence index must be an integer".to_string());
        }
    };
    let length = i64::try_from(length).map_err(|_| "sequence is too large".to_string())?;
    let normalized = if index < 0 {
        length
            .checked_add(index)
            .ok_or_else(|| "sequence index out of range".to_string())?
    } else {
        index
    };
    if normalized < 0 || normalized >= length {
        return Err("sequence index out of range".to_string());
    }
    usize::try_from(normalized).map_err(|_| "sequence index out of range".to_string())
}

use crate::object::Object;
use crate::value::Value;

use super::super::equality::{ensure_hashable, find_key};
use super::{ObjectOps, sequence_index};

impl ObjectOps<'_> {
    pub(in super::super) fn get_item(
        &mut self,
        object: Value,
        key: Value,
    ) -> Result<Value, String> {
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
            Object::BigInt(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => Err("object does not support item access".to_string()),
        }
    }

    pub(in super::super) fn set_item(
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
                    Ok(Object::List(values)) => values
                        .set(index, value)
                        .map(|_| ())
                        .ok_or_else(|| "list index out of range".to_string()),
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
            | crate::object::ObjectKind::Function
            | crate::object::ObjectKind::Class
            | crate::object::ObjectKind::Instance
            | crate::object::ObjectKind::BoundMethod => {
                Err("item assignment requires list or dict".to_string())
            }
        }
    }
}

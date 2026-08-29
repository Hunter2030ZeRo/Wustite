use crate::object::Object;
use crate::value::Value;

use super::{ObjectOps, raw_index, sequence_index};

impl ObjectOps<'_> {
    pub(in super::super) fn append_list(
        &mut self,
        list: Value,
        value: Value,
    ) -> Result<(), String> {
        let Value::Object(reference) = list else {
            return Err("list append requires an object".to_string());
        };
        match self.heap.get_mut(reference) {
            Ok(Object::List(values)) => {
                values.push(value);
                Ok(())
            }
            Ok(_) => Err("list append requires a list".to_string()),
            Err(error) => Err(error.to_string()),
        }
    }

    pub(in super::super) fn insert_list(
        &mut self,
        list: Value,
        index: Value,
        value: Value,
    ) -> Result<(), String> {
        let Value::Object(reference) = list else {
            return Err("list insert requires an object".to_string());
        };
        let length = match self
            .heap
            .get(reference)
            .map_err(|error| error.to_string())?
        {
            Object::List(values) => values.len(),
            _ => return Err("list insert requires a list".to_string()),
        };
        let length_i64 = i64::try_from(length).map_err(|_| "list is too large".to_string())?;
        let index = raw_index(self.heap, index)?;
        let normalized = if index < 0 {
            length_i64.saturating_add(index).max(0)
        } else {
            index.min(length_i64)
        };
        let index = usize::try_from(normalized).map_err(|_| "invalid list index".to_string())?;
        let Object::List(values) = self
            .heap
            .get_mut(reference)
            .map_err(|error| error.to_string())?
        else {
            return Err("list insert requires a list".to_string());
        };
        values.insert(index, value);
        Ok(())
    }

    pub(in super::super) fn pop_list(
        &mut self,
        list: Value,
        index: Value,
    ) -> Result<Value, String> {
        let Value::Object(reference) = list else {
            return Err("list pop requires an object".to_string());
        };
        let length = match self
            .heap
            .get(reference)
            .map_err(|error| error.to_string())?
        {
            Object::List(values) => values.len(),
            _ => return Err("list pop requires a list".to_string()),
        };
        let index = sequence_index(self.heap, index, length)?;
        let Object::List(values) = self
            .heap
            .get_mut(reference)
            .map_err(|error| error.to_string())?
        else {
            return Err("list pop requires a list".to_string());
        };
        values
            .remove(index)
            .ok_or_else(|| "list index out of range".to_string())
    }
}

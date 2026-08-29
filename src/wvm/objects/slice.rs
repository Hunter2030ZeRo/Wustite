use crate::object::Object;
use crate::value::Value;

use super::{ObjectOps, forward_slice_bounds, optional_index, slice_indices};

impl ObjectOps<'_> {
    pub(in super::super) fn get_slice(
        &mut self,
        object: Value,
        start: Option<Value>,
        stop: Option<Value>,
        step: Option<Value>,
    ) -> Result<Value, String> {
        let Value::Object(reference) = object else {
            return Err("slice access requires an object".to_string());
        };
        let length = match self
            .heap
            .get(reference)
            .map_err(|error| error.to_string())?
        {
            Object::Tuple(values) | Object::List(values) => values.len(),
            Object::String(value) => value.chars().count(),
            Object::BigInt(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => {
                return Err("object does not support slicing".to_string());
            }
        };
        let indices = slice_indices(self.heap, length, start, stop, step)?;
        let sliced = match self
            .heap
            .get(reference)
            .map_err(|error| error.to_string())?
        {
            Object::Tuple(values) => Object::tuple(
                indices
                    .iter()
                    .map(|index| values.get(*index).expect("validated slice index"))
                    .collect::<Vec<_>>(),
            ),
            Object::List(values) => Object::list(
                indices
                    .iter()
                    .map(|index| values.get(*index).expect("validated slice index"))
                    .collect::<Vec<_>>(),
            ),
            Object::String(value) => {
                let characters = value.chars().collect::<Vec<_>>();
                Object::String(indices.iter().map(|index| characters[*index]).collect())
            }
            Object::BigInt(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => {
                return Err("object does not support slicing".to_string());
            }
        };
        self.allocate(sliced)
    }

    pub(in super::super) fn set_slice(
        &mut self,
        object: Value,
        start: Option<Value>,
        stop: Option<Value>,
        step: Option<Value>,
        replacement: Value,
    ) -> Result<(), String> {
        let Value::Object(reference) = object else {
            return Err("slice assignment requires a list".to_string());
        };
        let Value::Object(replacement_reference) = replacement else {
            return Err("slice assignment value must be a sequence".to_string());
        };
        let replacement = match self
            .heap
            .get(replacement_reference)
            .map_err(|error| error.to_string())?
        {
            Object::Tuple(values) | Object::List(values) => values.to_vec(),
            Object::String(_)
            | Object::BigInt(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => {
                return Err("slice assignment value must be a list or tuple".to_string());
            }
        };
        let length = match self
            .heap
            .get(reference)
            .map_err(|error| error.to_string())?
        {
            Object::List(values) => values.len(),
            Object::String(_)
            | Object::Tuple(_)
            | Object::BigInt(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => {
                return Err("slice assignment requires a list".to_string());
            }
        };
        let step_value = optional_index(self.heap, step)?.unwrap_or(1);
        if step_value == 1 {
            let (start, stop) = forward_slice_bounds(self.heap, length, start, stop)?;
            let Object::List(values) = self
                .heap
                .get_mut(reference)
                .map_err(|error| error.to_string())?
            else {
                return Err("slice assignment requires a list".to_string());
            };
            values.replace_range(start..stop, replacement);
            return Ok(());
        }
        let indices = slice_indices(
            self.heap,
            length,
            start,
            stop,
            Some(Value::SmallInt(step_value)),
        )?;
        if indices.len() != replacement.len() {
            return Err("extended slice assignment size mismatch".to_string());
        }
        let Object::List(values) = self
            .heap
            .get_mut(reference)
            .map_err(|error| error.to_string())?
        else {
            return Err("slice assignment requires a list".to_string());
        };
        for (index, value) in indices.into_iter().zip(replacement) {
            let _ = values.set(index, value);
        }
        Ok(())
    }
}

use crate::object::{Object, ObjectHeap};
use crate::value::Value;
use num_traits::ToPrimitive;

use super::equality::{ensure_hashable, find_key};

mod indexing;
mod list;
mod slice;

pub(super) struct ObjectOps<'a> {
    heap: &'a mut ObjectHeap,
}

impl<'a> ObjectOps<'a> {
    pub(super) const fn new(heap: &'a mut ObjectHeap) -> Self {
        Self { heap }
    }

    pub(super) fn tuple(&mut self, values: Vec<Value>) -> Result<Value, String> {
        self.allocate(Object::tuple(values))
    }

    pub(super) fn list(&mut self, values: Vec<Value>) -> Result<Value, String> {
        self.allocate(Object::list(values))
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
            Object::BigInt(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => {
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
    let index = raw_index(heap, key)?;
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

fn raw_index(heap: &ObjectHeap, key: Value) -> Result<i64, String> {
    match key {
        Value::SmallInt(index) => Ok(index),
        Value::Object(reference) => match heap.get(reference).map_err(|error| error.to_string())? {
            Object::BigInt(index) => index
                .to_i64()
                .ok_or_else(|| "sequence index out of range".to_string()),
            Object::String(_)
            | Object::Tuple(_)
            | Object::List(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => Err("sequence index must be an integer".to_string()),
        },
        Value::Float(_) | Value::Bool(_) | Value::None | Value::Uninitialized => {
            Err("sequence index must be an integer".to_string())
        }
    }
}

pub(super) fn optional_index(
    heap: &ObjectHeap,
    value: Option<Value>,
) -> Result<Option<i64>, String> {
    value.map(|index| raw_index(heap, index)).transpose()
}

pub(super) fn forward_slice_bounds(
    heap: &ObjectHeap,
    length: usize,
    start: Option<Value>,
    stop: Option<Value>,
) -> Result<(usize, usize), String> {
    let length = i64::try_from(length).map_err(|_| "sequence is too large".to_string())?;
    let start = optional_index(heap, start)?.unwrap_or(0);
    let stop = optional_index(heap, stop)?.unwrap_or(length);
    let normalize = |index: i64| {
        if index < 0 {
            length.saturating_add(index).clamp(0, length)
        } else {
            index.clamp(0, length)
        }
    };
    let start = normalize(start);
    let stop = normalize(stop).max(start);
    Ok((
        usize::try_from(start).map_err(|_| "invalid slice start".to_string())?,
        usize::try_from(stop).map_err(|_| "invalid slice stop".to_string())?,
    ))
}

pub(super) fn slice_indices(
    heap: &ObjectHeap,
    length: usize,
    start: Option<Value>,
    stop: Option<Value>,
    step: Option<Value>,
) -> Result<Vec<usize>, String> {
    let length = i64::try_from(length).map_err(|_| "sequence is too large".to_string())?;
    let step = optional_index(heap, step)?.unwrap_or(1);
    if step == 0 {
        return Err("slice step cannot be zero".to_string());
    }
    let mut result = Vec::new();
    if step > 0 {
        let normalize = |index: i64| {
            if index < 0 {
                length.saturating_add(index).clamp(0, length)
            } else {
                index.clamp(0, length)
            }
        };
        let mut index = normalize(optional_index(heap, start)?.unwrap_or(0));
        let stop = normalize(optional_index(heap, stop)?.unwrap_or(length));
        while index < stop {
            result.push(usize::try_from(index).map_err(|_| "invalid slice index".to_string())?);
            index = index.saturating_add(step);
        }
    } else {
        let normalize = |index: i64| {
            if index < 0 {
                length
                    .saturating_add(index)
                    .clamp(-1, length.saturating_sub(1))
            } else {
                index.clamp(-1, length.saturating_sub(1))
            }
        };
        let mut index = match optional_index(heap, start)? {
            Some(index) => normalize(index),
            None => length.saturating_sub(1),
        };
        let stop = match optional_index(heap, stop)? {
            Some(index) => normalize(index),
            None => -1,
        };
        while index > stop {
            result.push(usize::try_from(index).map_err(|_| "invalid slice index".to_string())?);
            index = index.saturating_add(step);
        }
    }
    Ok(result)
}

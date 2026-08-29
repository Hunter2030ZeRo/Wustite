use num_traits::ToPrimitive;

use crate::object::Object;
use crate::value::Value;

use super::ValueOps;

impl ValueOps<'_> {
    pub(super) fn sequence_add(&mut self, lhs: Value, rhs: Value) -> Result<Option<Value>, String> {
        let (Value::Object(lhs_ref), Value::Object(rhs_ref)) = (lhs, rhs) else {
            return Ok(None);
        };
        let result = match (self.heap.get(lhs_ref), self.heap.get(rhs_ref)) {
            (Ok(Object::String(lhs)), Ok(Object::String(rhs))) => {
                Some(Object::String(format!("{lhs}{rhs}")))
            }
            (Ok(Object::Tuple(lhs)), Ok(Object::Tuple(rhs))) => {
                Some(Object::tuple(lhs.iter().chain(rhs.iter()).collect()))
            }
            (Ok(Object::List(lhs)), Ok(Object::List(rhs))) => {
                Some(Object::list(lhs.iter().chain(rhs.iter()).collect()))
            }
            (Err(error), _) | (_, Err(error)) => return Err(error.to_string()),
            _ => None,
        };
        result.map(|object| self.allocate(object)).transpose()
    }

    pub(super) fn sequence_repeat(
        &mut self,
        lhs: Value,
        rhs: Value,
    ) -> Result<Option<Value>, String> {
        if let Some(count) = self.repeat_count(rhs)?
            && let Some(object) = self.repeated_object(lhs, count)?
        {
            return self.allocate(object).map(Some);
        }
        if let Some(count) = self.repeat_count(lhs)?
            && let Some(object) = self.repeated_object(rhs, count)?
        {
            return self.allocate(object).map(Some);
        }
        Ok(None)
    }

    fn repeat_count(&self, value: Value) -> Result<Option<usize>, String> {
        match value {
            Value::SmallInt(value) => Ok(Some(usize::try_from(value).unwrap_or(0))),
            Value::Object(reference) => match self.heap.get(reference) {
                Ok(Object::BigInt(value)) if value.sign() == num_bigint::Sign::Minus => Ok(Some(0)),
                Ok(Object::BigInt(value)) => value
                    .to_usize()
                    .map(Some)
                    .ok_or_else(|| "sequence repetition count is too large".to_string()),
                Ok(_) => Ok(None),
                Err(error) => Err(error.to_string()),
            },
            Value::Float(_) | Value::Bool(_) | Value::None | Value::Uninitialized => Ok(None),
        }
    }

    fn repeated_object(&self, value: Value, count: usize) -> Result<Option<Object>, String> {
        let Value::Object(reference) = value else {
            return Ok(None);
        };
        match self.heap.get(reference) {
            Ok(Object::String(value)) => Ok(Some(Object::String(value.repeat(count)))),
            Ok(Object::Tuple(values)) => Ok(Some(Object::Tuple(values.repeated(count)))),
            Ok(Object::List(values)) => Ok(Some(Object::List(values.repeated(count)))),
            Ok(_) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
}

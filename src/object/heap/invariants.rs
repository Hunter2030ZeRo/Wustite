use super::{Object, ObjectError, ObjectHeap, ObjectRef};
use crate::object::SequenceObject;
use crate::value::Value;
use num_bigint::BigInt;
use num_traits::FromPrimitive;

impl ObjectHeap {
    pub(super) fn validate_host_object(&self, object: &Object) -> Result<(), ObjectError> {
        match object {
            Object::String(_) | Object::BigInt(_) | Object::Function(_) | Object::Class(_) => {
                Ok(())
            }
            Object::Tuple(values) | Object::List(values) => self.validate_values(values),
            Object::Dict(entries) => self.validate_host_dictionary(entries),
            Object::Instance(instance) => {
                self.validate_value(Value::Object(instance.class()))?;
                for (_, value) in &instance.fields {
                    self.validate_value(*value)?;
                }
                Ok(())
            }
            Object::BoundMethod(method) => self.validate_value(Value::Object(method.receiver())),
        }
    }

    pub(super) fn validate_value(&self, value: Value) -> Result<(), ObjectError> {
        match value {
            Value::SmallInt(_) | Value::Float(_) | Value::Bool(_) | Value::None => Ok(()),
            Value::Object(reference) => self.get(reference).map(|_| ()),
            Value::Uninitialized => Err(ObjectError::UninitializedValue),
        }
    }

    fn validate_host_dictionary(&self, entries: &[(Value, Value)]) -> Result<(), ObjectError> {
        for (index, (key, value)) in entries.iter().enumerate() {
            self.validate_value(*key)?;
            self.validate_value(*value)?;
            self.validate_hashable_key(*key)?;
            for (candidate, _) in &entries[..index] {
                if self.host_keys_equal(*candidate, *key)? {
                    return Err(ObjectError::DuplicateDictionaryKey);
                }
            }
        }
        Ok(())
    }

    fn validate_values(&self, values: &SequenceObject) -> Result<(), ObjectError> {
        for value in values.iter() {
            self.validate_value(value)?;
        }
        Ok(())
    }

    fn validate_hashable_key(&self, key: Value) -> Result<(), ObjectError> {
        match key {
            Value::SmallInt(_) | Value::Float(_) | Value::Bool(_) | Value::None => Ok(()),
            Value::Uninitialized => Err(ObjectError::UninitializedValue),
            Value::Object(reference) => match self.get(reference)? {
                Object::String(_)
                | Object::BigInt(_)
                | Object::Function(_)
                | Object::Class(_)
                | Object::Instance(_)
                | Object::BoundMethod(_) => Ok(()),
                Object::Tuple(values) => {
                    for value in values.iter() {
                        self.validate_hashable_key(value)?;
                    }
                    Ok(())
                }
                Object::List(_) | Object::Dict(_) => Err(ObjectError::UnhashableDictionaryKey),
            },
        }
    }

    fn host_keys_equal(&self, lhs: Value, rhs: Value) -> Result<bool, ObjectError> {
        match (lhs, rhs) {
            (Value::SmallInt(lhs), Value::SmallInt(rhs)) => Ok(lhs == rhs),
            (Value::Float(lhs), Value::Float(rhs)) => Ok(lhs == rhs),
            (Value::SmallInt(lhs), Value::Float(rhs)) => {
                Ok(integer_float_equal(&BigInt::from(lhs), rhs))
            }
            (Value::Float(lhs), Value::SmallInt(rhs)) => {
                Ok(integer_float_equal(&BigInt::from(rhs), lhs))
            }
            (Value::Bool(lhs), Value::Bool(rhs)) => Ok(lhs == rhs),
            (Value::None, Value::None) => Ok(true),
            (Value::Object(lhs), Value::Object(rhs)) => self.host_object_keys_equal(lhs, rhs),
            (Value::SmallInt(lhs), Value::Object(rhs)) => {
                self.object_key_matches_integer(rhs, &BigInt::from(lhs))
            }
            (Value::Object(lhs), Value::SmallInt(rhs)) => {
                self.object_key_matches_integer(lhs, &BigInt::from(rhs))
            }
            (Value::Float(lhs), Value::Object(rhs)) => self.object_key_matches_float(rhs, lhs),
            (Value::Object(lhs), Value::Float(rhs)) => self.object_key_matches_float(lhs, rhs),
            (Value::Uninitialized, _) | (_, Value::Uninitialized) => {
                Err(ObjectError::UninitializedValue)
            }
            (Value::None, _) | (_, Value::None) => Ok(false),
            (Value::SmallInt(_), Value::Bool(_))
            | (Value::Bool(_), Value::SmallInt(_))
            | (Value::Float(_), Value::Bool(_))
            | (Value::Bool(_), Value::Float(_))
            | (Value::Bool(_), Value::Object(_))
            | (Value::Object(_), Value::Bool(_)) => Ok(false),
        }
    }

    fn host_object_keys_equal(&self, lhs: ObjectRef, rhs: ObjectRef) -> Result<bool, ObjectError> {
        self.validate_value(Value::Object(lhs))?;
        self.validate_value(Value::Object(rhs))?;
        if lhs == rhs {
            return Ok(true);
        }
        match (self.get(lhs)?, self.get(rhs)?) {
            (Object::String(lhs), Object::String(rhs)) => Ok(lhs == rhs),
            (Object::BigInt(lhs), Object::BigInt(rhs)) => Ok(lhs == rhs),
            (Object::Function(lhs), Object::Function(rhs)) => Ok(lhs.id() == rhs.id()),
            (Object::Tuple(lhs), Object::Tuple(rhs)) => self.host_tuple_keys_equal(lhs, rhs),
            (Object::String(_), _)
            | (Object::BigInt(_), _)
            | (Object::Function(_), _)
            | (Object::Tuple(_), _)
            | (Object::List(_), _)
            | (Object::Dict(_), _)
            | (Object::Class(_), _)
            | (Object::Instance(_), _)
            | (Object::BoundMethod(_), _) => Ok(false),
        }
    }

    fn object_key_matches_integer(
        &self,
        reference: ObjectRef,
        integer: &BigInt,
    ) -> Result<bool, ObjectError> {
        match self.get(reference)? {
            Object::BigInt(value) => Ok(value == integer),
            Object::String(_)
            | Object::Tuple(_)
            | Object::List(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => Ok(false),
        }
    }

    fn object_key_matches_float(
        &self,
        reference: ObjectRef,
        float: f64,
    ) -> Result<bool, ObjectError> {
        match self.get(reference)? {
            Object::BigInt(value) => Ok(integer_float_equal(value, float)),
            Object::String(_)
            | Object::Tuple(_)
            | Object::List(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => Ok(false),
        }
    }

    fn host_tuple_keys_equal(
        &self,
        lhs: &SequenceObject,
        rhs: &SequenceObject,
    ) -> Result<bool, ObjectError> {
        if lhs.len() != rhs.len() {
            return Ok(false);
        }
        for (lhs, rhs) in lhs.iter().zip(rhs.iter()) {
            if !self.host_keys_equal(lhs, rhs)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn integer_float_equal(integer: &BigInt, float: f64) -> bool {
    float.is_finite()
        && float.fract() == 0.0
        && BigInt::from_f64(float).is_some_and(|value| value == *integer)
}

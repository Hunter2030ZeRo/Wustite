use crate::object::ObjectRef;
use crate::value::Value;

use super::RuntimeError;

/// Stable public values accepted and returned by the embeddable runtime facade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuntimeValue {
    SmallInt(i64),
    Float(f64),
    Bool(bool),
    None,
    Object(ObjectRef),
}

impl From<RuntimeValue> for Value {
    fn from(value: RuntimeValue) -> Self {
        match value {
            RuntimeValue::SmallInt(value) => Self::SmallInt(value),
            RuntimeValue::Float(value) => Self::Float(value),
            RuntimeValue::Bool(value) => Self::Bool(value),
            RuntimeValue::None => Self::None,
            RuntimeValue::Object(value) => Self::Object(value),
        }
    }
}

impl TryFrom<Value> for RuntimeValue {
    type Error = RuntimeError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::SmallInt(value) => Ok(Self::SmallInt(value)),
            Value::Float(value) => Ok(Self::Float(value)),
            Value::Bool(value) => Ok(Self::Bool(value)),
            Value::None => Ok(Self::None),
            Value::Object(value) => Ok(Self::Object(value)),
            Value::Uninitialized => Err(RuntimeError::InvalidResult(
                "WVM returned an uninitialized value".to_owned(),
            )),
        }
    }
}

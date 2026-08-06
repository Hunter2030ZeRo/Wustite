pub use crate::object::Object;
use crate::object::ObjectRef;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    SmallInt(i64),
    Float(f64),
    Bool(bool),
    Object(ObjectRef),
    Uninitialized,
}

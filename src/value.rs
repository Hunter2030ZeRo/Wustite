pub use crate::object::Object;
use crate::object::ObjectRef;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    SmallInt(i64),
    Float(f64),
    Bool(bool),
    None,
    Object(ObjectRef),
    Uninitialized,
}

const BOOL_TAG: u8 = 0;
const I64_TAG: u8 = 1;
const F64_TAG: u8 = 2;
const OBJECT_TAG: u8 = 3;
const UNINITIALIZED_TAG: u8 = 4;
const NONE_TAG: u8 = 5;
pub const RUNTIME_SLOT_ABI_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct RuntimeSlot {
    tag: u8,
    padding: [u8; 7],
    payload: u64,
    extra: u64,
}

impl RuntimeSlot {
    pub(crate) fn from_value(value: Value) -> Self {
        let (tag, payload, extra) = match value {
            Value::Bool(value) => (BOOL_TAG, u64::from(value), 0),
            Value::SmallInt(value) => (I64_TAG, value as u64, 0),
            Value::Float(value) => (F64_TAG, value.to_bits(), 0),
            Value::None => (NONE_TAG, 0, 0),
            Value::Object(reference) => (
                OBJECT_TAG,
                reference.heap_id(),
                u64::from(reference.slot()) | (u64::from(reference.generation()) << 32),
            ),
            Value::Uninitialized => (UNINITIALIZED_TAG, 0, 0),
        };
        Self {
            tag,
            padding: [0; 7],
            payload,
            extra,
        }
    }

    pub(crate) fn value(self) -> Value {
        match self.tag {
            BOOL_TAG => Value::Bool(self.payload != 0),
            I64_TAG => Value::SmallInt(self.payload as i64),
            F64_TAG => Value::Float(f64::from_bits(self.payload)),
            OBJECT_TAG => Value::Object(ObjectRef::new(
                self.payload,
                self.extra as u32,
                (self.extra >> 32) as u32,
            )),
            UNINITIALIZED_TAG => Value::Uninitialized,
            NONE_TAG => Value::None,
            _ => unreachable!("invalid runtime slot tag"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::mem::{align_of, size_of};

    use super::{RUNTIME_SLOT_ABI_VERSION, RuntimeSlot, Value};

    #[test]
    fn runtime_slot_has_stable_c_layout_and_round_trips() {
        assert_eq!(RUNTIME_SLOT_ABI_VERSION, 1);
        assert_eq!(size_of::<RuntimeSlot>(), 24);
        assert_eq!(align_of::<RuntimeSlot>(), 8);
        assert_eq!(
            RuntimeSlot::from_value(Value::SmallInt(-7)).value(),
            Value::SmallInt(-7)
        );
    }
}

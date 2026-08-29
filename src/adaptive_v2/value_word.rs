use super::handles::StableHandle;
use super::heap::{GcError, GcHeap, GcObject};

const TAG_MASK: u64 = 0xffff_0000_0000_0000;
const INTEGER_TAG: u64 = 0x7ffc_0000_0000_0000;
const HANDLE_TAG: u64 = 0x7ffd_0000_0000_0000;
const LOCAL_PAYLOAD_MASK: u64 = 0x0000_ffff_ffff_ffff;
const INTEGER_PAYLOAD_MASK: u64 = 0x0000_7fff_ffff_ffff;
const INTEGER_SIGN_BIT: u64 = 1 << 46;
pub(crate) const IMMEDIATE_INTEGER_MIN: i64 = -(1_i64 << 46);
pub(crate) const IMMEDIATE_INTEGER_MAX: i64 = (1_i64 << 46) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarValue {
    Integer(i64),
    FloatBits(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub(crate) struct ValueWord(u64);

impl ValueWord {
    pub(crate) fn encode_scalar(value: ScalarValue, heap: &GcHeap) -> Result<Self, GcError> {
        match value {
            ScalarValue::Integer(integer)
                if (IMMEDIATE_INTEGER_MIN..=IMMEDIATE_INTEGER_MAX).contains(&integer) =>
            {
                Ok(Self(
                    INTEGER_TAG | ((integer as u64) & INTEGER_PAYLOAD_MASK),
                ))
            }
            ScalarValue::FloatBits(bits) if !f64::from_bits(bits).is_nan() => Ok(Self(bits)),
            ScalarValue::Integer(_) | ScalarValue::FloatBits(_) => {
                let handle = heap.allocate(GcObject::boxed_scalar(value))?;
                heap.pin_host(handle)?;
                Ok(Self::from_handle(handle))
            }
        }
    }

    pub(crate) fn decode_scalar(self, heap: &GcHeap) -> Result<ScalarValue, GcError> {
        match self.0 & TAG_MASK {
            INTEGER_TAG => {
                let payload = self.0 & INTEGER_PAYLOAD_MASK;
                let integer = if payload & INTEGER_SIGN_BIT == 0 {
                    payload as i64
                } else {
                    (payload | !INTEGER_PAYLOAD_MASK) as i64
                };
                Ok(ScalarValue::Integer(integer))
            }
            HANDLE_TAG => heap.scalar(heap.handle_from_local(self.0 & LOCAL_PAYLOAD_MASK)),
            _ => Ok(ScalarValue::FloatBits(self.0)),
        }
    }

    pub(crate) fn from_handle(handle: StableHandle) -> Self {
        Self(HANDLE_TAG | handle.packed_local())
    }

    pub(crate) fn as_handle(self, heap: &GcHeap) -> Option<StableHandle> {
        self.is_boxed()
            .then(|| heap.handle_from_local(self.0 & LOCAL_PAYLOAD_MASK))
    }

    pub(crate) const fn bits(self) -> u64 {
        self.0
    }

    pub(crate) const fn is_boxed(self) -> bool {
        self.0 & TAG_MASK == HANDLE_TAG
    }
}

const _: () = assert!(std::mem::size_of::<ValueWord>() == 8);

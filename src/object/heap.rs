use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Object, ObjectKind, ObjectRef};
use crate::value::Value;

mod invariants;

static NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectError {
    WrongHeap {
        expected: u64,
        actual: u64,
    },
    InvalidSlot {
        slot: u32,
    },
    StaleGeneration {
        slot: u32,
        expected: u32,
        actual: u32,
    },
    VacantSlot {
        slot: u32,
    },
    SlotCapacityExhausted,
    GenerationExhausted {
        slot: u32,
    },
    UninitializedValue,
    UnhashableDictionaryKey,
    DuplicateDictionaryKey,
}

impl fmt::Display for ObjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongHeap { expected, actual } => {
                write!(
                    formatter,
                    "object belongs to heap {actual}, not heap {expected}"
                )
            }
            Self::InvalidSlot { slot } => write!(formatter, "object slot {slot} does not exist"),
            Self::StaleGeneration {
                slot,
                expected,
                actual,
            } => write!(
                formatter,
                "object slot {slot} has generation {expected}, not {actual}"
            ),
            Self::VacantSlot { slot } => write!(formatter, "object slot {slot} is vacant"),
            Self::SlotCapacityExhausted => {
                formatter.write_str("object heap slot capacity exhausted")
            }
            Self::GenerationExhausted { slot } => {
                write!(formatter, "object slot {slot} generation exhausted")
            }
            Self::UninitializedValue => {
                formatter.write_str("container contains an uninitialized value")
            }
            Self::UnhashableDictionaryKey => formatter.write_str("dictionary key is not hashable"),
            Self::DuplicateDictionaryKey => {
                formatter.write_str("dictionary contains a duplicate key")
            }
        }
    }
}

impl Error for ObjectError {}

#[derive(Debug)]
struct Slot {
    generation: u32,
    object: Option<Object>,
}

#[derive(Debug)]
pub struct ObjectHeap {
    heap_id: u64,
    slots: Vec<Slot>,
    free_slots: Vec<u32>,
}

impl ObjectHeap {
    pub fn new() -> Self {
        Self {
            heap_id: NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed),
            slots: Vec::new(),
            free_slots: Vec::new(),
        }
    }

    pub fn allocate(&mut self, object: Object) -> Result<ObjectRef, ObjectError> {
        self.validate_host_object(&object)?;
        self.allocate_validated(object)
    }

    /// Allocates a dictionary whose key uniqueness has already been normalized by the WVM.
    ///
    /// Host-created dictionaries use `allocate`, which can only compare same-family key values
    /// without depending on WVM equality. The WVM calls this after its equality-aware builder
    /// has rejected unhashable keys and collapsed equivalent keys.
    pub(crate) fn allocate_runtime_dict(
        &mut self,
        entries: Vec<(Value, Value)>,
    ) -> Result<ObjectRef, ObjectError> {
        for (key, value) in &entries {
            self.validate_value(*key)?;
            self.validate_value(*value)?;
        }
        self.allocate_validated(Object::Dict(entries))
    }

    fn allocate_validated(&mut self, object: Object) -> Result<ObjectRef, ObjectError> {
        if let Some(slot_index) = self.free_slots.pop() {
            let heap_id = self.heap_id;
            let slot = self.slot_mut(slot_index)?;
            let generation = slot
                .generation
                .checked_add(1)
                .ok_or(ObjectError::GenerationExhausted { slot: slot_index })?;
            slot.generation = generation;
            slot.object = Some(object);
            return Ok(ObjectRef::new(heap_id, slot_index, generation));
        }

        let slot_index =
            u32::try_from(self.slots.len()).map_err(|_| ObjectError::SlotCapacityExhausted)?;
        self.slots.push(Slot {
            generation: 0,
            object: Some(object),
        });
        Ok(ObjectRef::new(self.heap_id, slot_index, 0))
    }

    pub fn get(&self, reference: ObjectRef) -> Result<&Object, ObjectError> {
        let slot = self.live_slot(reference)?;
        slot.object.as_ref().ok_or(ObjectError::VacantSlot {
            slot: reference.slot(),
        })
    }

    pub(crate) fn get_mut(&mut self, reference: ObjectRef) -> Result<&mut Object, ObjectError> {
        let slot = self.live_slot_mut(reference)?;
        slot.object.as_mut().ok_or(ObjectError::VacantSlot {
            slot: reference.slot(),
        })
    }

    pub fn remove(&mut self, reference: ObjectRef) -> Result<Object, ObjectError> {
        let slot = self.live_slot_mut(reference)?;
        let object = slot.object.take().ok_or(ObjectError::VacantSlot {
            slot: reference.slot(),
        })?;
        self.free_slots.push(reference.slot());
        Ok(object)
    }

    pub fn kind(&self, reference: ObjectRef) -> Result<ObjectKind, ObjectError> {
        self.get(reference).map(Object::kind)
    }

    fn live_slot(&self, reference: ObjectRef) -> Result<&Slot, ObjectError> {
        self.validate_heap(reference)?;
        let slot_index =
            usize::try_from(reference.slot()).map_err(|_| ObjectError::InvalidSlot {
                slot: reference.slot(),
            })?;
        let slot = self.slots.get(slot_index).ok_or(ObjectError::InvalidSlot {
            slot: reference.slot(),
        })?;
        Self::validate_generation(slot, reference)?;
        Ok(slot)
    }

    fn live_slot_mut(&mut self, reference: ObjectRef) -> Result<&mut Slot, ObjectError> {
        self.validate_heap(reference)?;
        let slot = self.slot_mut(reference.slot())?;
        Self::validate_generation(slot, reference)?;
        Ok(slot)
    }

    fn slot_mut(&mut self, slot_index: u32) -> Result<&mut Slot, ObjectError> {
        let index = usize::try_from(slot_index)
            .map_err(|_| ObjectError::InvalidSlot { slot: slot_index })?;
        self.slots
            .get_mut(index)
            .ok_or(ObjectError::InvalidSlot { slot: slot_index })
    }

    fn validate_heap(&self, reference: ObjectRef) -> Result<(), ObjectError> {
        if reference.heap_id() == self.heap_id {
            Ok(())
        } else {
            Err(ObjectError::WrongHeap {
                expected: self.heap_id,
                actual: reference.heap_id(),
            })
        }
    }

    fn validate_generation(slot: &Slot, reference: ObjectRef) -> Result<(), ObjectError> {
        if reference.generation() == slot.generation {
            Ok(())
        } else {
            Err(ObjectError::StaleGeneration {
                slot: reference.slot(),
                expected: slot.generation,
                actual: reference.generation(),
            })
        }
    }
}

impl Default for ObjectHeap {
    fn default() -> Self {
        Self::new()
    }
}

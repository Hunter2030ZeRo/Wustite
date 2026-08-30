use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use super::sequence::{FloatSequenceSnapshot, IntegerSequenceSnapshot, SequenceView};
use super::{BoundMethodObject, ClassId, InstanceObject, Object, ObjectKind, ObjectRef, ShapeId};
use crate::executable::ExecutableFunction;
use crate::value::Value;

mod invariants;

#[cfg(test)]
mod transfer_tests;

#[cfg(test)]
mod snapshot_pair_tests {
    use super::ObjectHeap;
    use crate::object::{Object, SequenceObject};
    use crate::value::Value;

    fn float_list(heap: &mut ObjectHeap, values: &[f64]) -> crate::object::ObjectRef {
        heap.allocate(Object::List(SequenceObject::from_values(
            values.iter().copied().map(Value::Float).collect(),
        )))
        .expect("float list allocation")
    }

    fn integer_list(heap: &mut ObjectHeap, values: &[i64]) -> crate::object::ObjectRef {
        heap.allocate(Object::List(SequenceObject::from_values(
            values.iter().copied().map(Value::SmallInt).collect(),
        )))
        .expect("integer list allocation")
    }

    fn values(heap: &ObjectHeap, reference: crate::object::ObjectRef) -> Vec<Value> {
        let Object::List(sequence) = heap.get(reference).expect("live list") else {
            panic!("expected list")
        };
        sequence.to_vec()
    }

    #[test]
    fn float_pair_commits_atomically() {
        // Given two distinct live F64 lists with current layout versions.
        let mut heap = ObjectHeap::new();
        let first = float_list(&mut heap, &[1.0]);
        let second = float_list(&mut heap, &[2.0]);
        let first_version = heap.float_sequence_snapshot(first).unwrap().unwrap().1;
        let second_version = heap.float_sequence_snapshot(second).unwrap().unwrap().1;

        // When both owned snapshots are committed as one pair.
        let committed = heap
            .commit_float_sequence_snapshot_pair(
                (first, first_version, vec![3.0]),
                (second, second_version, vec![4.0]),
            )
            .unwrap();

        // Then both authoritative lists change exactly.
        assert!(committed);
        assert_eq!(values(&heap, first), vec![Value::Float(3.0)]);
        assert_eq!(values(&heap, second), vec![Value::Float(4.0)]);
    }

    #[test]
    fn float_snapshot_pair_rejects_stale_first_atomically() {
        // Given a stale first snapshot and a current second snapshot.
        let mut heap = ObjectHeap::new();
        let first = float_list(&mut heap, &[1.0]);
        let second = float_list(&mut heap, &[2.0]);
        let stale_first = heap.float_sequence_snapshot(first).unwrap().unwrap().1;
        let second_version = heap.float_sequence_snapshot(second).unwrap().unwrap().1;
        assert!(
            heap.commit_float_sequence_snapshot(first, stale_first, vec![5.0])
                .unwrap()
        );

        // When the pair commit validates both snapshots.
        let committed = heap
            .commit_float_sequence_snapshot_pair(
                (first, stale_first, vec![6.0]),
                (second, second_version, vec![7.0]),
            )
            .unwrap();

        // Then neither list is partially changed by the rejected pair.
        assert!(!committed);
        assert_eq!(values(&heap, first), vec![Value::Float(5.0)]);
        assert_eq!(values(&heap, second), vec![Value::Float(2.0)]);
    }

    #[test]
    fn float_snapshot_pair_rejects_stale_second_atomically() {
        // Given a current first snapshot and a stale second snapshot.
        let mut heap = ObjectHeap::new();
        let first = float_list(&mut heap, &[1.0]);
        let second = float_list(&mut heap, &[2.0]);
        let first_version = heap.float_sequence_snapshot(first).unwrap().unwrap().1;
        let stale_second = heap.float_sequence_snapshot(second).unwrap().unwrap().1;
        assert!(
            heap.commit_float_sequence_snapshot(second, stale_second, vec![5.0])
                .unwrap()
        );

        // When the pair commit validates both snapshots.
        let committed = heap
            .commit_float_sequence_snapshot_pair(
                (first, first_version, vec![6.0]),
                (second, stale_second, vec![7.0]),
            )
            .unwrap();

        // Then neither list is partially changed by the rejected pair.
        assert!(!committed);
        assert_eq!(values(&heap, first), vec![Value::Float(1.0)]);
        assert_eq!(values(&heap, second), vec![Value::Float(5.0)]);
    }

    #[test]
    fn float_snapshot_pair_rejects_same_handle() {
        // Given one live F64 list supplied in both pair positions.
        let mut heap = ObjectHeap::new();
        let list = float_list(&mut heap, &[1.0]);
        let version = heap.float_sequence_snapshot(list).unwrap().unwrap().1;

        // When the pair commit is attempted with aliased ownership.
        let committed = heap
            .commit_float_sequence_snapshot_pair(
                (list, version, vec![2.0]),
                (list, version, vec![3.0]),
            )
            .unwrap();

        // Then it is rejected and the authoritative list is unchanged.
        assert!(!committed);
        assert_eq!(values(&heap, list), vec![Value::Float(1.0)]);
    }

    #[test]
    fn int_snapshot_pair_commit_changes_both_lists_atomically() {
        let mut heap = ObjectHeap::new();
        let first = integer_list(&mut heap, &[1]);
        let second = integer_list(&mut heap, &[2]);
        let first_version = heap.integer_sequence_snapshot(first).unwrap().unwrap().1;
        let second_version = heap.integer_sequence_snapshot(second).unwrap().unwrap().1;

        let committed = heap
            .commit_integer_sequence_snapshot_pair(
                (first, first_version, vec![3]),
                (second, second_version, vec![4]),
            )
            .unwrap();

        assert!(committed);
        assert_eq!(values(&heap, first), vec![Value::SmallInt(3)]);
        assert_eq!(values(&heap, second), vec![Value::SmallInt(4)]);
    }

    #[test]
    fn int_snapshot_pair_rejects_stale_first_atomically() {
        let mut heap = ObjectHeap::new();
        let first = integer_list(&mut heap, &[1]);
        let second = integer_list(&mut heap, &[2]);
        let stale_first = heap.integer_sequence_snapshot(first).unwrap().unwrap().1;
        let second_version = heap.integer_sequence_snapshot(second).unwrap().unwrap().1;
        assert!(
            heap.commit_integer_sequence_snapshot(first, stale_first, vec![5])
                .unwrap()
        );

        let committed = heap
            .commit_integer_sequence_snapshot_pair(
                (first, stale_first, vec![6]),
                (second, second_version, vec![7]),
            )
            .unwrap();

        assert!(!committed);
        assert_eq!(values(&heap, first), vec![Value::SmallInt(5)]);
        assert_eq!(values(&heap, second), vec![Value::SmallInt(2)]);
    }

    #[test]
    fn int_snapshot_pair_rejects_stale_second_atomically() {
        let mut heap = ObjectHeap::new();
        let first = integer_list(&mut heap, &[1]);
        let second = integer_list(&mut heap, &[2]);
        let first_version = heap.integer_sequence_snapshot(first).unwrap().unwrap().1;
        let stale_second = heap.integer_sequence_snapshot(second).unwrap().unwrap().1;
        assert!(
            heap.commit_integer_sequence_snapshot(second, stale_second, vec![5])
                .unwrap()
        );

        let committed = heap
            .commit_integer_sequence_snapshot_pair(
                (first, first_version, vec![6]),
                (second, stale_second, vec![7]),
            )
            .unwrap();

        assert!(!committed);
        assert_eq!(values(&heap, first), vec![Value::SmallInt(1)]);
        assert_eq!(values(&heap, second), vec![Value::SmallInt(5)]);
    }

    #[test]
    fn int_snapshot_pair_rejects_same_handle() {
        let mut heap = ObjectHeap::new();
        let list = integer_list(&mut heap, &[1]);
        let version = heap.integer_sequence_snapshot(list).unwrap().unwrap().1;

        let committed = heap
            .commit_integer_sequence_snapshot_pair(
                (list, version, vec![2]),
                (list, version, vec![3]),
            )
            .unwrap();

        assert!(!committed);
        assert_eq!(values(&heap, list), vec![Value::SmallInt(1)]);
    }

    #[test]
    fn float_snapshot_set_rejects_one_stale_member_atomically() {
        // Given: three lists where only the middle transaction has a stale layout version.
        let mut heap = ObjectHeap::new();
        let first = float_list(&mut heap, &[1.0]);
        let second = float_list(&mut heap, &[2.0]);
        let third = float_list(&mut heap, &[3.0]);
        let first_version = heap.float_sequence_snapshot(first).unwrap().unwrap().1;
        let stale_second = heap.float_sequence_snapshot(second).unwrap().unwrap().1;
        let third_version = heap.float_sequence_snapshot(third).unwrap().unwrap().1;
        assert!(
            heap.commit_float_sequence_snapshot(second, stale_second, vec![20.0])
                .unwrap()
        );

        // When: the complete owned set is committed under one exclusive heap transaction.
        let committed = heap
            .commit_float_sequence_snapshots(vec![
                (first, first_version, vec![10.0]),
                (second, stale_second, vec![21.0]),
                (third, third_version, vec![30.0]),
            ])
            .unwrap();

        // Then: validation rejects the set before changing either current member.
        assert!(!committed);
        assert_eq!(values(&heap, first), vec![Value::Float(1.0)]);
        assert_eq!(values(&heap, second), vec![Value::Float(20.0)]);
        assert_eq!(values(&heap, third), vec![Value::Float(3.0)]);
    }

    #[test]
    fn float_set_rejects_duplicates_atomically() {
        // Given: a current list repeated in one owned transaction set.
        let mut heap = ObjectHeap::new();
        let list = float_list(&mut heap, &[1.0]);
        let version = heap.float_sequence_snapshot(list).unwrap().unwrap().1;

        // When: both set entries claim the same authoritative object identity.
        let committed = heap
            .commit_float_sequence_snapshots(vec![
                (list, version, vec![2.0]),
                (list, version, vec![3.0]),
            ])
            .unwrap();

        // Then: alias validation rejects before the list is changed.
        assert!(!committed);
        assert_eq!(values(&heap, list), vec![Value::Float(1.0)]);
    }
}

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
    TransferTargetOccupied {
        slot: u32,
    },
    SlotCapacityExhausted,
    GenerationExhausted {
        slot: u32,
    },
    UninitializedValue,
    UnhashableDictionaryKey,
    DuplicateDictionaryKey,
    NotClass,
    NotInstance,
    MissingAttribute,
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
            Self::TransferTargetOccupied { slot } => {
                write!(formatter, "object transfer target slot {slot} is occupied")
            }
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
            Self::NotClass => formatter.write_str("object is not a class"),
            Self::NotInstance => formatter.write_str("object is not an instance"),
            Self::MissingAttribute => formatter.write_str("object attribute was not found"),
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
    shapes: HashMap<(ClassId, Vec<String>), ShapeId>,
    next_shape: u64,
}

impl ObjectHeap {
    pub fn new() -> Self {
        Self {
            heap_id: NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed),
            slots: Vec::new(),
            free_slots: Vec::new(),
            shapes: HashMap::new(),
            next_shape: 1,
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

    pub(crate) fn transfer_out(&mut self, reference: ObjectRef) -> Result<Object, ObjectError> {
        let slot = self.live_slot_mut(reference)?;
        slot.object.take().ok_or(ObjectError::VacantSlot {
            slot: reference.slot(),
        })
    }

    pub(crate) fn transfer_in(
        &mut self,
        reference: ObjectRef,
        object: Object,
    ) -> Result<(), ObjectError> {
        self.validate_heap(reference)?;
        let slot = self.slot_mut(reference.slot())?;
        Self::validate_generation(slot, reference)?;
        if slot.object.is_some() {
            return Err(ObjectError::TransferTargetOccupied {
                slot: reference.slot(),
            });
        }
        slot.object = Some(object);
        Ok(())
    }

    pub fn kind(&self, reference: ObjectRef) -> Result<ObjectKind, ObjectError> {
        self.get(reference).map(Object::kind)
    }

    pub(crate) fn function_reference(&self, function: &ExecutableFunction) -> Option<ObjectRef> {
        self.slots
            .iter()
            .enumerate()
            .find_map(|(slot, entry)| match entry.object.as_ref() {
                Some(Object::Function(candidate)) if candidate.id() == function.id() => {
                    u32::try_from(slot)
                        .ok()
                        .map(|slot| ObjectRef::new(self.heap_id, slot, entry.generation))
                }
                _ => None,
            })
    }

    pub(crate) fn instantiate(&mut self, class: ObjectRef) -> Result<ObjectRef, ObjectError> {
        let class_id = match self.get(class)? {
            Object::Class(class) => class.id(),
            _ => return Err(ObjectError::NotClass),
        };
        let shape = self.intern_shape(class_id, Vec::new());
        self.allocate_validated(Object::Instance(InstanceObject {
            class,
            class_id,
            shape,
            fields: Vec::new(),
        }))
    }

    pub(crate) fn get_attribute(
        &mut self,
        receiver: ObjectRef,
        name: &str,
    ) -> Result<Value, ObjectError> {
        let (class, field) = match self.get(receiver)? {
            Object::Instance(instance) => (
                instance.class,
                instance
                    .fields
                    .iter()
                    .find(|(candidate, _)| candidate == name)
                    .map(|(_, value)| *value),
            ),
            _ => return Err(ObjectError::NotInstance),
        };
        if let Some(value) = field {
            return Ok(value);
        }
        let function = match self.get(class)? {
            Object::Class(class) => class.method(name).cloned(),
            _ => return Err(ObjectError::NotClass),
        }
        .ok_or(ObjectError::MissingAttribute)?;
        self.allocate_validated(Object::BoundMethod(BoundMethodObject {
            receiver,
            function,
        }))
        .map(Value::Object)
    }

    pub(crate) fn instance_shape(&self, receiver: ObjectRef) -> Result<ShapeId, ObjectError> {
        match self.get(receiver)? {
            Object::Instance(instance) => Ok(instance.shape),
            _ => Err(ObjectError::NotInstance),
        }
    }

    pub(crate) fn lookup_instance_field(
        &self,
        receiver: ObjectRef,
        name: &str,
    ) -> Result<Option<(ShapeId, usize, Value)>, ObjectError> {
        let instance = match self.get(receiver)? {
            Object::Instance(instance) => instance,
            _ => return Err(ObjectError::NotInstance),
        };
        Ok(instance
            .fields
            .iter()
            .enumerate()
            .find(|(_, (candidate, _))| candidate == name)
            .map(|(index, (_, value))| (instance.shape, index, *value)))
    }

    pub(crate) fn instance_field_at(
        &self,
        receiver: ObjectRef,
        expected_shape: ShapeId,
        index: usize,
    ) -> Result<Option<Value>, ObjectError> {
        let instance = match self.get(receiver)? {
            Object::Instance(instance) => instance,
            _ => return Err(ObjectError::NotInstance),
        };
        if instance.shape != expected_shape {
            return Ok(None);
        }
        Ok(instance.fields.get(index).map(|(_, value)| *value))
    }

    pub(crate) fn set_attribute(
        &mut self,
        receiver: ObjectRef,
        name: String,
        value: Value,
    ) -> Result<(), ObjectError> {
        self.validate_value(value)?;
        let (class_id, names, existing) = match self.get(receiver)? {
            Object::Instance(instance) => (
                instance.class_id,
                instance
                    .fields
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>(),
                instance
                    .fields
                    .iter()
                    .position(|(candidate, _)| candidate == &name),
            ),
            _ => return Err(ObjectError::NotInstance),
        };
        if let Some(index) = existing {
            let Object::Instance(instance) = self.get_mut(receiver)? else {
                return Err(ObjectError::NotInstance);
            };
            instance.fields[index].1 = value;
            return Ok(());
        }
        let mut next_names = names;
        next_names.push(name.clone());
        let shape = self.intern_shape(class_id, next_names);
        let Object::Instance(instance) = self.get_mut(receiver)? else {
            return Err(ObjectError::NotInstance);
        };
        instance.shape = shape;
        instance.fields.push((name, value));
        Ok(())
    }

    pub(crate) fn lookup_method(
        &self,
        receiver: ObjectRef,
        name: &str,
    ) -> Result<(ShapeId, ExecutableFunction), ObjectError> {
        let instance = match self.get(receiver)? {
            Object::Instance(instance) => instance,
            _ => return Err(ObjectError::NotInstance),
        };
        let function = match self.get(instance.class)? {
            Object::Class(class) => class.method(name),
            _ => return Err(ObjectError::NotClass),
        }
        .ok_or(ObjectError::MissingAttribute)?;
        Ok((instance.shape, function.clone()))
    }

    fn intern_shape(&mut self, class: ClassId, fields: Vec<String>) -> ShapeId {
        if let Some(shape) = self.shapes.get(&(class, fields.clone())) {
            return *shape;
        }
        let shape = ShapeId(self.next_shape);
        self.next_shape = self.next_shape.saturating_add(1);
        self.shapes.insert((class, fields), shape);
        shape
    }

    pub(crate) fn sequence_view(
        &mut self,
        reference: ObjectRef,
    ) -> Result<Option<SequenceView>, ObjectError> {
        match self.get_mut(reference)? {
            Object::List(sequence) => Ok(sequence.borrowed_view(true)),
            Object::Tuple(sequence) => Ok(sequence.borrowed_view(false)),
            _ => Ok(None),
        }
    }

    pub(crate) fn integer_sequence_snapshot(
        &self,
        reference: ObjectRef,
    ) -> Result<Option<IntegerSequenceSnapshot>, ObjectError> {
        match self.get(reference)? {
            Object::List(sequence) => Ok(sequence.integer_snapshot()),
            Object::Tuple(_)
            | Object::String(_)
            | Object::BigInt(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => Ok(None),
        }
    }

    pub(crate) fn commit_integer_sequence_snapshot(
        &mut self,
        reference: ObjectRef,
        expected_layout_version: u64,
        values: Vec<i64>,
    ) -> Result<bool, ObjectError> {
        match self.get_mut(reference)? {
            Object::List(sequence) => {
                Ok(sequence.commit_integer_snapshot(expected_layout_version, values))
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn commit_integer_sequence_snapshot_pair(
        &mut self,
        first: (ObjectRef, u64, Vec<i64>),
        second: (ObjectRef, u64, Vec<i64>),
    ) -> Result<bool, ObjectError> {
        self.commit_integer_sequence_snapshots(vec![first, second])
    }

    pub(crate) fn commit_integer_sequence_snapshots(
        &mut self,
        snapshots: Vec<(ObjectRef, u64, Vec<i64>)>,
    ) -> Result<bool, ObjectError> {
        let mut references = std::collections::HashSet::new();
        for (reference, version, _) in &snapshots {
            if !references.insert(*reference)
                || !matches!(
                    self.get(*reference)?,
                    Object::List(sequence) if sequence.can_commit_integer_snapshot(*version)
                )
            {
                return Ok(false);
            }
        }
        for (reference, _, values) in snapshots {
            let Object::List(sequence) = self.get_mut(reference)? else {
                unreachable!("validated list changed under exclusive heap access")
            };
            sequence.apply_integer_snapshot(values);
        }
        Ok(true)
    }

    pub(crate) fn float_sequence_snapshot(
        &self,
        reference: ObjectRef,
    ) -> Result<Option<FloatSequenceSnapshot>, ObjectError> {
        match self.get(reference)? {
            Object::List(sequence) => Ok(sequence.float_snapshot()),
            Object::Tuple(_)
            | Object::String(_)
            | Object::BigInt(_)
            | Object::Dict(_)
            | Object::Function(_)
            | Object::Class(_)
            | Object::Instance(_)
            | Object::BoundMethod(_) => Ok(None),
        }
    }

    pub(crate) fn commit_float_sequence_snapshot(
        &mut self,
        reference: ObjectRef,
        expected_layout_version: u64,
        values: Vec<f64>,
    ) -> Result<bool, ObjectError> {
        match self.get_mut(reference)? {
            Object::List(sequence) => {
                Ok(sequence.commit_float_snapshot(expected_layout_version, values))
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn commit_widened_float_sequence_snapshot(
        &mut self,
        reference: ObjectRef,
        expected_layout_version: u64,
        values: Vec<f64>,
    ) -> Result<bool, ObjectError> {
        match self.get_mut(reference)? {
            Object::List(sequence) => {
                Ok(sequence.commit_widened_float_snapshot(expected_layout_version, values))
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn commit_float_sequence_snapshot_pair(
        &mut self,
        first: (ObjectRef, u64, Vec<f64>),
        second: (ObjectRef, u64, Vec<f64>),
    ) -> Result<bool, ObjectError> {
        self.commit_float_sequence_snapshots(vec![first, second])
    }

    pub(crate) fn commit_float_sequence_snapshots(
        &mut self,
        snapshots: Vec<(ObjectRef, u64, Vec<f64>)>,
    ) -> Result<bool, ObjectError> {
        let mut references = std::collections::HashSet::new();
        for (reference, version, _) in &snapshots {
            if !references.insert(*reference)
                || !matches!(
                    self.get(*reference)?,
                    Object::List(sequence) if sequence.can_commit_float_snapshot(*version)
                )
            {
                return Ok(false);
            }
        }
        for (reference, _, values) in snapshots {
            let Object::List(sequence) = self.get_mut(reference)? else {
                unreachable!("validated list changed under exclusive heap access")
            };
            sequence.apply_float_snapshot(values);
        }
        Ok(true)
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

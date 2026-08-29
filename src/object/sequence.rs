use std::fmt;
use std::sync::Arc;

pub(crate) type IntegerSequenceSnapshot = (Arc<[i64]>, u64);
pub(crate) type FloatSequenceSnapshot = (Arc<[f64]>, u64);

use crate::value::{RuntimeSlot, Value};

use self::storage::{SequenceStorage, storage_from_values};

mod storage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceStrategy {
    Empty,
    Bool,
    I64,
    F64,
    Object,
}

#[derive(Clone)]
pub struct SequenceObject {
    storage: SequenceStorage,
    layout_version: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SequenceView {
    pub data: *mut u8,
    pub len: usize,
    pub layout_version: u64,
    pub strategy: SequenceStrategy,
    pub writable: bool,
}

impl SequenceObject {
    pub fn from_values(values: Vec<Value>) -> Self {
        Self {
            storage: storage_from_values(values),
            layout_version: 0,
        }
    }

    pub const fn strategy(&self) -> SequenceStrategy {
        match self.storage {
            SequenceStorage::Empty => SequenceStrategy::Empty,
            SequenceStorage::Bool(_) => SequenceStrategy::Bool,
            SequenceStorage::I64(_) => SequenceStrategy::I64,
            SequenceStorage::F64(_) => SequenceStrategy::F64,
            SequenceStorage::Object(_) => SequenceStrategy::Object,
        }
    }

    pub const fn layout_version(&self) -> u64 {
        self.layout_version
    }

    pub const fn direct_view_allowed(&self) -> bool {
        self.layout_version != u64::MAX
    }

    pub(crate) fn borrowed_view(&mut self, writable: bool) -> Option<SequenceView> {
        if !self.direct_view_allowed() {
            return None;
        }
        let (data, strategy) = match &mut self.storage {
            SequenceStorage::Empty => (
                std::ptr::NonNull::<u8>::dangling().as_ptr(),
                SequenceStrategy::Empty,
            ),
            SequenceStorage::Bool(values) => (values.as_mut_ptr(), SequenceStrategy::Bool),
            SequenceStorage::I64(values) => (values.as_mut_ptr().cast(), SequenceStrategy::I64),
            SequenceStorage::F64(values) => (values.as_mut_ptr().cast(), SequenceStrategy::F64),
            SequenceStorage::Object(values) => {
                (values.as_mut_ptr().cast(), SequenceStrategy::Object)
            }
        };
        Some(SequenceView {
            data,
            len: self.len(),
            layout_version: self.layout_version,
            strategy,
            writable,
        })
    }

    pub fn len(&self) -> usize {
        match &self.storage {
            SequenceStorage::Empty => 0,
            SequenceStorage::Bool(values) => values.len(),
            SequenceStorage::I64(values) => values.len(),
            SequenceStorage::F64(values) => values.len(),
            SequenceStorage::Object(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<Value> {
        match &self.storage {
            SequenceStorage::Empty => None,
            SequenceStorage::Bool(values) => {
                values.get(index).map(|value| Value::Bool(*value != 0))
            }
            SequenceStorage::I64(values) => values.get(index).copied().map(Value::SmallInt),
            SequenceStorage::F64(values) => values.get(index).copied().map(Value::Float),
            SequenceStorage::Object(values) => values.get(index).copied().map(RuntimeSlot::value),
        }
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = Value> + '_ {
        (0..self.len()).map(|index| self.get(index).expect("sequence index from length"))
    }

    pub(crate) fn integer_snapshot(&self) -> Option<IntegerSequenceSnapshot> {
        match &self.storage {
            SequenceStorage::I64(values) => {
                Some((Arc::from(values.as_slice()), self.layout_version))
            }
            SequenceStorage::Empty => Some((Arc::from([]), self.layout_version)),
            SequenceStorage::Bool(_) | SequenceStorage::F64(_) | SequenceStorage::Object(_) => None,
        }
    }

    pub(crate) fn float_snapshot(&self) -> Option<FloatSequenceSnapshot> {
        match &self.storage {
            SequenceStorage::F64(values) => {
                Some((Arc::from(values.as_slice()), self.layout_version))
            }
            SequenceStorage::Empty => Some((Arc::from([]), self.layout_version)),
            SequenceStorage::Bool(_) | SequenceStorage::I64(_) | SequenceStorage::Object(_) => None,
        }
    }

    pub(crate) fn commit_integer_snapshot(
        &mut self,
        expected_layout_version: u64,
        values: Vec<i64>,
    ) -> bool {
        if !self.can_commit_integer_snapshot(expected_layout_version) {
            return false;
        }
        self.apply_integer_snapshot(values);
        true
    }

    pub(crate) fn can_commit_integer_snapshot(&self, expected_layout_version: u64) -> bool {
        self.layout_version == expected_layout_version
            && matches!(
                self.storage,
                SequenceStorage::Empty | SequenceStorage::I64(_)
            )
    }

    pub(crate) fn apply_integer_snapshot(&mut self, values: Vec<i64>) {
        self.storage = if values.is_empty() {
            SequenceStorage::Empty
        } else {
            SequenceStorage::I64(values)
        };
        self.bump_layout();
    }

    pub(crate) fn commit_float_snapshot(
        &mut self,
        expected_layout_version: u64,
        values: Vec<f64>,
    ) -> bool {
        if !self.can_commit_float_snapshot(expected_layout_version) {
            return false;
        }
        self.apply_float_snapshot(values);
        true
    }

    pub(crate) fn commit_widened_float_snapshot(
        &mut self,
        expected_layout_version: u64,
        values: Vec<f64>,
    ) -> bool {
        if self.layout_version != expected_layout_version
            || !matches!(self.storage, SequenceStorage::I64(_))
        {
            return false;
        }
        self.apply_float_snapshot(values);
        true
    }

    pub(crate) fn can_commit_float_snapshot(&self, expected_layout_version: u64) -> bool {
        self.layout_version == expected_layout_version
            && matches!(
                self.storage,
                SequenceStorage::Empty | SequenceStorage::F64(_)
            )
    }

    pub(crate) fn apply_float_snapshot(&mut self, values: Vec<f64>) {
        self.storage = if values.is_empty() {
            SequenceStorage::Empty
        } else {
            SequenceStorage::F64(values)
        };
        self.bump_layout();
    }

    pub fn to_vec(&self) -> Vec<Value> {
        self.iter().collect()
    }

    pub fn set(&mut self, index: usize, value: Value) -> Option<Value> {
        let previous = self.get(index)?;
        match (&mut self.storage, value) {
            (SequenceStorage::Bool(values), Value::Bool(value)) => values[index] = u8::from(value),
            (SequenceStorage::I64(values), Value::SmallInt(value)) => values[index] = value,
            (SequenceStorage::F64(values), Value::Float(value)) => values[index] = value,
            (SequenceStorage::Object(values), value) => {
                values[index] = RuntimeSlot::from_value(value)
            }
            (_, value) => {
                let mut values = self.to_vec();
                values[index] = value;
                self.storage = SequenceStorage::Object(
                    values.into_iter().map(RuntimeSlot::from_value).collect(),
                );
                self.bump_layout();
            }
        }
        Some(previous)
    }

    pub fn push(&mut self, value: Value) {
        self.insert(self.len(), value);
    }

    pub fn insert(&mut self, index: usize, value: Value) {
        assert!(index <= self.len(), "sequence insertion index out of range");
        match (&mut self.storage, value) {
            (SequenceStorage::Empty, value) => self.storage = storage_from_values(vec![value]),
            (SequenceStorage::Bool(values), Value::Bool(value)) => {
                values.insert(index, u8::from(value))
            }
            (SequenceStorage::I64(values), Value::SmallInt(value)) => values.insert(index, value),
            (SequenceStorage::F64(values), Value::Float(value)) => values.insert(index, value),
            (SequenceStorage::Object(values), value) => {
                values.insert(index, RuntimeSlot::from_value(value))
            }
            (_, value) => {
                let mut values = self.to_vec();
                values.insert(index, value);
                self.storage = SequenceStorage::Object(
                    values.into_iter().map(RuntimeSlot::from_value).collect(),
                );
            }
        }
        self.bump_layout();
    }

    pub fn remove(&mut self, index: usize) -> Option<Value> {
        let value = match &mut self.storage {
            SequenceStorage::Empty => return None,
            SequenceStorage::Bool(values) => Value::Bool(values.get(index).copied()? != 0),
            SequenceStorage::I64(values) => Value::SmallInt(*values.get(index)?),
            SequenceStorage::F64(values) => Value::Float(*values.get(index)?),
            SequenceStorage::Object(values) => values.get(index).copied()?.value(),
        };
        match &mut self.storage {
            SequenceStorage::Empty => unreachable!(),
            SequenceStorage::Bool(values) => {
                values.remove(index);
            }
            SequenceStorage::I64(values) => {
                values.remove(index);
            }
            SequenceStorage::F64(values) => {
                values.remove(index);
            }
            SequenceStorage::Object(values) => {
                values.remove(index);
            }
        }
        self.bump_layout();
        Some(value)
    }

    pub fn replace_range(&mut self, range: std::ops::Range<usize>, replacement: Vec<Value>) {
        let mut values = self.to_vec();
        values.splice(range, replacement);
        self.storage = storage_from_values(values);
        self.bump_layout();
    }

    pub fn reverse_prefix(&mut self, end: usize) -> bool {
        if end > self.len() {
            return false;
        }
        let mut values = self.to_vec();
        values[..end].reverse();
        self.storage = storage_from_values(values);
        self.bump_layout();
        true
    }

    pub fn repeated(&self, count: usize) -> Self {
        Self::from_values(self.to_vec().repeat(count))
    }

    fn bump_layout(&mut self) {
        self.layout_version = self.layout_version.saturating_add(1);
    }
}

impl PartialEq for SequenceObject {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(lhs, rhs)| lhs == rhs)
    }
}

impl fmt::Debug for SequenceObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::SequenceObject;
    use crate::value::Value;

    #[test]
    fn integer_snapshot_commit_rejects_stale_layout_without_mutation() {
        // Given: an immutable snapshot whose authoritative list changes before commit.
        let mut sequence = SequenceObject::from_values(vec![Value::SmallInt(1)]);
        let (_, version) = sequence.integer_snapshot().expect("integer snapshot");
        sequence.push(Value::SmallInt(2));

        // When: native results attempt to commit against the stale version.
        let committed = sequence.commit_integer_snapshot(version, vec![9]);

        // Then: the authoritative post-snapshot mutation remains intact.
        assert!(!committed);
        assert_eq!(
            sequence.to_vec(),
            vec![Value::SmallInt(1), Value::SmallInt(2)]
        );
    }

    #[test]
    fn integer_snapshot_commit_rejects_layout_strategy_change() {
        // Given: a snapshot whose list changes from integer to generic object storage.
        let mut sequence = SequenceObject::from_values(vec![Value::SmallInt(1)]);
        let (_, version) = sequence.integer_snapshot().expect("integer snapshot");
        sequence.push(Value::None);

        // When: native integer storage attempts to commit after the representation change.
        let committed = sequence.commit_integer_snapshot(version, vec![9]);

        // Then: the generic authoritative values are not overwritten.
        assert!(!committed);
        assert_eq!(sequence.to_vec(), vec![Value::SmallInt(1), Value::None]);
    }

    #[test]
    fn float_snapshot_commit_is_bit_exact_and_rejects_stale_layout() {
        // Given: an owned float snapshot containing signed zero and a NaN payload.
        let nan = f64::from_bits(0x7ff8_0000_0000_0042);
        let mut sequence = SequenceObject::from_values(vec![Value::Float(-0.0), Value::Float(nan)]);
        let (snapshot, version) = sequence.float_snapshot().expect("float snapshot");

        // When: the exact snapshot is committed once, then replayed against its stale version.
        assert!(sequence.commit_float_snapshot(version, snapshot.to_vec()));
        let stale = sequence.commit_float_snapshot(version, vec![1.0]);

        // Then: the stale write is rejected and all original float bits remain authoritative.
        assert!(!stale);
        let bits = sequence
            .to_vec()
            .into_iter()
            .map(|value| match value {
                Value::Float(value) => value.to_bits(),
                other => panic!("unexpected value {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(bits, vec![(-0.0_f64).to_bits(), nan.to_bits()]);
    }

    #[test]
    fn float_snapshot_commit_rejects_storage_strategy_change() {
        // Given: a float snapshot whose authoritative list widens to object storage.
        let mut sequence = SequenceObject::from_values(vec![Value::Float(1.0)]);
        let (_, version) = sequence.float_snapshot().expect("float snapshot");
        sequence.push(Value::None);

        // When: native float storage attempts to commit after representation changed.
        let committed = sequence.commit_float_snapshot(version, vec![9.0]);

        // Then: the widened authoritative values remain intact.
        assert!(!committed);
        assert_eq!(sequence.to_vec(), vec![Value::Float(1.0), Value::None]);
    }

    #[test]
    fn widened_float_commit_requires_the_exact_integer_snapshot() {
        // Given: an integer list and its immutable layout version.
        let mut sequence = SequenceObject::from_values(vec![Value::SmallInt(1)]);
        let (_, version) = sequence.integer_snapshot().expect("integer snapshot");

        // When: the transaction commits float results against that exact version.
        let committed = sequence.commit_widened_float_snapshot(version, vec![1.5]);

        // Then: the strategy changes atomically and stale replay cannot overwrite it.
        assert!(committed);
        assert_eq!(sequence.to_vec(), vec![Value::Float(1.5)]);
        assert!(!sequence.commit_widened_float_snapshot(version, vec![9.0]));
        assert_eq!(sequence.to_vec(), vec![Value::Float(1.5)]);
    }
}

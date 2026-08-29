use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::handles::StableHandle;
use super::heap::{GcError, GcHeap, GcObject};
use super::value_word::{ScalarValue, ValueWord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ListLayoutKey {
    pub(crate) owner: StableHandle,
    pub(crate) epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListStrategy {
    Empty,
    ImmediateInteger,
    F64,
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListError {
    Heap(GcError),
    IndexOutOfBounds,
}

impl fmt::Display for ListError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Heap(error) => error.fmt(formatter),
            Self::IndexOutOfBounds => formatter.write_str("list index is out of bounds"),
        }
    }
}

impl std::error::Error for ListError {}

impl From<GcError> for ListError {
    fn from(value: GcError) -> Self {
        Self::Heap(value)
    }
}

#[derive(Debug)]
enum Storage {
    Empty,
    Integers(Vec<(i64, ValueWord)>),
    Floats(Vec<(u64, ValueWord)>),
    Generic(Vec<ValueWord>),
}

#[derive(Debug)]
pub(crate) struct TypedList {
    owner: StableHandle,
    epoch: u64,
    storage: Storage,
    native_version: Arc<AtomicU64>,
    native_integers: Arc<[i64]>,
}

#[derive(Debug, Clone)]
pub(crate) struct NativeIntegerLease {
    owner: StableHandle,
    layout_epoch: u64,
    version: u64,
    version_source: Arc<AtomicU64>,
    values: Arc<[i64]>,
}

impl NativeIntegerLease {
    pub(crate) const fn owner(&self) -> StableHandle {
        self.owner
    }

    pub(crate) const fn layout_epoch(&self) -> u64 {
        self.layout_epoch
    }

    pub(crate) const fn version(&self) -> u64 {
        self.version
    }

    pub(crate) fn values(&self) -> &[i64] {
        &self.values
    }

    pub(crate) fn is_current(&self) -> bool {
        self.version & 1 == 0 && self.version_source.load(Ordering::Acquire) == self.version
    }
}

impl TypedList {
    pub(crate) fn new(heap: &GcHeap) -> Result<Self, GcError> {
        Ok(Self {
            owner: heap.allocate(GcObject::new())?,
            epoch: 0,
            storage: Storage::Empty,
            native_version: Arc::new(AtomicU64::new(0)),
            native_integers: Arc::from([]),
        })
    }

    pub(crate) const fn handle(&self) -> StableHandle {
        self.owner
    }

    pub(crate) const fn key(&self) -> ListLayoutKey {
        ListLayoutKey {
            owner: self.owner,
            epoch: self.epoch,
        }
    }

    pub(crate) const fn strategy(&self) -> ListStrategy {
        match self.storage {
            Storage::Empty => ListStrategy::Empty,
            Storage::Integers(_) => ListStrategy::ImmediateInteger,
            Storage::Floats(_) => ListStrategy::F64,
            Storage::Generic(_) => ListStrategy::Generic,
        }
    }

    pub(crate) fn push(&mut self, heap: &GcHeap, value: ValueWord) -> Result<(), ListError> {
        self.record_reference(heap, value)?;
        let class = classify(heap, value)?;
        self.storage = match std::mem::replace(&mut self.storage, Storage::Empty) {
            Storage::Empty => match class {
                ValueClass::ImmediateInteger(integer) => Storage::Integers(vec![(integer, value)]),
                ValueClass::Float(bits) => Storage::Floats(vec![(bits, value)]),
                ValueClass::Generic => Storage::Generic(vec![value]),
            },
            Storage::Integers(mut values) => match class {
                ValueClass::ImmediateInteger(integer) => {
                    values.push((integer, value));
                    Storage::Integers(values)
                }
                ValueClass::Float(_) | ValueClass::Generic => {
                    let mut generic: Vec<_> = values.into_iter().map(|(_, word)| word).collect();
                    generic.push(value);
                    Storage::Generic(generic)
                }
            },
            Storage::Floats(mut values) => match class {
                ValueClass::Float(bits) => {
                    values.push((bits, value));
                    Storage::Floats(values)
                }
                ValueClass::ImmediateInteger(_) | ValueClass::Generic => {
                    let mut generic: Vec<_> = values.into_iter().map(|(_, word)| word).collect();
                    generic.push(value);
                    Storage::Generic(generic)
                }
            },
            Storage::Generic(mut values) => {
                values.push(value);
                Storage::Generic(values)
            }
        };
        self.epoch = self.epoch.wrapping_add(1);
        self.refresh_native_integers();
        Ok(())
    }

    pub(crate) fn get(&self, _heap: &GcHeap, index: usize) -> Result<ValueWord, ListError> {
        match &self.storage {
            Storage::Empty => None,
            Storage::Integers(values) => values.get(index).map(|(_, word)| *word),
            Storage::Floats(values) => values.get(index).map(|(_, word)| *word),
            Storage::Generic(values) => values.get(index).copied(),
        }
        .ok_or(ListError::IndexOutOfBounds)
    }

    pub(crate) fn set(
        &mut self,
        heap: &GcHeap,
        index: usize,
        value: ValueWord,
    ) -> Result<(), ListError> {
        if index >= self.len() {
            return Err(ListError::IndexOutOfBounds);
        }
        self.record_reference(heap, value)?;
        let class = classify(heap, value)?;
        let same_strategy = matches!(
            (&self.storage, class),
            (Storage::Integers(_), ValueClass::ImmediateInteger(_))
                | (Storage::Floats(_), ValueClass::Float(_))
                | (Storage::Generic(_), _)
        );
        if !same_strategy {
            self.widen_to_generic();
        }
        match (&mut self.storage, class) {
            (Storage::Integers(values), ValueClass::ImmediateInteger(integer)) => {
                values[index] = (integer, value);
            }
            (Storage::Floats(values), ValueClass::Float(bits)) => values[index] = (bits, value),
            (Storage::Generic(values), _) => values[index] = value,
            (Storage::Empty, _)
            | (Storage::Integers(_), ValueClass::Float(_) | ValueClass::Generic)
            | (Storage::Floats(_), ValueClass::ImmediateInteger(_) | ValueClass::Generic) => {
                return Err(ListError::IndexOutOfBounds);
            }
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.refresh_native_integers();
        Ok(())
    }

    pub(crate) fn insert(
        &mut self,
        heap: &GcHeap,
        index: usize,
        value: ValueWord,
    ) -> Result<(), ListError> {
        if index > self.len() {
            return Err(ListError::IndexOutOfBounds);
        }
        self.record_reference(heap, value)?;
        self.widen_to_generic();
        let Storage::Generic(values) = &mut self.storage else {
            return Err(ListError::IndexOutOfBounds);
        };
        values.insert(index, value);
        self.epoch = self.epoch.wrapping_add(1);
        self.refresh_native_integers();
        Ok(())
    }

    pub(crate) fn remove(&mut self, index: usize) -> Result<ValueWord, ListError> {
        let value = match &mut self.storage {
            Storage::Empty => None,
            Storage::Integers(values) => values.get(index).map(|(_, word)| *word),
            Storage::Floats(values) => values.get(index).map(|(_, word)| *word),
            Storage::Generic(values) => values.get(index).copied(),
        }
        .ok_or(ListError::IndexOutOfBounds)?;
        match &mut self.storage {
            Storage::Empty => return Err(ListError::IndexOutOfBounds),
            Storage::Integers(values) => {
                values.remove(index);
            }
            Storage::Floats(values) => {
                values.remove(index);
            }
            Storage::Generic(values) => {
                values.remove(index);
            }
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.refresh_native_integers();
        Ok(value)
    }

    pub(crate) fn len(&self) -> usize {
        match &self.storage {
            Storage::Empty => 0,
            Storage::Integers(values) => values.len(),
            Storage::Floats(values) => values.len(),
            Storage::Generic(values) => values.len(),
        }
    }

    pub(crate) fn native_integer_lease(&self) -> Option<NativeIntegerLease> {
        if !matches!(self.storage, Storage::Integers(_)) {
            return None;
        }
        let version = self.native_version.load(Ordering::Acquire);
        (version & 1 == 0).then(|| NativeIntegerLease {
            owner: self.owner,
            layout_epoch: self.epoch,
            version,
            version_source: Arc::clone(&self.native_version),
            values: Arc::clone(&self.native_integers),
        })
    }

    fn record_reference(&self, heap: &GcHeap, value: ValueWord) -> Result<(), GcError> {
        if let Some(target) = value.as_handle(heap) {
            heap.store_reference(self.owner, target)?;
        }
        Ok(())
    }

    fn widen_to_generic(&mut self) {
        let old = std::mem::replace(&mut self.storage, Storage::Empty);
        self.storage = match old {
            Storage::Empty => Storage::Generic(Vec::new()),
            Storage::Integers(values) => {
                Storage::Generic(values.into_iter().map(|(_, word)| word).collect())
            }
            Storage::Floats(values) => {
                Storage::Generic(values.into_iter().map(|(_, word)| word).collect())
            }
            Storage::Generic(values) => Storage::Generic(values),
        };
    }

    fn refresh_native_integers(&mut self) {
        self.native_version.fetch_add(1, Ordering::AcqRel);
        self.native_integers = match &self.storage {
            Storage::Integers(values) => values
                .iter()
                .map(|(integer, _)| *integer)
                .collect::<Vec<_>>()
                .into(),
            Storage::Empty | Storage::Floats(_) | Storage::Generic(_) => Arc::from([]),
        };
        self.native_version.fetch_add(1, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy)]
enum ValueClass {
    ImmediateInteger(i64),
    Float(u64),
    Generic,
}

fn classify(heap: &GcHeap, value: ValueWord) -> Result<ValueClass, GcError> {
    match value.decode_scalar(heap) {
        Ok(ScalarValue::Integer(integer)) if !value.is_boxed() => {
            Ok(ValueClass::ImmediateInteger(integer))
        }
        Ok(ScalarValue::Integer(_)) => Ok(ValueClass::Generic),
        Ok(ScalarValue::FloatBits(bits)) => Ok(ValueClass::Float(bits)),
        Err(GcError::NotScalar) => Ok(ValueClass::Generic),
        Err(error) => Err(error),
    }
}

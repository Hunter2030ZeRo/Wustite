use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::types::{HeapAdapterError, HeapValue};
use crate::adaptive_v2::handles::StableHandle;
use crate::adaptive_v2::heap::{GcConfig, GcHeap, HeapMetrics};
use crate::adaptive_v2::lists::TypedList;
use crate::adaptive_v2::objects::DenseObject;
use crate::adaptive_v2::roots::RootInventory;
use crate::adaptive_v2::shapes::{ShapeId, ShapeTable};
use crate::adaptive_v2::symbols::SymbolTable;
use crate::adaptive_v2::value_word::{ScalarValue, ValueWord};

pub(super) type BinaryCallable = dyn Fn(i64, i64) -> i64 + Send + Sync + 'static;

pub(super) struct Metadata {
    pub(super) symbols: SymbolTable,
    pub(super) shapes: ShapeTable,
    pub(super) root_shape: ShapeId,
}

pub(super) struct RuntimeInner {
    pub(super) heap: GcHeap,
    pub(super) metadata: Mutex<Metadata>,
    pub(super) objects: RwLock<HashMap<StableHandle, Arc<Mutex<DenseObject>>>>,
    pub(super) lists: RwLock<HashMap<StableHandle, Arc<Mutex<TypedList>>>>,
    pub(super) calls: RwLock<HashMap<StableHandle, Arc<BinaryCallable>>>,
}

struct HostLease {
    heap: GcHeap,
    handle: StableHandle,
}

impl Drop for HostLease {
    fn drop(&mut self) {
        let _ = self.heap.unpin_host(self.handle);
    }
}

#[derive(Clone)]
pub(crate) struct RootedValue {
    value: HeapValue,
    owner: Arc<RuntimeInner>,
    _lease: Option<Arc<HostLease>>,
}

impl RootedValue {
    pub(crate) const fn value(&self) -> HeapValue {
        self.value
    }

    pub(super) fn belongs_to(&self, runtime: &Arc<RuntimeInner>) -> bool {
        Arc::ptr_eq(&self.owner, runtime)
    }
}

impl fmt::Debug for RootedValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RootedValue")
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub(crate) struct AdaptiveHeapRuntime {
    pub(super) inner: Arc<RuntimeInner>,
}

impl AdaptiveHeapRuntime {
    pub(crate) fn new(config: GcConfig) -> Self {
        let symbols = SymbolTable::new();
        let mut shapes = ShapeTable::new(symbols.namespace());
        let (_, root_shape) = shapes.new_class_with_root();
        Self {
            inner: Arc::new(RuntimeInner {
                heap: GcHeap::new(config),
                metadata: Mutex::new(Metadata {
                    symbols,
                    shapes,
                    root_shape,
                }),
                objects: RwLock::new(HashMap::new()),
                lists: RwLock::new(HashMap::new()),
                calls: RwLock::new(HashMap::new()),
            }),
        }
    }

    pub(crate) fn scalar(&self, scalar: ScalarValue) -> Result<RootedValue, HeapAdapterError> {
        let word = ValueWord::encode_scalar(scalar, &self.inner.heap)?;
        let value = self.value_from_word(word)?;
        Ok(self.rooted(value, value.handle().is_some()))
    }

    pub(crate) fn decode_scalar(&self, value: HeapValue) -> Result<ScalarValue, HeapAdapterError> {
        value.validate(&self.inner.heap)?;
        value
            .word()
            .decode_scalar(&self.inner.heap)
            .map_err(Into::into)
    }

    pub(crate) fn root(&self, value: HeapValue) -> Result<RootedValue, HeapAdapterError> {
        value.validate(&self.inner.heap)?;
        if let Some(handle) = value.handle() {
            self.inner.heap.pin_host(handle)?;
        }
        Ok(self.rooted(value, value.handle().is_some()))
    }

    pub(super) fn rooted(&self, value: HeapValue, pinned: bool) -> RootedValue {
        let lease = value.handle().filter(|_| pinned).map(|handle| {
            Arc::new(HostLease {
                heap: self.inner.heap.clone(),
                handle,
            })
        });
        RootedValue {
            value,
            owner: Arc::clone(&self.inner),
            _lease: lease,
        }
    }

    pub(super) fn value_from_word(&self, word: ValueWord) -> Result<HeapValue, HeapAdapterError> {
        match word.as_handle(&self.inner.heap) {
            Some(handle) => {
                self.inner.heap.resolve(handle)?;
                Ok(HeapValue::from_handle(handle))
            }
            None => Ok(HeapValue::immediate(word)),
        }
    }

    pub(crate) fn root_inventory(&self) -> RootInventory {
        self.inner.heap.host_root_inventory()
    }

    pub(crate) fn collect_minor(&self) -> Result<(), HeapAdapterError> {
        self.inner.heap.minor_collect(&self.root_inventory())?;
        self.sweep_payloads();
        Ok(())
    }

    pub(crate) fn collect_major(&self) -> Result<(), HeapAdapterError> {
        self.inner
            .heap
            .start_major(&self.root_inventory())?
            .finish()?;
        self.sweep_payloads();
        Ok(())
    }

    pub(crate) fn payload_counts(&self) -> (usize, usize, usize) {
        (
            read_lock(&self.inner.objects).len(),
            read_lock(&self.inner.lists).len(),
            read_lock(&self.inner.calls).len(),
        )
    }

    pub(crate) fn heap_metrics(&self) -> HeapMetrics {
        self.inner.heap.metrics()
    }

    pub(super) fn sweep_payloads(&self) {
        let heap = &self.inner.heap;
        write_lock(&self.inner.objects).retain(|handle, _| heap.resolve(*handle).is_ok());
        write_lock(&self.inner.lists).retain(|handle, _| heap.resolve(*handle).is_ok());
        write_lock(&self.inner.calls).retain(|handle, _| heap.resolve(*handle).is_ok());
    }
}

pub(super) fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

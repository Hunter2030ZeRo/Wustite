use std::sync::Arc;

use super::runtime::{
    AdaptiveHeapRuntime, BinaryCallable, RootedValue, mutex_lock, read_lock, write_lock,
};
use super::types::{HeapAdapterError, HeapValue, PayloadKind};
use crate::adaptive_v2::handles::StableHandle;
use crate::adaptive_v2::heap::GcObject;
use crate::adaptive_v2::lists::{NativeIntegerLease, TypedList};
use crate::adaptive_v2::objects::DenseObject;
use crate::adaptive_v2::value_word::ScalarValue;

pub(crate) trait NativeHeapContext {
    fn object_store(
        &self,
        owner: HeapValue,
        field: &str,
        value: HeapValue,
    ) -> Result<(), HeapAdapterError>;

    fn object_load(&self, owner: HeapValue, field: &str) -> Result<RootedValue, HeapAdapterError>;

    fn list_append_value(&self, owner: HeapValue, value: HeapValue)
    -> Result<(), HeapAdapterError>;

    fn list_load(&self, owner: HeapValue, index: usize) -> Result<RootedValue, HeapAdapterError>;

    fn call_binary_value(
        &self,
        callable: HeapValue,
        left: HeapValue,
        right: HeapValue,
    ) -> Result<RootedValue, HeapAdapterError>;
}

impl AdaptiveHeapRuntime {
    pub(crate) fn native_integer_lease(
        &self,
        owner: &RootedValue,
    ) -> Result<NativeIntegerLease, HeapAdapterError> {
        if !owner.belongs_to(&self.inner) {
            return Err(HeapAdapterError::WrongRuntime);
        }
        let handle = owner
            .value()
            .handle()
            .ok_or(HeapAdapterError::ExpectedHandle)?;
        self.inner.heap.resolve(handle)?;
        let list = read_lock(&self.inner.lists)
            .get(&handle)
            .cloned()
            .ok_or(HeapAdapterError::MissingPayload(PayloadKind::List))?;
        mutex_lock(&list)
            .native_integer_lease()
            .ok_or(HeapAdapterError::ExpectedInteger)
    }
    pub(crate) fn allocate_object(&self) -> Result<RootedValue, HeapAdapterError> {
        let root_shape = mutex_lock(&self.inner.metadata).root_shape;
        let collections = self.inner.heap.metrics().minor_collections;
        let object = DenseObject::new(&self.inner.heap, root_shape)?;
        let handle = object.handle();
        if self.inner.heap.metrics().minor_collections != collections {
            self.sweep_payloads();
        }
        self.inner.heap.pin_host(handle)?;
        write_lock(&self.inner.objects).insert(handle, Arc::new(std::sync::Mutex::new(object)));
        Ok(self.rooted(HeapValue::from_handle(handle), true))
    }

    pub(crate) fn allocate_list(&self) -> Result<RootedValue, HeapAdapterError> {
        let collections = self.inner.heap.metrics().minor_collections;
        let list = TypedList::new(&self.inner.heap)?;
        let handle = list.handle();
        if self.inner.heap.metrics().minor_collections != collections {
            self.sweep_payloads();
        }
        self.inner.heap.pin_host(handle)?;
        write_lock(&self.inner.lists).insert(handle, Arc::new(std::sync::Mutex::new(list)));
        Ok(self.rooted(HeapValue::from_handle(handle), true))
    }

    pub(crate) fn register_binary_callable(
        &self,
        callable: impl Fn(i64, i64) -> i64 + Send + Sync + 'static,
    ) -> Result<RootedValue, HeapAdapterError> {
        let collections = self.inner.heap.metrics().minor_collections;
        let handle = self.inner.heap.allocate(GcObject::new())?;
        if self.inner.heap.metrics().minor_collections != collections {
            self.sweep_payloads();
        }
        self.inner.heap.pin_host(handle)?;
        let callable: Arc<BinaryCallable> = Arc::new(callable);
        write_lock(&self.inner.calls).insert(handle, callable);
        Ok(self.rooted(HeapValue::from_handle(handle), true))
    }

    pub(crate) fn object_set(
        &self,
        owner: &RootedValue,
        field: &str,
        value: HeapValue,
    ) -> Result<(), HeapAdapterError> {
        let owner_handle = self.rooted_handle(owner, PayloadKind::Object)?;
        let _value_root = self.root(value)?;
        let object = self.object_payload(owner_handle)?;
        let mut metadata = mutex_lock(&self.inner.metadata);
        let symbol = metadata.symbols.intern(field)?;
        let mut object = mutex_lock(&object);
        object
            .set_field(&self.inner.heap, &mut metadata.shapes, symbol, value.word())
            .map_err(Into::into)
    }

    pub(crate) fn object_get(
        &self,
        owner: &RootedValue,
        field: &str,
    ) -> Result<RootedValue, HeapAdapterError> {
        let owner_handle = self.rooted_handle(owner, PayloadKind::Object)?;
        let object = self.object_payload(owner_handle)?;
        let mut metadata = mutex_lock(&self.inner.metadata);
        let symbol = metadata.symbols.intern(field)?;
        let word = mutex_lock(&object).get_field(&metadata.shapes, symbol)?;
        let value = self.value_from_word(word)?;
        self.root(value)
    }

    pub(crate) fn list_append(
        &self,
        owner: &RootedValue,
        value: HeapValue,
    ) -> Result<(), HeapAdapterError> {
        let owner_handle = self.rooted_handle(owner, PayloadKind::List)?;
        let _value_root = self.root(value)?;
        let list = self.list_payload(owner_handle)?;
        mutex_lock(&list)
            .push(&self.inner.heap, value.word())
            .map_err(Into::into)
    }

    pub(crate) fn list_get(
        &self,
        owner: &RootedValue,
        index: usize,
    ) -> Result<RootedValue, HeapAdapterError> {
        let owner_handle = self.rooted_handle(owner, PayloadKind::List)?;
        let list = self.list_payload(owner_handle)?;
        let word = mutex_lock(&list).get(&self.inner.heap, index)?;
        let value = self.value_from_word(word)?;
        self.root(value)
    }

    pub(crate) fn list_set(
        &self,
        owner: &RootedValue,
        index: usize,
        value: HeapValue,
    ) -> Result<(), HeapAdapterError> {
        let owner_handle = self.rooted_handle(owner, PayloadKind::List)?;
        let _value_root = self.root(value)?;
        let list = self.list_payload(owner_handle)?;
        mutex_lock(&list)
            .set(&self.inner.heap, index, value.word())
            .map_err(Into::into)
    }

    pub(crate) fn list_insert(
        &self,
        owner: &RootedValue,
        index: usize,
        value: HeapValue,
    ) -> Result<(), HeapAdapterError> {
        let owner_handle = self.rooted_handle(owner, PayloadKind::List)?;
        let _value_root = self.root(value)?;
        let list = self.list_payload(owner_handle)?;
        mutex_lock(&list)
            .insert(&self.inner.heap, index, value.word())
            .map_err(Into::into)
    }

    pub(crate) fn list_pop(
        &self,
        owner: &RootedValue,
        index: usize,
    ) -> Result<RootedValue, HeapAdapterError> {
        let owner_handle = self.rooted_handle(owner, PayloadKind::List)?;
        let list = self.list_payload(owner_handle)?;
        let word = mutex_lock(&list).remove(index)?;
        self.root(self.value_from_word(word)?)
    }

    pub(crate) fn list_len(&self, owner: &RootedValue) -> Result<usize, HeapAdapterError> {
        let owner_handle = self.rooted_handle(owner, PayloadKind::List)?;
        let list = self.list_payload(owner_handle)?;
        Ok(mutex_lock(&list).len())
    }

    pub(crate) fn call_binary(
        &self,
        callable: &RootedValue,
        left: HeapValue,
        right: HeapValue,
    ) -> Result<RootedValue, HeapAdapterError> {
        let callable_handle = self.rooted_handle(callable, PayloadKind::Callable)?;
        let _left_root = self.root(left)?;
        let _right_root = self.root(right)?;
        let function = read_lock(&self.inner.calls)
            .get(&callable_handle)
            .cloned()
            .ok_or(HeapAdapterError::MissingPayload(PayloadKind::Callable))?;
        let left = self.integer_value(left)?;
        let right = self.integer_value(right)?;
        self.scalar(ScalarValue::Integer(function(left, right)))
    }

    fn integer_value(&self, value: HeapValue) -> Result<i64, HeapAdapterError> {
        match self.decode_scalar(value)? {
            ScalarValue::Integer(integer) => Ok(integer),
            ScalarValue::FloatBits(_) => Err(HeapAdapterError::ExpectedInteger),
        }
    }

    fn rooted_handle(
        &self,
        value: &RootedValue,
        kind: PayloadKind,
    ) -> Result<StableHandle, HeapAdapterError> {
        if !value.belongs_to(&self.inner) {
            return Err(HeapAdapterError::WrongRuntime);
        }
        let handle = value
            .value()
            .handle()
            .ok_or(HeapAdapterError::ExpectedHandle)?;
        self.inner.heap.resolve(handle)?;
        match kind {
            PayloadKind::Object if read_lock(&self.inner.objects).contains_key(&handle) => {
                Ok(handle)
            }
            PayloadKind::List if read_lock(&self.inner.lists).contains_key(&handle) => Ok(handle),
            PayloadKind::Callable if read_lock(&self.inner.calls).contains_key(&handle) => {
                Ok(handle)
            }
            PayloadKind::Object | PayloadKind::List | PayloadKind::Callable => {
                Err(HeapAdapterError::MissingPayload(kind))
            }
        }
    }

    fn object_payload(
        &self,
        handle: StableHandle,
    ) -> Result<Arc<std::sync::Mutex<DenseObject>>, HeapAdapterError> {
        read_lock(&self.inner.objects)
            .get(&handle)
            .cloned()
            .ok_or(HeapAdapterError::MissingPayload(PayloadKind::Object))
    }

    fn list_payload(
        &self,
        handle: StableHandle,
    ) -> Result<Arc<std::sync::Mutex<TypedList>>, HeapAdapterError> {
        read_lock(&self.inner.lists)
            .get(&handle)
            .cloned()
            .ok_or(HeapAdapterError::MissingPayload(PayloadKind::List))
    }
}

impl NativeHeapContext for AdaptiveHeapRuntime {
    fn object_store(
        &self,
        owner: HeapValue,
        field: &str,
        value: HeapValue,
    ) -> Result<(), HeapAdapterError> {
        let owner = self.root(owner)?;
        self.object_set(&owner, field, value)
    }

    fn object_load(&self, owner: HeapValue, field: &str) -> Result<RootedValue, HeapAdapterError> {
        let owner = self.root(owner)?;
        self.object_get(&owner, field)
    }

    fn list_append_value(
        &self,
        owner: HeapValue,
        value: HeapValue,
    ) -> Result<(), HeapAdapterError> {
        let owner = self.root(owner)?;
        self.list_append(&owner, value)
    }

    fn list_load(&self, owner: HeapValue, index: usize) -> Result<RootedValue, HeapAdapterError> {
        let owner = self.root(owner)?;
        self.list_get(&owner, index)
    }

    fn call_binary_value(
        &self,
        callable: HeapValue,
        left: HeapValue,
        right: HeapValue,
    ) -> Result<RootedValue, HeapAdapterError> {
        let callable = self.root(callable)?;
        self.call_binary(&callable, left, right)
    }
}

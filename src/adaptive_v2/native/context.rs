use std::collections::HashMap;

use super::{NativeError, NativeRuntime, NativeValue};
use crate::adaptive_v2::heap::GcConfig;
use crate::adaptive_v2::lists::NativeIntegerLease;
use crate::adaptive_v2::public_heap::operations::NativeHeapContext;
use crate::adaptive_v2::public_heap::runtime::{AdaptiveHeapRuntime, RootedValue};
use crate::adaptive_v2::public_heap::types::HeapValue;
use crate::adaptive_v2::value_word::ScalarValue;

pub(super) const ERROR_VALUE: i64 = i64::MIN;

pub(super) trait HelperOperations {
    fn native_integer_lease(
        &mut self,
        _handle: u64,
    ) -> Result<Option<NativeIntegerLease>, NativeError> {
        Ok(None)
    }
    fn object_get(&mut self, handle: u64, key: i64) -> Result<i64, NativeError>;
    fn object_set(&mut self, handle: u64, key: i64, value: i64) -> Result<i64, NativeError>;
    fn list_get(&mut self, handle: u64, index: i64) -> Result<i64, NativeError>;
    fn list_set(&mut self, handle: u64, index: i64, value: i64) -> Result<i64, NativeError>;
    fn list_append(&mut self, handle: u64, value: i64) -> Result<i64, NativeError>;
    fn direct_call(&mut self, callee: u64, left: i64, right: i64) -> Result<i64, NativeError>;
}

pub(super) struct HelperContext<'a> {
    operations: &'a mut dyn HelperOperations,
    error: Option<NativeError>,
}

impl<'a> HelperContext<'a> {
    pub(super) fn new(operations: &'a mut dyn HelperOperations) -> Self {
        Self {
            operations,
            error: None,
        }
    }

    pub(super) fn error(&self) -> Option<&NativeError> {
        self.error.as_ref()
    }

    pub(super) fn invoke(
        &mut self,
        operation: impl FnOnce(&mut dyn HelperOperations) -> Result<i64, NativeError>,
    ) -> i64 {
        match operation(self.operations) {
            Ok(value) => value,
            Err(error) => {
                self.error = Some(error);
                ERROR_VALUE
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct AdaptiveNativeContext {
    runtime: AdaptiveHeapRuntime,
    values: HashMap<u64, RootedValue>,
    fields: HashMap<i64, String>,
    calls: HashMap<u64, u64>,
    next_value: u64,
}

impl AdaptiveNativeContext {
    pub(crate) fn new(config: GcConfig) -> Self {
        Self::with_runtime(AdaptiveHeapRuntime::new(config))
    }

    pub(crate) fn with_runtime(runtime: AdaptiveHeapRuntime) -> Self {
        Self {
            runtime,
            values: HashMap::new(),
            fields: HashMap::new(),
            calls: HashMap::new(),
            next_value: 1,
        }
    }

    pub(crate) fn allocate_object(&mut self) -> Result<NativeValue, NativeError> {
        let rooted = self
            .runtime
            .allocate_object()
            .map_err(|_| NativeError::Helper)?;
        self.register(rooted)
    }

    pub(crate) fn allocate_list(&mut self) -> Result<NativeValue, NativeError> {
        let rooted = self
            .runtime
            .allocate_list()
            .map_err(|_| NativeError::Helper)?;
        self.register(rooted)
    }

    pub(crate) fn register_binary_callable(
        &mut self,
        callable: impl Fn(i64, i64) -> i64 + Send + Sync + 'static,
    ) -> Result<NativeValue, NativeError> {
        let rooted = self
            .runtime
            .register_binary_callable(callable)
            .map_err(|_| NativeError::Helper)?;
        self.register(rooted)
    }

    pub(crate) fn bind_field(&mut self, key: i64, field: &str) {
        self.fields.insert(key, field.to_owned());
    }

    pub(crate) fn bind_callable(
        &mut self,
        callee: u64,
        callable: NativeValue,
    ) -> Result<(), NativeError> {
        let NativeValue::Handle(alias) = callable else {
            return Err(NativeError::MalformedValue);
        };
        if !self.values.contains_key(&alias) {
            return Err(NativeError::Helper);
        }
        self.calls.insert(callee, alias);
        Ok(())
    }

    pub(crate) fn ensure_binary_callable(
        &mut self,
        callee: u64,
        callable: impl Fn(i64, i64) -> i64 + Send + Sync + 'static,
    ) -> Result<(), NativeError> {
        if self.calls.contains_key(&callee) {
            return Ok(());
        }
        let value = self.register_binary_callable(callable)?;
        self.bind_callable(callee, value)
    }

    pub(crate) fn append_integer(&self, list: NativeValue, value: i64) -> Result<(), NativeError> {
        let NativeValue::Handle(alias) = list else {
            return Err(NativeError::MalformedValue);
        };
        NativeHeapContext::list_append_value(
            &self.runtime,
            self.value(alias)?,
            self.integer(value)?,
        )
        .map_err(|_| NativeError::Helper)
    }

    pub(crate) fn set_integer_field(
        &mut self,
        object: NativeValue,
        key: i64,
        field: &str,
        value: i64,
    ) -> Result<(), NativeError> {
        self.bind_field(key, field);
        let NativeValue::Handle(alias) = object else {
            return Err(NativeError::MalformedValue);
        };
        NativeHeapContext::object_store(
            &self.runtime,
            self.value(alias)?,
            field,
            self.integer(value)?,
        )
        .map_err(|_| NativeError::Helper)
    }

    pub(crate) fn get_integer_field(
        &mut self,
        object: NativeValue,
        key: i64,
        field: &str,
    ) -> Result<i64, NativeError> {
        self.bind_field(key, field);
        let NativeValue::Handle(alias) = object else {
            return Err(NativeError::MalformedValue);
        };
        let value = NativeHeapContext::object_load(&self.runtime, self.value(alias)?, field)
            .map_err(|_| NativeError::Helper)?;
        self.decode_integer(value)
    }

    pub(crate) fn integer_at(&self, list: NativeValue, index: usize) -> Result<i64, NativeError> {
        let NativeValue::Handle(alias) = list else {
            return Err(NativeError::MalformedValue);
        };
        let value = NativeHeapContext::list_load(&self.runtime, self.value(alias)?, index)
            .map_err(|_| NativeError::Helper)?;
        self.decode_integer(value)
    }

    pub(crate) fn set_integer_at(
        &self,
        list: NativeValue,
        index: usize,
        value: i64,
    ) -> Result<(), NativeError> {
        let NativeValue::Handle(alias) = list else {
            return Err(NativeError::MalformedValue);
        };
        let value = self.integer(value)?;
        self.runtime
            .list_set(self.rooted(alias)?, index, value)
            .map_err(|_| NativeError::Helper)
    }

    pub(crate) fn insert_integer(
        &self,
        list: NativeValue,
        index: usize,
        value: i64,
    ) -> Result<(), NativeError> {
        let NativeValue::Handle(alias) = list else {
            return Err(NativeError::MalformedValue);
        };
        let value = self.integer(value)?;
        self.runtime
            .list_insert(self.rooted(alias)?, index, value)
            .map_err(|_| NativeError::Helper)
    }

    pub(crate) fn pop_integer(&self, list: NativeValue, index: usize) -> Result<i64, NativeError> {
        let NativeValue::Handle(alias) = list else {
            return Err(NativeError::MalformedValue);
        };
        let value = self
            .runtime
            .list_pop(self.rooted(alias)?, index)
            .map_err(|_| NativeError::Helper)?;
        self.decode_integer(value)
    }

    pub(crate) fn list_len(&self, list: NativeValue) -> Result<usize, NativeError> {
        let NativeValue::Handle(alias) = list else {
            return Err(NativeError::MalformedValue);
        };
        self.runtime
            .list_len(self.rooted(alias)?)
            .map_err(|_| NativeError::Helper)
    }

    pub(crate) fn direct_call_value(
        &mut self,
        callee: u64,
        left: i64,
        right: i64,
    ) -> Result<i64, NativeError> {
        HelperOperations::direct_call(self, callee, left, right)
    }

    pub(crate) fn collect_minor(&self) -> Result<(), NativeError> {
        self.runtime
            .collect_minor()
            .map_err(|_| NativeError::Helper)
    }

    pub(crate) fn rooted_value(&self, value: NativeValue) -> Result<RootedValue, NativeError> {
        let NativeValue::Handle(alias) = value else {
            return Err(NativeError::MalformedValue);
        };
        self.rooted(alias).cloned()
    }

    pub(crate) fn discard_value(&mut self, value: NativeValue) -> Result<(), NativeError> {
        let NativeValue::Handle(alias) = value else {
            return Err(NativeError::MalformedValue);
        };
        self.values.remove(&alias).ok_or(NativeError::Helper)?;
        self.calls.retain(|_, candidate| *candidate != alias);
        Ok(())
    }

    fn register(&mut self, rooted: RootedValue) -> Result<NativeValue, NativeError> {
        let alias = self.next_value;
        self.next_value = self
            .next_value
            .checked_add(1)
            .ok_or(NativeError::CountOverflow)?;
        self.values.insert(alias, rooted);
        Ok(NativeValue::Handle(alias))
    }

    fn value(&self, alias: u64) -> Result<HeapValue, NativeError> {
        self.values
            .get(&alias)
            .map(RootedValue::value)
            .ok_or(NativeError::Helper)
    }

    fn rooted(&self, alias: u64) -> Result<&RootedValue, NativeError> {
        self.values.get(&alias).ok_or(NativeError::Helper)
    }

    fn field(&self, key: i64) -> Result<&str, NativeError> {
        self.fields
            .get(&key)
            .map(String::as_str)
            .ok_or(NativeError::Helper)
    }

    fn integer(&self, value: i64) -> Result<HeapValue, NativeError> {
        self.runtime
            .scalar(ScalarValue::Integer(value))
            .map(|rooted| rooted.value())
            .map_err(|_| NativeError::Helper)
    }

    fn decode_integer(&self, value: RootedValue) -> Result<i64, NativeError> {
        match self
            .runtime
            .decode_scalar(value.value())
            .map_err(|_| NativeError::Helper)?
        {
            ScalarValue::Integer(value) => Ok(value),
            ScalarValue::FloatBits(_) => Err(NativeError::MalformedValue),
        }
    }
}

impl HelperOperations for AdaptiveNativeContext {
    fn native_integer_lease(
        &mut self,
        handle: u64,
    ) -> Result<Option<NativeIntegerLease>, NativeError> {
        self.runtime
            .native_integer_lease(self.rooted(handle)?)
            .map(Some)
            .map_err(|_| NativeError::Helper)
    }
    fn object_get(&mut self, handle: u64, key: i64) -> Result<i64, NativeError> {
        let value =
            NativeHeapContext::object_load(&self.runtime, self.value(handle)?, self.field(key)?)
                .map_err(|_| NativeError::Helper)?;
        self.decode_integer(value)
    }

    fn object_set(&mut self, handle: u64, key: i64, value: i64) -> Result<i64, NativeError> {
        NativeHeapContext::object_store(
            &self.runtime,
            self.value(handle)?,
            self.field(key)?,
            self.integer(value)?,
        )
        .map_err(|_| NativeError::Helper)?;
        Ok(0)
    }

    fn list_get(&mut self, handle: u64, index: i64) -> Result<i64, NativeError> {
        let index = usize::try_from(index).map_err(|_| NativeError::MalformedValue)?;
        let value = NativeHeapContext::list_load(&self.runtime, self.value(handle)?, index)
            .map_err(|_| NativeError::Helper)?;
        self.decode_integer(value)
    }

    fn list_set(&mut self, handle: u64, index: i64, value: i64) -> Result<i64, NativeError> {
        let index = usize::try_from(index).map_err(|_| NativeError::MalformedValue)?;
        let value = self.integer(value)?;
        self.runtime
            .list_set(self.rooted(handle)?, index, value)
            .map_err(|_| NativeError::Helper)?;
        Ok(0)
    }

    fn list_append(&mut self, handle: u64, value: i64) -> Result<i64, NativeError> {
        NativeHeapContext::list_append_value(
            &self.runtime,
            self.value(handle)?,
            self.integer(value)?,
        )
        .map_err(|_| NativeError::Helper)?;
        Ok(0)
    }

    fn direct_call(&mut self, callee: u64, left: i64, right: i64) -> Result<i64, NativeError> {
        let alias = self
            .calls
            .get(&callee)
            .copied()
            .ok_or(NativeError::Helper)?;
        let value = NativeHeapContext::call_binary_value(
            &self.runtime,
            self.value(alias)?,
            self.integer(left)?,
            self.integer(right)?,
        )
        .map_err(|_| NativeError::Helper)?;
        self.decode_integer(value)
    }
}

impl HelperOperations for NativeRuntime {
    fn object_get(&mut self, handle: u64, key: i64) -> Result<i64, NativeError> {
        self.objects
            .get(&handle)
            .and_then(|fields| fields.get(&key))
            .copied()
            .ok_or(NativeError::Helper)
    }

    fn object_set(&mut self, handle: u64, key: i64, value: i64) -> Result<i64, NativeError> {
        let fields = self.objects.get_mut(&handle).ok_or(NativeError::Helper)?;
        fields.insert(key, value);
        Ok(0)
    }

    fn list_get(&mut self, handle: u64, index: i64) -> Result<i64, NativeError> {
        self.lists
            .get(&handle)
            .and_then(|list| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| list.get(index))
            })
            .copied()
            .ok_or(NativeError::Helper)
    }

    fn list_set(&mut self, handle: u64, index: i64, value: i64) -> Result<i64, NativeError> {
        let item = self
            .lists
            .get_mut(&handle)
            .and_then(|list| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| list.get_mut(index))
            })
            .ok_or(NativeError::Helper)?;
        *item = value;
        Ok(0)
    }

    fn list_append(&mut self, handle: u64, value: i64) -> Result<i64, NativeError> {
        let list = self.lists.get_mut(&handle).ok_or(NativeError::Helper)?;
        list.push(value);
        Ok(0)
    }

    fn direct_call(&mut self, callee: u64, left: i64, right: i64) -> Result<i64, NativeError> {
        self.calls
            .get(&callee)
            .map(|function| function(left, right))
            .ok_or(NativeError::Helper)
    }
}

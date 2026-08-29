use std::collections::HashMap;
use std::fmt;

use super::handles::StableHandle;
use super::heap::{GcError, GcHeap, GcObject};
use super::shapes::{ClassId, ShapeError, ShapeId, ShapeTable};
use super::symbols::{RuntimeNamespace, SymbolId};
use super::value_word::ValueWord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct MethodTarget(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectCallable {
    target: MethodTarget,
    receiver: StableHandle,
}

impl DirectCallable {
    pub(crate) const fn target(self) -> MethodTarget {
        self.target
    }

    pub(crate) const fn receiver(self) -> StableHandle {
        self.receiver
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoundMethod {
    identity: u64,
    callable: DirectCallable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttrValue {
    Field(ValueWord),
    BoundMethod(BoundMethod),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectError {
    Shape(ShapeError),
    Heap(GcError),
    MissingAttribute,
    MissingMethod,
    WrongRuntime,
    Capacity,
}

impl fmt::Display for ObjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shape(error) => error.fmt(formatter),
            Self::Heap(error) => error.fmt(formatter),
            Self::MissingAttribute => formatter.write_str("object attribute is missing"),
            Self::MissingMethod => formatter.write_str("object method is missing"),
            Self::WrongRuntime => formatter.write_str("method identity belongs to another runtime"),
            Self::Capacity => formatter.write_str("object metadata capacity exhausted"),
        }
    }
}

impl std::error::Error for ObjectError {}

impl From<ShapeError> for ObjectError {
    fn from(value: ShapeError) -> Self {
        Self::Shape(value)
    }
}

impl From<GcError> for ObjectError {
    fn from(value: GcError) -> Self {
        Self::Heap(value)
    }
}

#[derive(Debug)]
pub(crate) struct DenseObject {
    handle: StableHandle,
    shape: ShapeId,
    fields: Vec<Option<ValueWord>>,
}

impl DenseObject {
    pub(crate) fn new(heap: &GcHeap, shape: ShapeId) -> Result<Self, GcError> {
        Ok(Self {
            handle: heap.allocate(GcObject::new())?,
            shape,
            fields: Vec::new(),
        })
    }

    pub(crate) const fn handle(&self) -> StableHandle {
        self.handle
    }

    pub(crate) const fn shape(&self) -> ShapeId {
        self.shape
    }

    pub(crate) fn set_field(
        &mut self,
        heap: &GcHeap,
        shapes: &mut ShapeTable,
        symbol: SymbolId,
        value: ValueWord,
    ) -> Result<(), ObjectError> {
        let next = shapes.transition(self.shape, symbol)?;
        let slot =
            usize::try_from(shapes.slot(next, symbol)?).map_err(|_| ObjectError::Capacity)?;
        let required = shapes.shape(next)?.slot_count();
        self.fields.resize(required, None);
        if let Some(target) = value.as_handle(heap) {
            heap.store_reference(self.handle, target)?;
        }
        self.fields[slot] = Some(value);
        self.shape = next;
        Ok(())
    }

    pub(crate) fn get_field(
        &self,
        shapes: &ShapeTable,
        symbol: SymbolId,
    ) -> Result<ValueWord, ObjectError> {
        let slot = usize::try_from(shapes.slot(self.shape, symbol)?)
            .map_err(|_| ObjectError::MissingAttribute)?;
        self.fields
            .get(slot)
            .and_then(|value| *value)
            .ok_or(ObjectError::MissingAttribute)
    }
}

#[derive(Debug)]
pub(crate) struct MethodTable {
    namespace: RuntimeNamespace,
    methods: HashMap<(ClassId, SymbolId), (MethodTarget, u64)>,
    escaped: HashMap<(StableHandle, SymbolId), BoundMethod>,
    next_target: u32,
    next_bound: u64,
    materializations: u64,
}

impl MethodTable {
    pub(crate) fn new(namespace: RuntimeNamespace) -> Self {
        Self {
            namespace,
            methods: HashMap::new(),
            escaped: HashMap::new(),
            next_target: 0,
            next_bound: 0,
            materializations: 0,
        }
    }

    pub(crate) fn define(
        &mut self,
        class: ClassId,
        symbol: SymbolId,
    ) -> Result<MethodTarget, ObjectError> {
        if symbol.namespace() != self.namespace {
            return Err(ObjectError::WrongRuntime);
        }
        if let Some((target, _)) = self.methods.get(&(class, symbol)) {
            return Ok(*target);
        }
        let target = MethodTarget(self.next_target);
        self.next_target = self
            .next_target
            .checked_add(1)
            .ok_or(ObjectError::Capacity)?;
        self.methods.insert((class, symbol), (target, 0));
        Ok(target)
    }

    pub(crate) fn invalidate(
        &mut self,
        class: ClassId,
        symbol: SymbolId,
    ) -> Result<MethodTarget, ObjectError> {
        let (_, epoch) = self
            .methods
            .get(&(class, symbol))
            .copied()
            .ok_or(ObjectError::MissingMethod)?;
        let target = MethodTarget(self.next_target);
        self.next_target = self
            .next_target
            .checked_add(1)
            .ok_or(ObjectError::Capacity)?;
        self.methods
            .insert((class, symbol), (target, epoch.wrapping_add(1)));
        self.escaped
            .retain(|(_, cached_symbol), _| *cached_symbol != symbol);
        Ok(target)
    }

    pub(crate) fn resolve_direct(
        &self,
        shapes: &ShapeTable,
        object: &DenseObject,
        symbol: SymbolId,
    ) -> Result<DirectCallable, ObjectError> {
        let class = shapes.shape(object.shape)?.class();
        let (target, _) = self
            .methods
            .get(&(class, symbol))
            .copied()
            .ok_or(ObjectError::MissingMethod)?;
        Ok(DirectCallable {
            target,
            receiver: object.handle,
        })
    }

    pub(crate) fn resolve_attr(
        &mut self,
        shapes: &ShapeTable,
        object: &DenseObject,
        symbol: SymbolId,
    ) -> Result<AttrValue, ObjectError> {
        if let Ok(value) = object.get_field(shapes, symbol) {
            return Ok(AttrValue::Field(value));
        }
        if let Some(bound) = self.escaped.get(&(object.handle, symbol)) {
            return Ok(AttrValue::BoundMethod(*bound));
        }
        let callable = self.resolve_direct(shapes, object, symbol)?;
        let bound = BoundMethod {
            identity: self.next_bound,
            callable,
        };
        self.next_bound = self
            .next_bound
            .checked_add(1)
            .ok_or(ObjectError::Capacity)?;
        self.materializations += 1;
        self.escaped.insert((object.handle, symbol), bound);
        Ok(AttrValue::BoundMethod(bound))
    }

    pub(crate) const fn bound_materializations(&self) -> u64 {
        self.materializations
    }
}

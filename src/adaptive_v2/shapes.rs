use std::collections::HashMap;
use std::fmt;

use super::symbols::{RuntimeNamespace, SymbolId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ClassId {
    namespace: RuntimeNamespace,
    index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ShapeId {
    namespace: RuntimeNamespace,
    index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ShapeKey {
    pub(crate) shape: ShapeId,
    pub(crate) dependency_epoch: u64,
    pub(crate) layout_epoch: u64,
}

impl ShapeKey {
    pub(crate) const fn serialized_parts(self) -> (u64, u64, u64) {
        (
            self.shape.index as u64,
            self.dependency_epoch,
            self.layout_epoch,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shape {
    id: ShapeId,
    class: ClassId,
    slots: HashMap<SymbolId, u32>,
    dependency_epoch: u64,
    layout_epoch: u64,
}

impl Shape {
    pub(crate) const fn class(&self) -> ClassId {
        self.class
    }

    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeError {
    WrongRuntime,
    WrongClass,
    UnknownClass,
    UnknownShape,
    UnknownField,
    Stale,
    Capacity,
}

impl fmt::Display for ShapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongRuntime => formatter.write_str("shape identity belongs to another runtime"),
            Self::WrongClass => formatter.write_str("shape transition crosses class identity"),
            Self::UnknownClass => formatter.write_str("class identity is unknown"),
            Self::UnknownShape => formatter.write_str("shape identity is unknown"),
            Self::UnknownField => formatter.write_str("field is absent from shape"),
            Self::Stale => formatter.write_str("shape dependency epoch is stale"),
            Self::Capacity => formatter.write_str("shape table capacity exhausted"),
        }
    }
}

impl std::error::Error for ShapeError {}

#[derive(Debug)]
pub(crate) struct ShapeTable {
    namespace: RuntimeNamespace,
    shapes: Vec<Shape>,
    roots: HashMap<ClassId, ShapeId>,
    class_epochs: Vec<u64>,
    transitions: HashMap<(ShapeId, SymbolId), ShapeId>,
}

impl ShapeTable {
    pub(crate) fn new(namespace: RuntimeNamespace) -> Self {
        Self {
            namespace,
            shapes: Vec::new(),
            roots: HashMap::new(),
            class_epochs: Vec::new(),
            transitions: HashMap::new(),
        }
    }

    pub(crate) fn new_class(&mut self) -> ClassId {
        self.new_class_with_root().0
    }

    pub(crate) fn new_class_with_root(&mut self) -> (ClassId, ShapeId) {
        let index = u32::try_from(self.class_epochs.len()).unwrap_or(u32::MAX);
        let class = ClassId {
            namespace: self.namespace,
            index,
        };
        self.class_epochs.push(0);
        let root = self.insert_shape(class, HashMap::new(), 0, 0);
        self.roots.insert(class, root);
        (class, root)
    }

    pub(crate) fn root_shape(&self, class: ClassId) -> Result<ShapeId, ShapeError> {
        self.validate_class(class)?;
        self.roots
            .get(&class)
            .copied()
            .ok_or(ShapeError::UnknownClass)
    }

    pub(crate) fn shape(&self, id: ShapeId) -> Result<&Shape, ShapeError> {
        if id.namespace != self.namespace {
            return Err(ShapeError::WrongRuntime);
        }
        let index = usize::try_from(id.index).map_err(|_| ShapeError::UnknownShape)?;
        self.shapes.get(index).ok_or(ShapeError::UnknownShape)
    }

    pub(crate) fn transition(
        &mut self,
        from: ShapeId,
        symbol: SymbolId,
    ) -> Result<ShapeId, ShapeError> {
        if symbol.namespace() != self.namespace {
            return Err(ShapeError::WrongRuntime);
        }
        let shape = self.shape(from)?;
        if !self.key_is_current(self.key(from)?) {
            return Err(ShapeError::Stale);
        }
        if shape.slots.contains_key(&symbol) {
            return Ok(from);
        }
        if let Some(existing) = self.transitions.get(&(from, symbol)) {
            return Ok(*existing);
        }
        let class = shape.class;
        let dependency_epoch = shape.dependency_epoch;
        let layout_epoch = shape.layout_epoch.wrapping_add(1);
        let mut slots = shape.slots.clone();
        let slot = u32::try_from(slots.len()).map_err(|_| ShapeError::Capacity)?;
        slots.insert(symbol, slot);
        let next = self.insert_shape(class, slots, dependency_epoch, layout_epoch);
        self.transitions.insert((from, symbol), next);
        Ok(next)
    }

    pub(crate) fn transition_for_class(
        &mut self,
        class: ClassId,
        from: ShapeId,
        symbol: SymbolId,
    ) -> Result<ShapeId, ShapeError> {
        self.validate_class(class)?;
        if self.shape(from)?.class != class {
            return Err(ShapeError::WrongClass);
        }
        self.transition(from, symbol)
    }

    pub(crate) fn slot(&self, shape: ShapeId, symbol: SymbolId) -> Result<u32, ShapeError> {
        if symbol.namespace() != self.namespace {
            return Err(ShapeError::WrongRuntime);
        }
        self.shape(shape)?
            .slots
            .get(&symbol)
            .copied()
            .ok_or(ShapeError::UnknownField)
    }

    pub(crate) fn key(&self, id: ShapeId) -> Result<ShapeKey, ShapeError> {
        let shape = self.shape(id)?;
        Ok(ShapeKey {
            shape: shape.id,
            dependency_epoch: shape.dependency_epoch,
            layout_epoch: shape.layout_epoch,
        })
    }

    pub(crate) fn key_is_current(&self, key: ShapeKey) -> bool {
        self.shape(key.shape).is_ok_and(|shape| {
            let class_index = usize::try_from(shape.class.index).ok();
            class_index
                .and_then(|index| self.class_epochs.get(index))
                .is_some_and(|epoch| *epoch == key.dependency_epoch)
                && shape.layout_epoch == key.layout_epoch
        })
    }

    pub(crate) fn serialized_key_is_current(
        &self,
        identity: u64,
        dependency_epoch: u64,
        layout_epoch: u64,
    ) -> bool {
        u32::try_from(identity).ok().is_some_and(|index| {
            self.key_is_current(ShapeKey {
                shape: ShapeId {
                    namespace: self.namespace,
                    index,
                },
                dependency_epoch,
                layout_epoch,
            })
        })
    }

    pub(crate) fn invalidate_class(&mut self, class: ClassId) -> Result<(), ShapeError> {
        self.validate_class(class)?;
        let index = usize::try_from(class.index).map_err(|_| ShapeError::UnknownClass)?;
        let next_epoch = {
            let epoch = self
                .class_epochs
                .get_mut(index)
                .ok_or(ShapeError::UnknownClass)?;
            *epoch = epoch.wrapping_add(1);
            *epoch
        };
        let root = self.insert_shape(class, HashMap::new(), next_epoch, 0);
        self.roots.insert(class, root);
        Ok(())
    }

    fn validate_class(&self, class: ClassId) -> Result<(), ShapeError> {
        if class.namespace != self.namespace {
            return Err(ShapeError::WrongRuntime);
        }
        let index = usize::try_from(class.index).map_err(|_| ShapeError::UnknownClass)?;
        self.class_epochs
            .get(index)
            .map(|_| ())
            .ok_or(ShapeError::UnknownClass)
    }

    fn insert_shape(
        &mut self,
        class: ClassId,
        slots: HashMap<SymbolId, u32>,
        dependency_epoch: u64,
        layout_epoch: u64,
    ) -> ShapeId {
        let id = ShapeId {
            namespace: self.namespace,
            index: u32::try_from(self.shapes.len()).unwrap_or(u32::MAX),
        };
        self.shapes.push(Shape {
            id,
            class,
            slots,
            dependency_epoch,
            layout_epoch,
        });
        id
    }
}

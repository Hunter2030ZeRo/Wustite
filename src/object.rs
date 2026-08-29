mod heap;
mod sequence;

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use num_bigint::BigInt;

use crate::executable::ExecutableFunction;
use crate::value::Value;

pub use heap::{ObjectError, ObjectHeap};
pub use sequence::{SequenceObject, SequenceStrategy};

static NEXT_CLASS_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    heap_id: u64,
    slot: u32,
    generation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClassId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShapeId(pub u64);

#[derive(Clone)]
pub struct ClassObject {
    id: ClassId,
    name: String,
    methods: Vec<(String, ExecutableFunction)>,
}

impl ClassObject {
    pub fn new(name: String, methods: Vec<(String, ExecutableFunction)>) -> Self {
        let id = NEXT_CLASS_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            id: ClassId(id),
            name,
            methods,
        }
    }

    pub const fn id(&self) -> ClassId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn method(&self, name: &str) -> Option<&ExecutableFunction> {
        self.methods
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, function)| function)
    }
}

#[derive(Clone)]
pub struct InstanceObject {
    class: ObjectRef,
    class_id: ClassId,
    shape: ShapeId,
    fields: Vec<(String, Value)>,
}

impl InstanceObject {
    pub const fn class(&self) -> ObjectRef {
        self.class
    }

    pub const fn shape(&self) -> ShapeId {
        self.shape
    }

    pub(crate) fn fields(&self) -> &[(String, Value)] {
        &self.fields
    }
}

#[derive(Clone)]
pub struct BoundMethodObject {
    receiver: ObjectRef,
    function: ExecutableFunction,
}

impl BoundMethodObject {
    pub const fn receiver(&self) -> ObjectRef {
        self.receiver
    }

    pub fn function(&self) -> &ExecutableFunction {
        &self.function
    }
}

impl ObjectRef {
    pub(crate) const fn new(heap_id: u64, slot: u32, generation: u32) -> Self {
        Self {
            heap_id,
            slot,
            generation,
        }
    }

    pub const fn heap_id(self) -> u64 {
        self.heap_id
    }

    pub const fn slot(self) -> u32 {
        self.slot
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone)]
#[allow(
    clippy::large_enum_variant,
    reason = "boxing functions would add indirection to the VM's uniform object representation"
)]
pub enum Object {
    String(String),
    Tuple(SequenceObject),
    BigInt(BigInt),
    List(SequenceObject),
    Dict(Vec<(Value, Value)>),
    Function(ExecutableFunction),
    Class(ClassObject),
    Instance(InstanceObject),
    BoundMethod(BoundMethodObject),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    String,
    Tuple,
    BigInt,
    List,
    Dict,
    Function,
    Class,
    Instance,
    BoundMethod,
}

impl Object {
    pub fn tuple(values: Vec<Value>) -> Self {
        Self::Tuple(SequenceObject::from_values(values))
    }

    pub fn list(values: Vec<Value>) -> Self {
        Self::List(SequenceObject::from_values(values))
    }

    pub const fn kind(&self) -> ObjectKind {
        match self {
            Self::String(_) => ObjectKind::String,
            Self::Tuple(_) => ObjectKind::Tuple,
            Self::BigInt(_) => ObjectKind::BigInt,
            Self::List(_) => ObjectKind::List,
            Self::Dict(_) => ObjectKind::Dict,
            Self::Function(_) => ObjectKind::Function,
            Self::Class(_) => ObjectKind::Class,
            Self::Instance(_) => ObjectKind::Instance,
            Self::BoundMethod(_) => ObjectKind::BoundMethod,
        }
    }
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(lhs), Self::String(rhs)) => lhs == rhs,
            (Self::Tuple(lhs), Self::Tuple(rhs)) => lhs == rhs,
            (Self::BigInt(lhs), Self::BigInt(rhs)) => lhs == rhs,
            (Self::List(lhs), Self::List(rhs)) => lhs == rhs,
            (Self::Dict(lhs), Self::Dict(rhs)) => lhs == rhs,
            (Self::Function(lhs), Self::Function(rhs)) => lhs.id() == rhs.id(),
            (Self::Class(lhs), Self::Class(rhs)) => lhs.id() == rhs.id(),
            (Self::Instance(_), Self::Instance(_)) => false,
            (Self::BoundMethod(lhs), Self::BoundMethod(rhs)) => {
                lhs.receiver == rhs.receiver && lhs.function.id() == rhs.function.id()
            }
            (
                Self::String(_)
                | Self::Tuple(_)
                | Self::BigInt(_)
                | Self::List(_)
                | Self::Dict(_)
                | Self::Function(_)
                | Self::Class(_)
                | Self::Instance(_)
                | Self::BoundMethod(_),
                _,
            ) => false,
        }
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => formatter.debug_tuple("String").field(value).finish(),
            Self::Tuple(values) => formatter.debug_tuple("Tuple").field(values).finish(),
            Self::BigInt(value) => formatter.debug_tuple("BigInt").field(value).finish(),
            Self::List(values) => formatter.debug_tuple("List").field(values).finish(),
            Self::Dict(entries) => formatter.debug_tuple("Dict").field(entries).finish(),
            Self::Function(function) => formatter
                .debug_struct("Function")
                .field("executable_id", &function.id())
                .finish(),
            Self::Class(class) => formatter
                .debug_struct("Class")
                .field("id", &class.id)
                .field("name", &class.name)
                .finish(),
            Self::Instance(instance) => formatter
                .debug_struct("Instance")
                .field("class", &instance.class)
                .field("shape", &instance.shape)
                .finish(),
            Self::BoundMethod(method) => formatter
                .debug_struct("BoundMethod")
                .field("receiver", &method.receiver)
                .field("executable_id", &method.function.id())
                .finish(),
        }
    }
}

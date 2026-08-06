mod heap;

use std::fmt;

use num_bigint::BigInt;

use crate::executable::ExecutableFunction;
use crate::value::Value;

pub use heap::{ObjectError, ObjectHeap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    heap_id: u64,
    slot: u32,
    generation: u32,
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
pub enum Object {
    String(String),
    Tuple(Vec<Value>),
    BigInt(BigInt),
    List(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Function(ExecutableFunction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    String,
    Tuple,
    BigInt,
    List,
    Dict,
    Function,
}

impl Object {
    pub const fn kind(&self) -> ObjectKind {
        match self {
            Self::String(_) => ObjectKind::String,
            Self::Tuple(_) => ObjectKind::Tuple,
            Self::BigInt(_) => ObjectKind::BigInt,
            Self::List(_) => ObjectKind::List,
            Self::Dict(_) => ObjectKind::Dict,
            Self::Function(_) => ObjectKind::Function,
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
            (
                Self::String(_)
                | Self::Tuple(_)
                | Self::BigInt(_)
                | Self::List(_)
                | Self::Dict(_)
                | Self::Function(_),
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
        }
    }
}

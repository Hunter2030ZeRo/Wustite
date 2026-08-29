use std::fmt;

use crate::adaptive_v2::handles::{HandleError, StableHandle};
use crate::adaptive_v2::heap::{GcError, GcHeap};
use crate::adaptive_v2::lists::ListError;
use crate::adaptive_v2::objects::ObjectError;
use crate::adaptive_v2::symbols::SymbolError;
use crate::adaptive_v2::value_word::ValueWord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PayloadKind {
    Object,
    List,
    Callable,
}

impl fmt::Display for PayloadKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Object => formatter.write_str("object"),
            Self::List => formatter.write_str("list"),
            Self::Callable => formatter.write_str("callable"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeapAdapterError {
    Heap(GcError),
    Object(ObjectError),
    List(ListError),
    Symbol(SymbolError),
    MissingPayload(PayloadKind),
    WrongRuntime,
    StaleHandle,
    ExpectedHandle,
    ExpectedInteger,
}

impl fmt::Display for HeapAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Heap(error) => error.fmt(formatter),
            Self::Object(error) => error.fmt(formatter),
            Self::List(error) => error.fmt(formatter),
            Self::Symbol(error) => error.fmt(formatter),
            Self::MissingPayload(kind) => write!(formatter, "adaptive {kind} payload is missing"),
            Self::WrongRuntime => formatter.write_str("adaptive value belongs to another runtime"),
            Self::StaleHandle => formatter.write_str("adaptive value handle is stale"),
            Self::ExpectedHandle => formatter.write_str("adaptive operation expected a handle"),
            Self::ExpectedInteger => {
                formatter.write_str("adaptive call expected integer arguments")
            }
        }
    }
}

impl std::error::Error for HeapAdapterError {}

impl From<GcError> for HeapAdapterError {
    fn from(error: GcError) -> Self {
        match error {
            GcError::InvalidHandle(HandleError::WrongRuntime) => Self::WrongRuntime,
            GcError::InvalidHandle(HandleError::Stale) => Self::StaleHandle,
            other => Self::Heap(other),
        }
    }
}

impl From<ObjectError> for HeapAdapterError {
    fn from(error: ObjectError) -> Self {
        match error {
            ObjectError::Heap(heap) => Self::from(heap),
            ObjectError::WrongRuntime => Self::WrongRuntime,
            other => Self::Object(other),
        }
    }
}

impl From<ListError> for HeapAdapterError {
    fn from(error: ListError) -> Self {
        match error {
            ListError::Heap(heap) => Self::from(heap),
            other => Self::List(other),
        }
    }
}

impl From<SymbolError> for HeapAdapterError {
    fn from(error: SymbolError) -> Self {
        match error {
            SymbolError::WrongRuntime => Self::WrongRuntime,
            other => Self::Symbol(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeapValue {
    word: ValueWord,
    handle: Option<StableHandle>,
}

impl HeapValue {
    pub(super) const fn immediate(word: ValueWord) -> Self {
        Self { word, handle: None }
    }

    pub(super) fn from_handle(handle: StableHandle) -> Self {
        Self {
            word: ValueWord::from_handle(handle),
            handle: Some(handle),
        }
    }

    pub(crate) const fn word(self) -> ValueWord {
        self.word
    }

    pub(crate) const fn handle(self) -> Option<StableHandle> {
        self.handle
    }

    pub(super) fn validate(self, heap: &GcHeap) -> Result<(), HeapAdapterError> {
        if let Some(handle) = self.handle {
            heap.resolve(handle)?;
            if self.word.as_handle(heap) != Some(handle) {
                return Err(HeapAdapterError::WrongRuntime);
            }
        }
        Ok(())
    }
}

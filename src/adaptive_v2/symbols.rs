use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeNamespace(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SymbolId {
    namespace: RuntimeNamespace,
    index: u32,
}

impl SymbolId {
    pub(crate) const fn namespace(self) -> RuntimeNamespace {
        self.namespace
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymbolError {
    WrongRuntime,
    Unknown,
    Capacity,
}

impl fmt::Display for SymbolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongRuntime => formatter.write_str("symbol belongs to another runtime"),
            Self::Unknown => formatter.write_str("symbol is unknown"),
            Self::Capacity => formatter.write_str("symbol table capacity exhausted"),
        }
    }
}

impl std::error::Error for SymbolError {}

#[derive(Debug)]
pub(crate) struct SymbolTable {
    namespace: RuntimeNamespace,
    ids: HashMap<Arc<str>, SymbolId>,
    names: Vec<Arc<str>>,
}

impl SymbolTable {
    pub(crate) fn new() -> Self {
        Self {
            namespace: next_namespace(),
            ids: HashMap::new(),
            names: Vec::new(),
        }
    }

    pub(crate) const fn namespace(&self) -> RuntimeNamespace {
        self.namespace
    }

    pub(crate) fn intern(&mut self, name: &str) -> Result<SymbolId, SymbolError> {
        if let Some(symbol) = self.ids.get(name) {
            return Ok(*symbol);
        }
        let owned: Arc<str> = Arc::from(name);
        let index = u32::try_from(self.names.len()).map_err(|_| SymbolError::Capacity)?;
        let symbol = SymbolId {
            namespace: self.namespace,
            index,
        };
        self.names.push(Arc::clone(&owned));
        self.ids.insert(owned, symbol);
        Ok(symbol)
    }

    pub(crate) fn resolve(&self, symbol: SymbolId) -> Result<&str, SymbolError> {
        if symbol.namespace != self.namespace {
            return Err(SymbolError::WrongRuntime);
        }
        let index = usize::try_from(symbol.index).map_err(|_| SymbolError::Unknown)?;
        self.names
            .get(index)
            .map(AsRef::as_ref)
            .ok_or(SymbolError::Unknown)
    }
}

fn next_namespace() -> RuntimeNamespace {
    static NEXT: OnceLock<Mutex<u64>> = OnceLock::new();
    let mut next = NEXT
        .get_or_init(|| Mutex::new(1))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let namespace = RuntimeNamespace(*next);
    *next = next.wrapping_add(1);
    namespace
}

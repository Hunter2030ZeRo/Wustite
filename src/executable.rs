use std::sync::atomic::{AtomicU64, Ordering};

use crate::bytecode::Function;
use crate::structure_map::StructureMap;

static NEXT_EXECUTABLE_ID: AtomicU64 = AtomicU64::new(1);

/// Process-local identity for one immutable executable revision.
///
/// This value is only suitable as an in-memory runtime cache key. It is not a
/// persistent or cross-process identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExecutableId(u64);

#[derive(Clone)]
pub struct ExecutableFunction {
    id: ExecutableId,
    bytecode: Function,
    structure_map: StructureMap,
}

impl ExecutableFunction {
    /// Creates a new immutable executable revision with a fresh process-local ID.
    pub fn new(bytecode: Function, structure_map: StructureMap) -> Self {
        let id = NEXT_EXECUTABLE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .unwrap_or_else(|_| panic!("process-local executable ID space exhausted"));

        Self {
            id: ExecutableId(id),
            bytecode,
            structure_map,
        }
    }

    pub fn id(&self) -> ExecutableId {
        self.id
    }

    pub fn bytecode(&self) -> &Function {
        &self.bytecode
    }

    pub fn structure_map(&self) -> &StructureMap {
        &self.structure_map
    }
}

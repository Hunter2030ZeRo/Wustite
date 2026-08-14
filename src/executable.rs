use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::bytecode::{Function, Register};
use crate::structure_map::{SlotType, StructureMap};

static NEXT_EXECUTABLE_ID: AtomicU64 = AtomicU64::new(1);

/// Process-local identity for one immutable executable revision.
///
/// This value is only suitable as an in-memory runtime cache key. It is not a
/// persistent or cross-process identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExecutableId(u64);

impl ExecutableId {
    pub(crate) const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantId(pub usize);

#[derive(Clone)]
pub enum ExecutableConstant {
    String(String),
    BigInt(num_bigint::BigInt),
    Function(Box<ExecutableFunction>),
}

/// One positional host-to-WVM argument mapping in the execution ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableParameter {
    pub name: String,
    pub register: Register,
    pub ty: SlotType,
}

#[derive(Clone)]
pub struct ExecutableFunction {
    id: ExecutableId,
    bytecode: Function,
    structure_map: StructureMap,
    parameters: Vec<ExecutableParameter>,
    constants: Vec<ExecutableConstant>,
    verification: Arc<OnceLock<Result<(), String>>>,
}

impl ExecutableFunction {
    /// Creates a new immutable executable revision with a fresh process-local ID.
    pub fn new(bytecode: Function, structure_map: StructureMap) -> Self {
        Self::new_with_abi(bytecode, structure_map, Vec::new(), Vec::new())
    }

    /// Creates an executable whose positional parameters define its host ABI.
    pub fn new_with_parameters(
        bytecode: Function,
        structure_map: StructureMap,
        parameters: Vec<ExecutableParameter>,
    ) -> Self {
        Self::new_with_abi(bytecode, structure_map, parameters, Vec::new())
    }

    /// Creates an executable with a host ABI and immutable constant pool.
    pub fn new_with_abi(
        bytecode: Function,
        structure_map: StructureMap,
        parameters: Vec<ExecutableParameter>,
        constants: Vec<ExecutableConstant>,
    ) -> Self {
        let id = NEXT_EXECUTABLE_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .unwrap_or_else(|_| panic!("process-local executable ID space exhausted"));

        Self {
            id: ExecutableId(id),
            bytecode,
            structure_map,
            parameters,
            constants,
            verification: Arc::new(OnceLock::new()),
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

    pub fn parameters(&self) -> &[ExecutableParameter] {
        &self.parameters
    }

    pub fn constants(&self) -> &[ExecutableConstant] {
        &self.constants
    }

    pub(crate) fn verification_cache(&self) -> &OnceLock<Result<(), String>> {
        &self.verification
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_id_is_clone_stable_and_revision_unique() {
        let executable = ExecutableFunction::new(
            Function {
                code: Vec::new(),
                register_count: 0,
            },
            StructureMap::default(),
        );
        let clone = executable.clone();
        let revision = ExecutableFunction::new(
            Function {
                code: Vec::new(),
                register_count: 0,
            },
            StructureMap::default(),
        );

        assert_eq!(executable.id().as_u64(), clone.id().as_u64());
        assert_ne!(executable.id().as_u64(), revision.id().as_u64());
    }
}

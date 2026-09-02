mod call;
mod operations;
mod site;
mod snapshot;
mod transfer;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::adaptive_v2::native::{AdaptiveNativeContext, NativeValue};
use crate::adaptive_v2::profile::AdaptiveProfile;
use crate::adaptive_v2::public_heap::runtime::AdaptiveHeapRuntime;
use crate::adaptive_v2::wxir_v2::ir::SnapshotDraft;
use crate::bytecode::Instruction;
use crate::executable::ExecutableFunction;
use crate::jit::CompilerBackend;
use crate::object::{ObjectHeap, ObjectRef};
use crate::value::Value;

use super::SharedTier1Code;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum SiteOperation {
    ObjectGet,
    ListGet,
    DirectCall,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SiteKey {
    executable: u64,
    pc: u32,
    operation: SiteOperation,
}

enum Operation {
    ObjectGet {
        receiver: ObjectRef,
        field: String,
        dst: u16,
    },
    ObjectSet {
        receiver: ObjectRef,
        field: String,
        value: i64,
    },
    ListGet {
        list: ObjectRef,
        index: usize,
        dst: u16,
    },
    ListAppend {
        list: ObjectRef,
        value: i64,
    },
    ListSet {
        list: ObjectRef,
        index: i64,
        value: i64,
    },
    ListInsert {
        list: ObjectRef,
        index: i64,
        value: i64,
    },
    ListPop {
        list: ObjectRef,
        index: i64,
        dst: u16,
    },
    ListLength {
        list: ObjectRef,
        dst: u16,
    },
    DirectCall {
        receiver: ObjectRef,
        callee: u64,
        method: call::NumericMethod,
        argument: i64,
        dst: u16,
    },
}

pub(crate) struct ObjectTicket {
    output: Option<(u16, Value)>,
    handled: bool,
}

impl ObjectTicket {
    pub(crate) const fn output(&self) -> Option<(u16, Value)> {
        self.output
    }

    pub(crate) const fn handled(&self) -> bool {
        self.handled
    }
}

struct SiteState {
    profile: AdaptiveProfile,
    classification: super::observation::StaticClassification,
    draft: Option<SnapshotDraft>,
    native: Option<Arc<SharedTier1Code>>,
    cache_bytes: u64,
}

pub(crate) struct ObjectSites {
    states: Mutex<HashMap<SiteKey, SiteState>>,
}

impl ObjectSites {
    pub(crate) fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }
}

pub(crate) struct ObjectOsr {
    context: AdaptiveNativeContext,
    bindings: HashMap<ObjectRef, transfer::Binding>,
    sites: Arc<ObjectSites>,
    backend: Option<CompilerBackend>,
    hot_threshold: u64,
}

impl ObjectOsr {
    pub(crate) fn new(
        heap: AdaptiveHeapRuntime,
        backend: Option<CompilerBackend>,
        sites: Arc<ObjectSites>,
        hot_threshold: u64,
    ) -> Self {
        Self {
            context: AdaptiveNativeContext::with_runtime(heap),
            bindings: HashMap::new(),
            sites,
            backend,
            hot_threshold,
        }
    }

    pub(crate) fn root_binding(
        &self,
        reference: ObjectRef,
    ) -> Option<crate::adaptive_v2::public_heap::runtime::RootedValue> {
        self.bindings
            .get(&reference)
            .and_then(|binding| self.context.rooted_value(binding.native).ok())
    }

    pub(crate) fn entry_inputs(
        &mut self,
        executable: &ExecutableFunction,
        arguments: &[Value],
        heap: &mut ObjectHeap,
    ) -> Result<Vec<NativeValue>, String> {
        for (pc, instruction) in executable.bytecode().code.iter().enumerate() {
            let key = u32::try_from(pc).map_err(|_| "adaptive entry pc overflow".to_owned())?;
            match instruction {
                Instruction::GetAttr { name, .. } | Instruction::SetAttr { name, .. } => {
                    self.context.bind_field(i64::from(key), name);
                }
                _ => {}
            }
        }
        arguments
            .iter()
            .map(|argument| match argument {
                Value::SmallInt(value) => Ok(NativeValue::Integer(*value)),
                Value::Float(value) => Ok(NativeValue::FloatBits(value.to_bits())),
                Value::Bool(value) => Ok(NativeValue::Boolean(*value)),
                Value::Object(reference) => self.ensure_binding(*reference, heap),
                Value::None | Value::Uninitialized => {
                    Err("adaptive-v2 entry type changed".to_owned())
                }
            })
            .collect()
    }

    pub(crate) fn finish_entry(
        &mut self,
        arguments: &[Value],
        heap: &mut ObjectHeap,
    ) -> Result<(), String> {
        for argument in arguments {
            if let Value::Object(reference) = argument {
                transfer::hand_back_entry(&mut self.context, &mut self.bindings, *reference, heap)?;
            }
        }
        Ok(())
    }

    fn ensure_binding(
        &mut self,
        reference: ObjectRef,
        heap: &mut ObjectHeap,
    ) -> Result<NativeValue, String> {
        transfer::ensure(&mut self.context, &mut self.bindings, reference, heap)
    }

    fn hand_back(
        &mut self,
        reference: ObjectRef,
        operation: &Operation,
        heap: &mut ObjectHeap,
    ) -> Result<(), String> {
        transfer::hand_back(
            &mut self.context,
            &mut self.bindings,
            reference,
            operation,
            heap,
        )
    }
}

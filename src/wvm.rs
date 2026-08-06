use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::bytecode::Register;
use crate::executable::{ExecutableFunction, ExecutableId};
use crate::object::{Object, ObjectError, ObjectHeap, ObjectKind, ObjectRef};
use crate::profiler::Profile;
use crate::structure_map::RegionId;
use crate::value::Value;
use crate::verifier::verify;
use crate::wxir::WxExitKind;

use self::quickening::QuickCode;

mod arguments;
mod arithmetic;
mod callables;
mod equality;
mod interpreter;
mod jit_runtime;
mod objects;
mod quickening;
mod registers;

/// Default observed region entries before synchronous tier-up.
pub const DEFAULT_HOT_THRESHOLD: u64 = 1_000;

const MAX_GUEST_CALL_DEPTH: usize = 128;

pub(super) struct Frame {
    pc: usize,
    registers: Vec<Value>,
    suppress_osr_pc: Option<usize>,
    suppressed_regions: HashSet<RegionId>,
}

pub struct Vm {
    hot_threshold: u64,
    jit_report: JitReport,
    runtimes: HashMap<ExecutableId, FunctionRuntime>,
    last_executed: Option<ExecutableId>,
    object_heap: ObjectHeap,
    call_depth: usize,
}

pub(super) struct FunctionRuntime {
    profile: Profile,
    jit: jit_runtime::JitRuntime,
    constants: Vec<Option<Value>>,
    current_function: Option<Value>,
    quick_code: Arc<QuickCode>,
}

impl FunctionRuntime {
    fn new(executable: &ExecutableFunction) -> Self {
        Self::with_quick_code(executable, Arc::new(QuickCode::new(executable)))
    }

    fn recursive_placeholder(executable: &ExecutableFunction, quick_code: Arc<QuickCode>) -> Self {
        Self::with_quick_code(executable, quick_code)
    }

    fn with_quick_code(executable: &ExecutableFunction, quick_code: Arc<QuickCode>) -> Self {
        Self {
            profile: Profile::new(executable.structure_map().loops.len()),
            jit: jit_runtime::JitRuntime::new(executable),
            constants: vec![None; executable.constants().len()],
            current_function: None,
            quick_code,
        }
    }
}

/// Stage at which one region was disabled for the current execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitFailureStage {
    BuildWxir,
    Compile,
    Execute,
}

impl JitFailureStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BuildWxir => "build_wxir",
            Self::Compile => "compile",
            Self::Execute => "execute",
        }
    }
}

/// Preserved diagnostic for a failed synchronous tier-up operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitFailure {
    pub region_id: RegionId,
    pub stage: JitFailureStage,
    pub reason: String,
}

/// Observable tier-up activity for the latest execute invocation, including failures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitReport {
    pub compilation_attempts: u64,
    pub compiled_regions: u64,
    pub disabled_regions: u64,
    pub native_executions: u64,
    pub last_resume_pc: Option<usize>,
    pub last_exit_kind: Option<WxExitKind>,
    pub failures: Vec<JitFailure>,
}

impl JitReport {
    pub const fn last_exit_kind_name(&self) -> Option<&'static str> {
        match self.last_exit_kind {
            Some(WxExitKind::RegionExit) => Some("region_exit"),
            Some(WxExitKind::ReplayInstruction) => Some("replay_instruction"),
            Some(WxExitKind::Deopt) => Some("deopt"),
            None => None,
        }
    }
}

pub struct ExecutionResult {
    pub value: Value,
}

impl Vm {
    pub fn new() -> Self {
        Self::with_hot_threshold(DEFAULT_HOT_THRESHOLD)
    }

    /// Creates a VM that tiers up after this many observed region entries.
    pub fn with_hot_threshold(hot_threshold: u64) -> Self {
        Self {
            hot_threshold,
            jit_report: JitReport::default(),
            runtimes: HashMap::new(),
            last_executed: None,
            object_heap: ObjectHeap::new(),
            call_depth: 0,
        }
    }

    pub fn set_hot_threshold(&mut self, hot_threshold: u64) {
        self.hot_threshold = hot_threshold;
    }

    pub fn profile(&self) -> Option<&Profile> {
        self.last_executed
            .and_then(|id| self.runtimes.get(&id))
            .map(|runtime| &runtime.profile)
    }

    pub fn profile_for(&self, executable: &ExecutableFunction) -> Option<&Profile> {
        self.runtimes
            .get(&executable.id())
            .map(|runtime| &runtime.profile)
    }

    pub fn jit_report(&self) -> &JitReport {
        &self.jit_report
    }

    pub fn allocate_object(&mut self, object: Object) -> Result<ObjectRef, ObjectError> {
        self.object_heap.allocate(object)
    }

    pub fn object(&self, reference: ObjectRef) -> Result<&Object, ObjectError> {
        self.object_heap.get(reference)
    }

    pub fn object_kind(&self, reference: ObjectRef) -> Result<ObjectKind, ObjectError> {
        self.object_heap.kind(reference)
    }

    pub fn execute(&mut self, executable: &ExecutableFunction) -> Result<ExecutionResult, String> {
        self.execute_with_args(executable, &[])
    }

    pub fn execute_with_args(
        &mut self,
        executable: &ExecutableFunction,
        function_arguments: &[Value],
    ) -> Result<ExecutionResult, String> {
        self.jit_report = JitReport::default();
        self.execute_function(executable, function_arguments)
    }

    pub(super) fn execute_function(
        &mut self,
        executable: &ExecutableFunction,
        function_arguments: &[Value],
    ) -> Result<ExecutionResult, String> {
        if self.call_depth >= MAX_GUEST_CALL_DEPTH {
            return Err(format!(
                "guest call depth limit of {MAX_GUEST_CALL_DEPTH} exceeded"
            ));
        }
        self.call_depth += 1;
        let result = self.execute_function_at_depth(executable, function_arguments);
        self.call_depth -= 1;
        result
    }

    fn execute_function_at_depth(
        &mut self,
        executable: &ExecutableFunction,
        function_arguments: &[Value],
    ) -> Result<ExecutionResult, String> {
        verify(executable)?;
        let registers =
            arguments::initialize_registers(executable, function_arguments, &self.object_heap)?;
        let id = executable.id();
        let mut runtime = self
            .runtimes
            .remove(&id)
            .unwrap_or_else(|| FunctionRuntime::new(executable));
        let result = self.execute_with_runtime(executable, &mut runtime, registers);
        self.runtimes.insert(id, runtime);
        self.last_executed = Some(id);
        result
    }

    #[inline]
    pub(super) fn read_register(frame: &Frame, register: Register) -> Result<Value, String> {
        frame
            .registers
            .get(usize::from(register))
            .copied()
            .ok_or_else(|| format!("invalid register r{register}"))
    }

    pub(super) fn write_register(
        frame: &mut Frame,
        register: Register,
        value: Value,
    ) -> Result<(), String> {
        let slot = frame
            .registers
            .get_mut(usize::from(register))
            .ok_or_else(|| format!("invalid register r{register}"))?;
        *slot = value;
        Ok(())
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "wvm/tests/runtime_identity.rs"]
mod tests;

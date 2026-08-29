use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::adaptive_v2::integration::AdaptiveVm;
use crate::bytecode::Register;
use crate::executable::{ExecutableFunction, ExecutableId};
use crate::jit::CompilerBackend;
use crate::object::{Object, ObjectError, ObjectHeap, ObjectKind, ObjectRef};
use crate::planner::JitPolicy;
use crate::profiler::{Profile, ProfileArtifact, RegionProfileSchema};
use crate::structure_map::RegionId;
use crate::value::Value;
use crate::verifier::verify;

use self::quickening::QuickCode;

mod arguments;
mod arithmetic;
mod callables;
mod config;
mod equality;
mod interpreter;
mod jit_report;
mod jit_runtime;
mod native_jit;
mod objects;
mod quickening;
mod registers;
mod runtime_dispatch;

pub use self::jit_report::{
    JitCallSites, JitExits, JitFailure, JitFailureStage, JitGuestCalls, JitHelperCalls, JitReport,
    JitRuntimeOps,
};
pub(crate) use self::native_jit::{match_reverse_prefix, temporary_is_dead};

/// Default observed region entries before synchronous tier-up.
pub const DEFAULT_HOT_THRESHOLD: u64 = 1_000;
pub const DEFAULT_TIER2_THRESHOLD: u64 = 10;

const MAX_GUEST_CALL_DEPTH: usize = 128;
static NEXT_EXECUTION_ID: AtomicU64 = AtomicU64::new(1);

fn next_execution_id() -> u64 {
    NEXT_EXECUTION_ID.fetch_add(1, Ordering::Relaxed)
}

pub(super) struct Frame {
    pc: usize,
    registers: Vec<Value>,
    suppress_osr_pc: Option<usize>,
    suppressed_regions: HashSet<RegionId>,
}

pub struct Vm {
    hot_threshold: u64,
    tier2_threshold: u64,
    compiler_backend: Option<CompilerBackend>,
    jit_report: JitReport,
    jit_policy: JitPolicy,
    dump_wxir: bool,
    runtimes: HashMap<ExecutableId, FunctionRuntime>,
    last_executed: Option<ExecutableId>,
    object_heap: ObjectHeap,
    call_depth: usize,
    frame_pool: HashMap<usize, Vec<Frame>>,
    verified_functions: HashSet<ExecutableId>,
    adaptive_v2: Option<Arc<AdaptiveVm>>,
    adaptive_execution_id: u64,
    last_adaptive_report: Option<crate::runtime::AdaptiveReport>,
    defer_adaptive_report_sync: bool,
}

pub(super) struct FunctionRuntime {
    profile: Profile,
    profile_schemas: Vec<RegionProfileSchema>,
    jit: Option<jit_runtime::JitRuntime>,
    constants: Vec<Option<Value>>,
    current_function: Option<Value>,
    quick_code: Arc<QuickCode>,
    leaf_calls: Vec<Option<native_jit::leaf::PreparedLeafCall>>,
    call_sites: Vec<callables::PreparedCallSite>,
}

impl FunctionRuntime {
    #[cfg(test)]
    fn new(executable: &ExecutableFunction) -> Self {
        Self::with_compiler_backend(executable, Some(CompilerBackend::Tiered))
    }

    fn with_compiler_backend(
        executable: &ExecutableFunction,
        compiler_backend: Option<CompilerBackend>,
    ) -> Self {
        Self::with_quick_code(
            executable,
            Arc::new(QuickCode::new(executable)),
            compiler_backend,
        )
    }

    fn recursive_placeholder(executable: &ExecutableFunction, quick_code: Arc<QuickCode>) -> Self {
        Self::with_quick_code(executable, quick_code, None)
    }

    fn with_quick_code(
        executable: &ExecutableFunction,
        quick_code: Arc<QuickCode>,
        compiler_backend: Option<CompilerBackend>,
    ) -> Self {
        Self {
            profile: Profile::new(
                executable.structure_map().regions().len(),
                executable.bytecode().code.len(),
            ),
            profile_schemas: executable
                .structure_map()
                .regions()
                .iter()
                .enumerate()
                .filter_map(|(index, _)| {
                    RegionProfileSchema::from_structure_map(
                        executable.structure_map(),
                        RegionId(index),
                    )
                })
                .collect(),
            jit: compiler_backend.map(|backend| jit_runtime::JitRuntime::new(executable, backend)),
            constants: vec![None; executable.constants().len()],
            current_function: None,
            quick_code,
            leaf_calls: (0..executable.bytecode().code.len())
                .map(|_| None)
                .collect(),
            call_sites: (0..executable.bytecode().code.len())
                .map(|_| callables::PreparedCallSite::default())
                .collect(),
        }
    }
}

pub struct ExecutionResult {
    pub value: Value,
}

impl Vm {
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

    pub fn seed_profile(
        &mut self,
        executable: &ExecutableFunction,
        artifact: &ProfileArtifact,
        fingerprint: &str,
    ) -> Result<(), String> {
        let runtime = self.runtimes.entry(executable.id()).or_insert_with(|| {
            FunctionRuntime::with_compiler_backend(executable, self.compiler_backend)
        });
        runtime.profile.seed_from_artifact(artifact, fingerprint)
    }

    pub fn jit_report(&self) -> &JitReport {
        &self.jit_report
    }

    pub fn adaptive_report(&self) -> Option<&crate::runtime::AdaptiveReport> {
        self.last_adaptive_report.as_ref()
    }

    pub(crate) const fn adaptive_execution_id(&self) -> u64 {
        self.adaptive_execution_id
    }

    fn sync_adaptive_report(&mut self) {
        if self.call_depth != 0 || self.defer_adaptive_report_sync {
            return;
        }
        let report = self.adaptive_v2.as_ref().map(|adaptive| adaptive.report());
        if report.is_some() {
            self.last_adaptive_report = report;
        }
    }

    pub(crate) fn begin_adaptive_report_batch(&mut self) {
        self.sync_adaptive_report();
        self.defer_adaptive_report_sync = true;
    }

    pub(crate) fn end_adaptive_report_batch(&mut self) {
        self.defer_adaptive_report_sync = false;
        self.sync_adaptive_report();
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
        let result = self.execute_function_at_depth(executable, function_arguments, false);
        self.call_depth -= 1;
        self.sync_adaptive_report();
        result
    }

    pub(super) fn execute_prepared_function(
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
        self.jit_report.call_sites.prepared_call_hit = self
            .jit_report
            .call_sites
            .prepared_call_hit
            .saturating_add(1);
        let result = self.execute_function_at_depth(executable, function_arguments, true);
        self.call_depth -= 1;
        self.sync_adaptive_report();
        result
    }

    fn execute_function_at_depth(
        &mut self,
        executable: &ExecutableFunction,
        function_arguments: &[Value],
        prepared: bool,
    ) -> Result<ExecutionResult, String> {
        self.jit_report.record_function_call(executable.name());
        let id = executable.id();
        if !prepared || !self.verified_functions.contains(&id) {
            verify(executable)?;
            self.verified_functions.insert(id);
        }
        let register_count = executable.bytecode().register_count;
        let mut frame = self
            .frame_pool
            .get_mut(&register_count)
            .and_then(Vec::pop)
            .unwrap_or_else(|| Frame {
                pc: 0,
                registers: vec![Value::Uninitialized; register_count],
                suppress_osr_pc: None,
                suppressed_regions: HashSet::new(),
            });
        frame.pc = 0;
        frame.suppress_osr_pc = None;
        frame.suppressed_regions.clear();
        if let Err(error) = arguments::initialize_registers(
            executable,
            function_arguments,
            &self.object_heap,
            &mut frame.registers,
        ) {
            frame.registers.fill(Value::Uninitialized);
            self.frame_pool
                .entry(register_count)
                .or_default()
                .push(frame);
            return Err(error);
        }
        let adaptive_result = self.adaptive_v2.as_ref().and_then(|adaptive| {
            adaptive.try_execute_entry(
                self.adaptive_execution_id,
                executable,
                function_arguments,
                &mut self.object_heap,
            )
        });
        if let Some(result) = adaptive_result {
            frame.registers.fill(Value::Uninitialized);
            self.frame_pool
                .entry(register_count)
                .or_default()
                .push(frame);
            self.last_executed = Some(id);
            return result.map(|value| ExecutionResult { value });
        }
        let compiler_backend = self.compiler_backend;
        let mut runtime = self.runtimes.remove(&id).unwrap_or_else(|| {
            FunctionRuntime::with_compiler_backend(executable, compiler_backend)
        });
        let result = self.execute_with_runtime(executable, &mut runtime, &mut frame);
        self.runtimes.insert(id, runtime);
        frame.registers.fill(Value::Uninitialized);
        self.frame_pool
            .entry(register_count)
            .or_default()
            .push(frame);
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

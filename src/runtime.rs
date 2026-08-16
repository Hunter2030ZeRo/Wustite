use std::error::Error;
use std::fmt;
use std::time::Instant;

use crate::executable::{ExecutableFunction, ExecutableId, ExecutableParameter};
use crate::frontend::{PythonFrontendError, compile_python_function};
use crate::jit::CompilerBackend;
use crate::metrics::{CompilationMetrics, ExecutionMetrics};
use crate::object::{Object, ObjectError, ObjectKind, ObjectRef};
use crate::profiler::Profile;
use crate::structure_map::{RegionId, RegionKind, StateSlot};
use crate::value::Value;
use crate::wvm::{DEFAULT_HOT_THRESHOLD, JitReport, Vm};

mod value;

pub use value::RuntimeValue;

/// Selects interpreter-only execution or a specific native compilation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Interpreter,
    AdaptiveJit,
    Jit(CompilerBackend),
}

/// Configuration fixed for one long-lived [`Runtime`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub execution_mode: ExecutionMode,
    pub hot_threshold: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            execution_mode: ExecutionMode::Jit(CompilerBackend::Tiered),
            hot_threshold: DEFAULT_HOT_THRESHOLD,
        }
    }
}

/// Structured errors at the frontend, verification, execution, and result boundary.
#[derive(Debug)]
pub enum RuntimeError {
    Frontend(PythonFrontendError),
    Object(ObjectError),
    Verification(String),
    Execution(String),
    InvalidResult(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frontend(error) => write!(formatter, "frontend error: {error}"),
            Self::Object(error) => write!(formatter, "object error: {error}"),
            Self::Verification(error) => write!(formatter, "verification error: {error}"),
            Self::Execution(error) => write!(formatter, "execution error: {error}"),
            Self::InvalidResult(error) => write!(formatter, "invalid runtime result: {error}"),
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Frontend(error) => Some(error),
            Self::Object(error) => Some(error),
            _ => None,
        }
    }
}

impl From<PythonFrontendError> for RuntimeError {
    fn from(error: PythonFrontendError) -> Self {
        Self::Frontend(error)
    }
}

impl From<ObjectError> for RuntimeError {
    fn from(error: ObjectError) -> Self {
        Self::Object(error)
    }
}

/// Read-only metadata for one WVM loop region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionInfo {
    pub id: RegionId,
    pub header: usize,
    pub backedge: usize,
    pub exits: Vec<usize>,
    pub live_slots: Vec<StateSlot>,
}

/// Read-only metadata for an immutable executable revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableInfo {
    pub id: ExecutableId,
    pub register_count: usize,
    pub instruction_count: usize,
    pub parameters: Vec<ExecutableParameter>,
    pub regions: Vec<RegionInfo>,
}

/// Reusable, long-lived execution engine for CLI and embedded hosts.
///
/// CLI and other hosts should use this facade instead of constructing a [`Vm`]
/// directly.
///
/// Retain an [`ExecutableFunction`] and call [`Runtime::execute`] repeatedly to
/// reuse its profile and compiled regions. This runtime is not yet a persistent
/// Python session: globals, imports, modules, and notebook-cell state are future
/// work.
pub struct Runtime {
    vm: Vm,
    config: RuntimeConfig,
}

impl Runtime {
    pub fn new(config: RuntimeConfig) -> Self {
        let vm = match config.execution_mode {
            ExecutionMode::Interpreter => Vm::interpreter(),
            ExecutionMode::AdaptiveJit => Vm::with_compiler_backend(
                config.hot_threshold,
                crate::wvm::DEFAULT_TIER2_THRESHOLD,
                CompilerBackend::Tiered,
            ),
            ExecutionMode::Jit(backend) => Vm::with_compiler_backend(
                config.hot_threshold,
                crate::wvm::DEFAULT_TIER2_THRESHOLD,
                backend,
            ),
        };
        Self { vm, config }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Allocates an object owned by this runtime's VM heap.
    pub fn allocate_object(&mut self, object: Object) -> Result<ObjectRef, RuntimeError> {
        self.vm
            .allocate_object(object)
            .map_err(RuntimeError::Object)
    }

    /// Looks up a live object owned by this runtime's VM heap.
    pub fn object(&self, reference: ObjectRef) -> Result<&Object, RuntimeError> {
        self.vm.object(reference).map_err(RuntimeError::Object)
    }

    /// Returns the kind of a live object owned by this runtime's VM heap.
    pub fn object_kind(&self, reference: ObjectRef) -> Result<ObjectKind, RuntimeError> {
        self.vm.object_kind(reference).map_err(RuntimeError::Object)
    }

    pub fn compile_function(
        &mut self,
        source: &str,
        function_name: &str,
    ) -> Result<ExecutableFunction, RuntimeError> {
        compile_python_function(source, function_name).map_err(RuntimeError::Frontend)
    }

    pub fn compile_function_measured(
        &mut self,
        source: &str,
        function_name: &str,
    ) -> Result<MeasuredCompilation, RuntimeError> {
        let started = Instant::now();
        let executable = self.compile_function(source, function_name)?;
        let frontend_time = started.elapsed();

        Ok(MeasuredCompilation {
            executable,
            metrics: CompilationMetrics { frontend_time },
        })
    }

    pub fn execute(
        &mut self,
        executable: &ExecutableFunction,
    ) -> Result<RuntimeValue, RuntimeError> {
        self.execute_with_args(executable, &[])
    }

    pub fn execute_with_args(
        &mut self,
        executable: &ExecutableFunction,
        arguments: &[RuntimeValue],
    ) -> Result<RuntimeValue, RuntimeError> {
        // TODO: Split verification from dispatch errors once Vm has a typed error.
        let arguments = arguments
            .iter()
            .copied()
            .map(Value::from)
            .collect::<Vec<_>>();
        let result = self
            .vm
            .execute_with_args(executable, &arguments)
            .map_err(RuntimeError::Execution)?;
        RuntimeValue::try_from(result.value)
    }

    pub fn execute_measured(
        &mut self,
        executable: &ExecutableFunction,
    ) -> Result<MeasuredExecution, RuntimeError> {
        self.execute_measured_with_args(executable, &[])
    }

    pub fn execute_measured_with_args(
        &mut self,
        executable: &ExecutableFunction,
        arguments: &[RuntimeValue],
    ) -> Result<MeasuredExecution, RuntimeError> {
        let started = Instant::now();

        let value = self.execute_with_args(executable, arguments)?;

        let total_time = started.elapsed();
        let jit_report = self.last_jit_report().clone();

        Ok(MeasuredExecution {
            value,
            metrics: ExecutionMetrics { total_time },
            jit_report,
        })
    }

    /// Convenience compile-and-run API.
    ///
    /// Each call creates a fresh executable revision. For repeated JIT reuse,
    /// retain the value returned by [`Runtime::compile_function`] and pass it to
    /// [`Runtime::execute`].
    pub fn run_function(
        &mut self,
        source: &str,
        function_name: &str,
    ) -> Result<RuntimeValue, RuntimeError> {
        self.run_function_with_args(source, function_name, &[])
    }

    pub fn run_function_with_args(
        &mut self,
        source: &str,
        function_name: &str,
        arguments: &[RuntimeValue],
    ) -> Result<RuntimeValue, RuntimeError> {
        let executable = self.compile_function(source, function_name)?;
        self.execute_with_args(&executable, arguments)
    }

    pub fn inspect(&self, executable: &ExecutableFunction) -> ExecutableInfo {
        let bytecode = executable.bytecode();
        let regions = executable
            .structure_map()
            .regions()
            .iter()
            .enumerate()
            .filter_map(|(index, region)| {
                let RegionKind::Loop { backedge } = region.kind else {
                    return None;
                };
                Some(RegionInfo {
                    id: RegionId(index),
                    header: region.entry,
                    backedge,
                    exits: region.exits.iter().map(|exit| exit.target).collect(),
                    live_slots: region.entry_summary.clone(),
                })
            })
            .collect();

        ExecutableInfo {
            id: executable.id(),
            register_count: bytecode.register_count,
            instruction_count: bytecode.code.len(),
            parameters: executable.parameters().to_vec(),
            regions,
        }
    }

    pub fn last_jit_report(&self) -> &JitReport {
        self.vm.jit_report()
    }

    pub fn profile_for(&self, executable: &ExecutableFunction) -> Option<&Profile> {
        self.vm.profile_for(executable)
    }
}

pub struct MeasuredCompilation {
    pub executable: ExecutableFunction,
    pub metrics: CompilationMetrics,
}

#[derive(Debug, Clone)]
pub struct MeasuredExecution {
    pub value: RuntimeValue,
    pub metrics: ExecutionMetrics,
    pub jit_report: JitReport,
}

use std::error::Error;
use std::fmt;

use crate::executable::{ExecutableFunction, ExecutableId};
use crate::frontend::{PythonFrontendError, compile_python_function};
use crate::profiler::Profile;
use crate::structure_map::{LiveSlot, RegionId};
use crate::value::Value;
use crate::wvm::{DEFAULT_HOT_THRESHOLD, JitReport, Vm};

/// Selects interpreter-only execution or adaptive synchronous tier-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Interpreter,
    AdaptiveJit,
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
            execution_mode: ExecutionMode::AdaptiveJit,
            hot_threshold: DEFAULT_HOT_THRESHOLD,
        }
    }
}

/// Stable public values returned by the embeddable runtime facade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeValue {
    I64(i64),
    Bool(bool),
}

impl TryFrom<Value> for RuntimeValue {
    type Error = RuntimeError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::I64(value) => Ok(Self::I64(value)),
            Value::Bool(value) => Ok(Self::Bool(value)),
            Value::Uninitialized => Err(RuntimeError::InvalidResult(
                "WVM returned an uninitialized value".to_string(),
            )),
        }
    }
}

/// Structured errors at the frontend, verification, execution, and result boundary.
#[derive(Debug)]
pub enum RuntimeError {
    Frontend(PythonFrontendError),
    Verification(String),
    Execution(String),
    InvalidResult(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frontend(error) => write!(formatter, "frontend error: {error}"),
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
            _ => None,
        }
    }
}

impl From<PythonFrontendError> for RuntimeError {
    fn from(error: PythonFrontendError) -> Self {
        Self::Frontend(error)
    }
}

/// Read-only metadata for one WVM loop region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionInfo {
    pub id: RegionId,
    pub header: usize,
    pub backedge: usize,
    pub exits: Vec<usize>,
    pub live_slots: Vec<LiveSlot>,
}

/// Read-only metadata for an immutable executable revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableInfo {
    pub id: ExecutableId,
    pub register_count: usize,
    pub instruction_count: usize,
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
        let threshold = match config.execution_mode {
            ExecutionMode::Interpreter => u64::MAX,
            ExecutionMode::AdaptiveJit => config.hot_threshold,
        };
        Self {
            vm: Vm::with_hot_threshold(threshold),
            config,
        }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    pub fn compile_function(
        &mut self,
        source: &str,
        function_name: &str,
    ) -> Result<ExecutableFunction, RuntimeError> {
        compile_python_function(source, function_name).map_err(RuntimeError::Frontend)
    }

    pub fn execute(
        &mut self,
        executable: &ExecutableFunction,
    ) -> Result<RuntimeValue, RuntimeError> {
        // TODO: Split verification from dispatch errors once Vm has a typed error.
        let result = self
            .vm
            .execute(executable)
            .map_err(RuntimeError::Execution)?;
        RuntimeValue::try_from(result.value)
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
        let executable = self.compile_function(source, function_name)?;
        self.execute(&executable)
    }

    pub fn inspect(&self, executable: &ExecutableFunction) -> ExecutableInfo {
        let bytecode = executable.bytecode();
        let regions = executable
            .structure_map()
            .loops
            .iter()
            .enumerate()
            .map(|(index, region)| RegionInfo {
                id: RegionId(index),
                header: region.header,
                backedge: region.backedge,
                exits: region.exits.iter().map(|exit| exit.target).collect(),
                live_slots: region.live_slots.clone(),
            })
            .collect();

        ExecutableInfo {
            id: executable.id(),
            register_count: bytecode.register_count,
            instruction_count: bytecode.code.len(),
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

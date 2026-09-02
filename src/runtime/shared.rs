use std::sync::{Arc, Mutex};

use crate::adaptive_v2::integration::AdaptiveVm;
use crate::adaptive_v2::public_heap::runtime::RootedValue;
use crate::executable::ExecutableFunction;
use crate::jit::CompilerBackend;
use crate::object::Object;

use super::{AdaptiveReport, Runtime, RuntimeConfig, RuntimeError, RuntimeValue};

#[derive(Clone)]
pub struct SharedRuntime {
    inner: Arc<SharedState>,
}

#[derive(Clone)]
pub struct RootedResult {
    owner: Arc<SharedState>,
    value: RuntimeValue,
    _adaptive_root: Option<RootedValue>,
    compatibility_runtime: Arc<Mutex<SharedExecutionRuntime>>,
}

struct SharedState {
    config: RuntimeConfig,
    adaptive: Arc<AdaptiveVm>,
}

struct SharedExecutionRuntime(Runtime);

// SAFETY: this wrapper is only constructed with `Runtime::with_shared_adaptive_v2`, whose local
// WVM has no legacy compiler backend or native-code owner. A Mutex serializes access to its
// compatibility object heap; compiled adaptive code remains in the separately synchronized core.
unsafe impl Send for SharedExecutionRuntime {}

impl SharedRuntime {
    pub fn new_adaptive_v2(config: RuntimeConfig) -> Self {
        let backend = match config.execution_mode {
            super::ExecutionMode::Interpreter => None,
            super::ExecutionMode::AdaptiveJit => Some(CompilerBackend::Tiered),
            super::ExecutionMode::Jit(backend) => Some(backend),
        };
        let adaptive = Arc::new(AdaptiveVm::new(backend, config.hot_threshold));
        Self {
            inner: Arc::new(SharedState { config, adaptive }),
        }
    }

    pub fn compile_function(
        &self,
        source: &str,
        function_name: &str,
    ) -> Result<ExecutableFunction, RuntimeError> {
        crate::frontend::compile_python_function(source, function_name).map_err(RuntimeError::from)
    }

    pub fn execute_rooted(
        &self,
        executable: &ExecutableFunction,
        arguments: &[RuntimeValue],
    ) -> Result<RootedResult, RuntimeError> {
        let compatibility_runtime = Arc::new(Mutex::new(SharedExecutionRuntime(
            Runtime::with_shared_adaptive_v2(
                self.inner.config.clone(),
                Arc::clone(&self.inner.adaptive),
            ),
        )));
        let (value, execution_id) = {
            let mut runtime = compatibility_runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let value = runtime.0.execute_with_args(executable, arguments)?;
            (value, runtime.0.adaptive_execution_id())
        };
        let adaptive_root = self
            .inner
            .adaptive
            .root_public_value(execution_id, value)
            .map_err(RuntimeError::Execution)?;
        Ok(RootedResult {
            owner: Arc::clone(&self.inner),
            value,
            _adaptive_root: adaptive_root,
            compatibility_runtime,
        })
    }

    pub fn resolve_rooted(&self, rooted: &RootedResult) -> Result<RuntimeValue, RuntimeError> {
        if !Arc::ptr_eq(&self.inner, &rooted.owner) {
            return Err(RuntimeError::InvalidResult(
                "rooted result belongs to another runtime".to_owned(),
            ));
        }
        if let Some(root) = &rooted._adaptive_root {
            self.inner
                .adaptive
                .validate_public_root(root)
                .map_err(RuntimeError::InvalidResult)?;
        }
        if let RuntimeValue::Object(reference) = rooted.value {
            rooted
                .compatibility_runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0
                .object(reference)
                .map_err(|error| RuntimeError::InvalidResult(error.to_string()))?;
        }
        Ok(rooted.value)
    }

    pub fn adaptive_report(&self) -> Result<Option<AdaptiveReport>, RuntimeError> {
        Ok(Some(self.inner.adaptive.report()))
    }

    pub fn collect_garbage(&self) -> Result<(), RuntimeError> {
        self.inner
            .adaptive
            .collect_public_heap()
            .map_err(RuntimeError::Execution)
    }
}

impl RootedResult {
    pub fn value(&self) -> RuntimeValue {
        self.value
    }

    pub fn release(self) -> RuntimeValue {
        self.value
    }

    pub fn object(&self) -> Result<Object, RuntimeError> {
        let RuntimeValue::Object(reference) = self.value else {
            return Err(RuntimeError::InvalidResult(
                "rooted result is not an object".to_owned(),
            ));
        };
        self.compatibility_runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0
            .object(reference)
            .cloned()
    }
}

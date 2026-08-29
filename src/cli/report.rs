use std::fmt;

use serde::Serialize;
use wustite::object::ObjectRef;
use wustite::{AdaptiveReport, ExecutionMode, Runtime, RuntimeValue};

use super::profile_cache::ProfileCacheStatus;
use super::value_names::object_kind_name;

mod jit;

pub(super) use self::jit::{JitOutput, print_jit_debug, print_jit_trace};

#[derive(Serialize)]
pub(super) struct RunDocument {
    path: String,
    function: String,
    execution_mode: &'static str,
    compiler_backend: Option<&'static str>,
    hot_threshold: u64,
    profile_cache: ProfileCacheStatus,
    runs: Vec<RunOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adaptive_v2: Option<AdaptiveReport>,
}

pub(super) struct RunContext {
    pub(super) path: String,
    pub(super) function: String,
    pub(super) execution_mode: ExecutionMode,
    pub(super) hot_threshold: u64,
    pub(super) profile_cache: ProfileCacheStatus,
    pub(super) adaptive_v2: Option<AdaptiveReport>,
}

impl RunDocument {
    pub(super) fn new(context: RunContext, runs: Vec<RunOutput>) -> Self {
        let (execution_mode, compiler_backend) = match context.execution_mode {
            ExecutionMode::Interpreter => ("interpreter", None),
            ExecutionMode::AdaptiveJit => ("adaptive_jit", Some("tiered")),
            ExecutionMode::Jit(backend) => ("adaptive_jit", Some(backend.as_str())),
        };
        Self {
            path: context.path,
            function: context.function,
            execution_mode,
            compiler_backend,
            hot_threshold: context.hot_threshold,
            profile_cache: context.profile_cache,
            runs,
            adaptive_v2: context.adaptive_v2,
        }
    }
}

#[derive(Serialize)]
pub(super) struct RunOutput {
    index: usize,
    value: OutputValue,
    jit: JitOutput,
}

impl RunOutput {
    pub(super) fn snapshot(
        index: usize,
        value: RuntimeValue,
        runtime: &Runtime,
    ) -> Result<Self, String> {
        Ok(Self {
            index,
            value: OutputValue::snapshot(value, runtime)?,
            jit: JitOutput::snapshot(runtime.last_jit_report()),
        })
    }
}

#[derive(Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum OutputValue {
    SmallInt(i64),
    Float(f64),
    Bool(bool),
    None,
    Object(ObjectOutput),
}

impl OutputValue {
    fn snapshot(value: RuntimeValue, runtime: &Runtime) -> Result<Self, String> {
        match value {
            RuntimeValue::SmallInt(value) => Ok(Self::SmallInt(value)),
            RuntimeValue::Float(value) => Ok(Self::Float(value)),
            RuntimeValue::Bool(value) => Ok(Self::Bool(value)),
            RuntimeValue::None => Ok(Self::None),
            RuntimeValue::Object(reference) => {
                Ok(Self::Object(ObjectOutput::snapshot(reference, runtime)?))
            }
        }
    }
}

impl fmt::Display for OutputValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SmallInt(value) => value.fmt(formatter),
            Self::Float(value) => value.fmt(formatter),
            Self::Bool(value) => value.fmt(formatter),
            Self::None => formatter.write_str("None"),
            Self::Object(value) => value.fmt(formatter),
        }
    }
}

#[derive(Serialize)]
struct ObjectOutput {
    kind: &'static str,
    heap_id: u64,
    slot: u32,
    generation: u32,
}

impl ObjectOutput {
    fn snapshot(reference: ObjectRef, runtime: &Runtime) -> Result<Self, String> {
        let kind = runtime
            .object_kind(reference)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            kind: object_kind_name(kind),
            heap_id: reference.heap_id(),
            slot: reference.slot(),
            generation: reference.generation(),
        })
    }
}

impl fmt::Display for ObjectOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "object(kind={}, heap_id={}, slot={}, generation={})",
            self.kind, self.heap_id, self.slot, self.generation
        )
    }
}

pub(super) fn print_run_values(runs: &[RunOutput]) {
    if let [run] = runs {
        println!("{}", run.value);
    } else {
        for run in runs {
            println!("run {}: {}", run.index, run.value);
        }
    }
}

pub(super) fn print_json(value: &impl Serialize) -> Result<(), String> {
    let document = serde_json::to_string_pretty(value)
        .map_err(|error| format!("failed to serialize JSON output: {error}"))?;
    println!("{document}");
    Ok(())
}

use std::fmt;

use serde::Serialize;
use wustite::object::ObjectRef;
use wustite::wvm::JitReport;
use wustite::{ExecutionMode, Runtime, RuntimeValue};

use super::value_names::object_kind_name;

#[derive(Serialize)]
pub(super) struct RunDocument {
    path: String,
    function: String,
    execution_mode: &'static str,
    compiler_backend: Option<&'static str>,
    hot_threshold: u64,
    runs: Vec<RunOutput>,
}

pub(super) struct RunContext {
    pub(super) path: String,
    pub(super) function: String,
    pub(super) execution_mode: ExecutionMode,
    pub(super) hot_threshold: u64,
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
            runs,
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
    Object(ObjectOutput),
}

impl OutputValue {
    fn snapshot(value: RuntimeValue, runtime: &Runtime) -> Result<Self, String> {
        match value {
            RuntimeValue::SmallInt(value) => Ok(Self::SmallInt(value)),
            RuntimeValue::Float(value) => Ok(Self::Float(value)),
            RuntimeValue::Bool(value) => Ok(Self::Bool(value)),
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

#[derive(Serialize)]
struct JitOutput {
    compilation_attempts: u64,
    compiled_regions: u64,
    tier2_compilation_attempts: u64,
    tier2_compiled_regions: u64,
    disabled_regions: u64,
    native_executions: u64,
    tier2_native_executions: u64,
    last_resume_pc: Option<usize>,
    last_exit_kind: Option<String>,
    failures: Vec<JitFailureOutput>,
}

impl JitOutput {
    fn snapshot(report: &JitReport) -> Self {
        Self {
            compilation_attempts: report.compilation_attempts,
            compiled_regions: report.compiled_regions,
            tier2_compilation_attempts: report.tier2_compilation_attempts,
            tier2_compiled_regions: report.tier2_compiled_regions,
            disabled_regions: report.disabled_regions,
            native_executions: report.native_executions,
            tier2_native_executions: report.tier2_native_executions,
            last_resume_pc: report.last_resume_pc,
            last_exit_kind: report.last_exit_kind_name().map(str::to_string),
            failures: report
                .failures
                .iter()
                .map(|failure| JitFailureOutput {
                    region_id: failure.region_id.0,
                    stage: failure.stage.as_str(),
                    reason: failure.reason.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct JitFailureOutput {
    region_id: usize,
    stage: &'static str,
    reason: String,
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

pub(super) fn print_jit_trace(runs: &[RunOutput]) {
    for run in runs {
        let jit = &run.jit;
        let mut line = format!(
            "run {}: compilation_attempts={} compiled_regions={} tier2_compilation_attempts={} tier2_compiled_regions={} disabled_regions={} native_executions={} tier2_native_executions={}",
            run.index,
            jit.compilation_attempts,
            jit.compiled_regions,
            jit.tier2_compilation_attempts,
            jit.tier2_compiled_regions,
            jit.disabled_regions,
            jit.native_executions,
            jit.tier2_native_executions
        );
        if let Some(resume_pc) = jit.last_resume_pc {
            line.push_str(&format!(" last_resume_pc={resume_pc}"));
        }
        if let Some(exit_kind) = &jit.last_exit_kind {
            line.push_str(&format!(" last_exit_kind={exit_kind}"));
        }
        eprintln!("{line}");
        for failure in &jit.failures {
            eprintln!(
                "run {}: failure region={} stage={} reason={}",
                run.index, failure.region_id, failure.stage, failure.reason
            );
        }
    }
}

pub(super) fn print_json(value: &impl Serialize) -> Result<(), String> {
    let document = serde_json::to_string_pretty(value)
        .map_err(|error| format!("failed to serialize JSON output: {error}"))?;
    println!("{document}");
    Ok(())
}

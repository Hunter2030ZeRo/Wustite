//! Thin command-line host for the embeddable Wustite Runtime API.

use std::fmt;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use wustite::structure_map::SlotType;
use wustite::wvm::JitReport;
use wustite::{ExecutableInfo, ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

#[derive(Parser)]
#[command(
    name = "wustite",
    version,
    about = "Execute the supported Python subset with Wustite"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile and execute a named zero-argument function.
    Run(RunArgs),
    /// Compile and inspect WVM metadata without executing it.
    Inspect(InspectArgs),
}

#[derive(Args)]
struct RunArgs {
    /// Python source file to execute.
    path: PathBuf,

    /// Zero-argument function to execute.
    #[arg(long, default_value = "main")]
    function: String,

    /// Number of executions. Repeated runs print `run N: VALUE`.
    #[arg(long, default_value_t = NonZeroUsize::MIN)]
    repeat: NonZeroUsize,

    /// Disable adaptive native tier-up.
    #[arg(long)]
    interpreter: bool,

    /// Region-entry threshold for adaptive JIT compilation.
    #[arg(long, default_value_t = RuntimeConfig::default().hot_threshold)]
    hot_threshold: u64,

    /// Print per-run JIT diagnostics to stderr.
    #[arg(long)]
    trace_jit: bool,

    /// Emit one JSON document to stdout.
    #[arg(long, conflicts_with = "trace_jit")]
    json: bool,
}

#[derive(Args)]
struct InspectArgs {
    /// Python source file to inspect.
    path: PathBuf,

    /// Zero-argument function to inspect.
    #[arg(long, default_value = "main")]
    function: String,

    /// Emit one JSON document to stdout.
    #[arg(long)]
    json: bool,
}

pub(crate) fn main_entry() -> ExitCode {
    match dispatch(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Run(args) => run(args),
        Command::Inspect(args) => inspect(args),
    }
}

fn run(args: RunArgs) -> Result<(), String> {
    let source = read_source(&args.path)?;
    let execution_mode = if args.interpreter {
        ExecutionMode::Interpreter
    } else {
        ExecutionMode::AdaptiveJit
    };
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode,
        hot_threshold: args.hot_threshold,
    });
    let executable = runtime
        .compile_function(&source, &args.function)
        .map_err(|error| error.to_string())?;

    let mut runs = Vec::with_capacity(args.repeat.get());
    for index in 1..=args.repeat.get() {
        let value = runtime
            .execute(&executable)
            .map(OutputValue::from)
            .map_err(|error| error.to_string())?;
        let jit = JitOutput::snapshot(runtime.last_jit_report());
        runs.push(RunOutput { index, value, jit });
    }

    if args.json {
        print_json(&RunDocument {
            path: args.path.display().to_string(),
            function: args.function,
            execution_mode: execution_mode_name(execution_mode),
            hot_threshold: args.hot_threshold,
            runs,
        })?;
    } else {
        print_run_values(&runs);
        if args.trace_jit {
            print_jit_trace(&runs);
        }
    }

    Ok(())
}

fn inspect(args: InspectArgs) -> Result<(), String> {
    let source = read_source(&args.path)?;
    let mut runtime = Runtime::new(RuntimeConfig::default());
    let executable = runtime
        .compile_function(&source, &args.function)
        .map_err(|error| error.to_string())?;
    let info = runtime.inspect(&executable);

    if args.json {
        print_json(&InspectDocument::new(
            args.path.display().to_string(),
            args.function,
            info,
        ))?;
    } else {
        print_inspection(&args.function, &info);
    }

    Ok(())
}

fn read_source(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))
}

fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Interpreter => "interpreter",
        ExecutionMode::AdaptiveJit => "adaptive_jit",
    }
}

#[derive(Serialize)]
struct RunDocument {
    path: String,
    function: String,
    execution_mode: &'static str,
    hot_threshold: u64,
    runs: Vec<RunOutput>,
}

#[derive(Serialize)]
struct RunOutput {
    index: usize,
    value: OutputValue,
    jit: JitOutput,
}

#[derive(Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum OutputValue {
    I64(i64),
    Bool(bool),
}

impl From<RuntimeValue> for OutputValue {
    fn from(value: RuntimeValue) -> Self {
        match value {
            RuntimeValue::I64(value) => Self::I64(value),
            RuntimeValue::Bool(value) => Self::Bool(value),
        }
    }
}

impl fmt::Display for OutputValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::I64(value) => value.fmt(formatter),
            Self::Bool(value) => value.fmt(formatter),
        }
    }
}

#[derive(Serialize)]
struct JitOutput {
    compilation_attempts: u64,
    compiled_regions: u64,
    disabled_regions: u64,
    native_executions: u64,
    last_resume_pc: Option<usize>,
    last_exit_kind: Option<String>,
    failures: Vec<JitFailureOutput>,
}

impl JitOutput {
    fn snapshot(report: &JitReport) -> Self {
        Self {
            compilation_attempts: report.compilation_attempts,
            compiled_regions: report.compiled_regions,
            disabled_regions: report.disabled_regions,
            native_executions: report.native_executions,
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

fn print_run_values(runs: &[RunOutput]) {
    if let [run] = runs {
        println!("{}", run.value);
    } else {
        for run in runs {
            println!("run {}: {}", run.index, run.value);
        }
    }
}

fn print_jit_trace(runs: &[RunOutput]) {
    for run in runs {
        let jit = &run.jit;
        let mut line = format!(
            "run {}: compilation_attempts={} compiled_regions={} disabled_regions={} native_executions={}",
            run.index,
            jit.compilation_attempts,
            jit.compiled_regions,
            jit.disabled_regions,
            jit.native_executions
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

#[derive(Serialize)]
struct InspectDocument {
    path: String,
    function: String,
    register_count: usize,
    instruction_count: usize,
    regions: Vec<RegionOutput>,
}

impl InspectDocument {
    fn new(path: String, function: String, info: ExecutableInfo) -> Self {
        Self {
            path,
            function,
            register_count: info.register_count,
            instruction_count: info.instruction_count,
            regions: info
                .regions
                .into_iter()
                .map(|region| RegionOutput {
                    id: region.id.0,
                    header: region.header,
                    backedge: region.backedge,
                    exits: region.exits,
                    live_slots: region
                        .live_slots
                        .into_iter()
                        .map(|slot| LiveSlotOutput {
                            register: slot.register,
                            ty: slot_type_name(slot.ty),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct RegionOutput {
    id: usize,
    header: usize,
    backedge: usize,
    exits: Vec<usize>,
    live_slots: Vec<LiveSlotOutput>,
}

#[derive(Serialize)]
struct LiveSlotOutput {
    register: u16,
    #[serde(rename = "type")]
    ty: &'static str,
}

fn slot_type_name(ty: SlotType) -> &'static str {
    match ty {
        SlotType::I64 => "i64",
        SlotType::Bool => "bool",
    }
}

fn print_inspection(function: &str, info: &ExecutableInfo) {
    println!("Function: {function}");
    println!("Registers: {}", info.register_count);
    println!("Instructions: {}", info.instruction_count);
    println!("Regions: {}", info.regions.len());

    for region in &info.regions {
        println!();
        println!("Region {}", region.id.0);
        println!("  Header: {}", region.header);
        println!("  Backedge: {}", region.backedge);
        let exits = region
            .exits
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  Exits: {}",
            if exits.is_empty() { "none" } else { &exits }
        );
        println!("  Live slots:");
        if region.live_slots.is_empty() {
            println!("    none");
        } else {
            for slot in &region.live_slots {
                println!("    r{}: {}", slot.register, slot_type_name(slot.ty));
            }
        }
    }
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    let document = serde_json::to_string_pretty(value)
        .map_err(|error| format!("failed to serialize JSON output: {error}"))?;
    println!("{document}");
    Ok(())
}

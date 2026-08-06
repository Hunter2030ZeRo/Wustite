//! Thin command-line host for the embeddable Wustite Runtime API.

use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use wustite::{ExecutionMode, Runtime, RuntimeConfig};

mod arguments;
mod benchmark;
mod inspection;
mod report;
mod value_names;

use self::arguments::parse_arguments;
use self::inspection::{InspectDocument, print_inspection};
use self::report::{RunContext, RunDocument, RunOutput, print_jit_trace, print_run_values};

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
    /// Compile and execute a named function.
    Run(RunArgs),
    /// Compile and inspect WVM metadata without executing it.
    Inspect(InspectArgs),
    /// Compares interpreter and adaptive JIT execution.
    Bench(BenchArgs),
}

#[derive(Args)]
struct RunArgs {
    /// Python source file to execute.
    path: PathBuf,

    /// Function to execute.
    #[arg(long, default_value = "main")]
    function: String,

    /// Positional value parsed according to the parameter annotation. Repeat for multiple arguments.
    #[arg(long = "arg", value_name = "VALUE", allow_hyphen_values = true)]
    arguments: Vec<String>,

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

    /// Function to inspect.
    #[arg(long, default_value = "main")]
    function: String,

    /// Emit one JSON document to stdout.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub(super) struct BenchArgs {
    /// Python source file to benchmark.
    pub(super) path: PathBuf,

    /// Function to execute.
    #[arg(long, default_value = "main")]
    pub(super) function: String,

    /// Positional value parsed according to the parameter annotation. Repeat for multiple arguments.
    #[arg(long = "arg", value_name = "VALUE", allow_hyphen_values = true)]
    pub(super) arguments: Vec<String>,

    /// Unmeasured stabilization runs before collecting warm samples.
    #[arg(long, default_value_t = 10)]
    pub(super) warmup: usize,

    /// Number of measured samples for each steady-state mode.
    #[arg(long, default_value = "100")]
    pub(super) iterations: NonZeroUsize,

    /// Region-entry threshold for adaptive JIT compilation.
    #[arg(long, default_value_t = 10)]
    pub(super) hot_threshold: u64,
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
        Command::Bench(args) => benchmark::run(args),
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
    let function_arguments =
        parse_arguments(&mut runtime, executable.parameters(), &args.arguments)?;

    let mut runs = Vec::with_capacity(args.repeat.get());
    for index in 1..=args.repeat.get() {
        let value = runtime
            .execute_with_args(&executable, &function_arguments)
            .map_err(|error| error.to_string())?;
        runs.push(RunOutput::snapshot(index, value, &runtime)?);
    }

    if args.json {
        report::print_json(&RunDocument::new(
            RunContext {
                path: args.path.display().to_string(),
                function: args.function,
                execution_mode,
                hot_threshold: args.hot_threshold,
            },
            runs,
        ))?;
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
        report::print_json(&InspectDocument::new(
            args.path.display().to_string(),
            args.function,
            info,
        ))?;
    } else {
        print_inspection(&args.function, &info);
    }
    Ok(())
}

pub(super) fn read_source(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))
}

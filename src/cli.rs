//! Thin command-line host for the embeddable Wustite Runtime API.

use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use wustite::{AdaptiveReport, CompilerBackend, ExecutionMode, JitPolicy, Runtime, RuntimeConfig};

mod arguments;
mod benchmark;
mod inspection;
mod profile_cache;
mod report;
mod value_names;

use self::arguments::parse_arguments;
use self::inspection::{InspectDocument, print_inspection};
use self::profile_cache::ProfileCache;
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

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(super) enum BackendArg {
    Cranelift,
    #[cfg(feature = "inkwell")]
    Llvm,
    Tiered,
}

impl From<BackendArg> for CompilerBackend {
    fn from(backend: BackendArg) -> Self {
        match backend {
            BackendArg::Cranelift => Self::Cranelift,
            #[cfg(feature = "inkwell")]
            BackendArg::Llvm => Self::Llvm,
            BackendArg::Tiered => Self::Tiered,
        }
    }
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
    #[arg(long, conflicts_with = "backend")]
    interpreter: bool,

    /// Native compiler policy. Tiered uses Cranelift before LLVM when available.
    #[arg(long, value_enum, default_value_t = BackendArg::Tiered)]
    backend: BackendArg,

    /// Region-entry threshold for adaptive JIT compilation.
    #[arg(long, default_value_t = RuntimeConfig::default().hot_threshold)]
    hot_threshold: u64,

    /// Print detailed JIT decisions and failures to stderr.
    #[arg(long, visible_aliases = ["debug", "trace-jit"])]
    debug_jit: bool,

    /// Print each compiled WXIR function to stderr.
    #[arg(long, conflicts_with = "interpreter")]
    dump_wxir: bool,

    /// Emit one JSON document to stdout.
    #[arg(long, conflicts_with = "debug_jit")]
    json: bool,

    /// JIT planning policy. Both policies require a hot, runtime-validated profile.
    #[arg(long, value_enum, default_value_t = JitPolicyArg::StructureMap)]
    jit_policy: JitPolicyArg,

    /// Runtime core. The legacy core remains the default until performance qualification passes.
    #[arg(long, value_enum, default_value_t = RuntimeCoreArg::AdaptiveV2)]
    runtime_core: RuntimeCoreArg,
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

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum JitPolicyArg {
    Profile,
    StructureMap,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(super) enum RuntimeCoreArg {
    Legacy,
    AdaptiveV2,
}

impl std::fmt::Display for RuntimeCoreArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Legacy => "legacy core 1.0",
            Self::AdaptiveV2 => "core 2.0",
        })
    }
}

impl From<JitPolicyArg> for JitPolicy {
    fn from(policy: JitPolicyArg) -> Self {
        match policy {
            JitPolicyArg::Profile => Self::Profile,
            JitPolicyArg::StructureMap => Self::StructureMap,
        }
    }
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

    /// Interpreter-only stabilization runs. Defaults to `--warmup` when omitted.
    #[arg(long)]
    pub(super) interpreter_warmup: Option<usize>,

    /// Interpreter-only measured samples. Defaults to `--iterations` when omitted.
    #[arg(long)]
    pub(super) interpreter_iterations: Option<NonZeroUsize>,

    /// Native compiler policy used for the JIT comparison.
    #[arg(long, value_enum, default_value_t = BackendArg::Tiered)]
    pub(super) backend: BackendArg,

    /// Region-entry threshold for adaptive JIT compilation.
    #[arg(long, default_value_t = 10)]
    pub(super) hot_threshold: u64,

    /// Print detailed JIT decisions and failures to stderr.
    #[arg(long, visible_alias = "debug")]
    pub(super) debug_jit: bool,

    /// Print each compiled WXIR function to stderr.
    #[arg(long)]
    pub(super) dump_wxir: bool,

    /// JIT planning policy. Both policies require a hot, runtime-validated profile.
    #[arg(long, value_enum, default_value_t = JitPolicyArg::StructureMap)]
    pub(super) jit_policy: JitPolicyArg,

    /// Runtime core used for the adaptive side of the comparison.
    #[arg(long, value_enum, default_value_t = RuntimeCoreArg::AdaptiveV2)]
    pub(super) runtime_core: RuntimeCoreArg,
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
    reject_unsupported_adaptive_backend(args.runtime_core, args.backend)?;
    let source = read_source(&args.path)?;
    let execution_mode = if args.interpreter {
        ExecutionMode::Interpreter
    } else {
        ExecutionMode::Jit(args.backend.into())
    };
    let config = RuntimeConfig {
        execution_mode,
        hot_threshold: args.hot_threshold,
    };
    let mut runtime = match args.runtime_core {
        RuntimeCoreArg::Legacy => Runtime::new(config),
        RuntimeCoreArg::AdaptiveV2 => Runtime::new_adaptive_v2(config),
    };
    runtime.set_jit_policy(args.jit_policy.into());
    runtime.set_dump_wxir(args.dump_wxir);
    let executable = runtime
        .compile_function(&source, &args.function)
        .map_err(|error| error.to_string())?;
    let mut profile_cache = ProfileCache::new(
        &source,
        &args.function,
        &executable,
        !args.interpreter && args.runtime_core == RuntimeCoreArg::Legacy,
    );
    if let Some(artifact) = profile_cache.load()
        && runtime
            .seed_profile(&executable, &artifact, profile_cache.fingerprint())
            .is_err()
    {
        profile_cache.reject();
    }
    let function_arguments =
        parse_arguments(&mut runtime, executable.parameters(), &args.arguments)?;

    let mut runs = Vec::with_capacity(args.repeat.get());
    for index in 1..=args.repeat.get() {
        let value = runtime
            .execute_with_args(&executable, &function_arguments)
            .map_err(|error| error.to_string())?;
        runs.push(RunOutput::snapshot(index, value, &runtime)?);
    }
    if let Some(artifact) =
        runtime.profile_artifact(&executable, profile_cache.fingerprint().to_string())
    {
        profile_cache.store(&artifact);
    }

    if args.json {
        report::print_json(&RunDocument::new(
            RunContext {
                path: args.path.display().to_string(),
                function: args.function,
                execution_mode,
                hot_threshold: args.hot_threshold,
                profile_cache: profile_cache.status(),
                adaptive_v2: runtime.last_adaptive_report().cloned(),
            },
            runs,
        ))?;
    } else {
        print_run_values(&runs);
        if args.debug_jit {
            eprintln!(
                "profile cache: {}",
                serde_json::to_value(profile_cache.status())
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "error".to_string())
            );
            print_jit_trace(&runs);
            if let Some(report) = runtime.last_adaptive_report() {
                print_adaptive_debug(report);
            }
        }
    }
    Ok(())
}

fn reject_unsupported_adaptive_backend(
    runtime_core: RuntimeCoreArg,
    backend: BackendArg,
) -> Result<(), String> {
    #[cfg(feature = "inkwell")]
    if runtime_core == RuntimeCoreArg::AdaptiveV2 && backend == BackendArg::Llvm {
        return Err(
            "adaptive-v2 requires Cranelift tier-1 before LLVM promotion; use --backend tiered"
                .to_owned(),
        );
    }
    let _ = (runtime_core, backend);
    Ok(())
}

fn print_adaptive_debug(report: &AdaptiveReport) {
    eprintln!(
        "adaptive-v2 schema={} default=legacy qualified={} rollback={} machine_entries={} native_executions={} helper_calls={} generic_dispatch_calls={} deopts={}",
        report.schema_version,
        report.qualified_for_default,
        report.rollback_available,
        report.machine_entries,
        report.native_executions,
        report.helper_calls,
        report.generic_dispatch_calls,
        report.deopts,
    );
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

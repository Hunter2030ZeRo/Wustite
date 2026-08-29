use std::time::Duration;

use wustite::wvm::JitReport;
use wustite::{AdaptiveReport, CompilerBackend, ExecutionMode, Runtime, RuntimeConfig};

use super::arguments::parse_arguments;
use super::report::{JitOutput, print_jit_debug};
use super::{BenchArgs, RuntimeCoreArg, read_source};

struct DurationStats {
    min: Duration,
    median: Duration,
    p95: Duration,
    max: Duration,
}

pub(super) fn run(args: BenchArgs) -> Result<(), String> {
    super::reject_unsupported_adaptive_backend(args.runtime_core, args.backend)?;
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: benchmark results from a debug build are not representative; use `cargo run --release -- bench ...`"
        );
    }

    let source = read_source(&args.path)?;
    let mut compiler = Runtime::new(RuntimeConfig::default());
    let compilation = compiler
        .compile_function_measured(&source, &args.function)
        .map_err(|error| error.to_string())?;
    let executable = compilation.executable;

    let mut interpreter = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: args.hot_threshold,
    });
    let interpreter_warmup = args.interpreter_warmup.unwrap_or(args.warmup);
    let interpreter_iterations = args.interpreter_iterations.unwrap_or(args.iterations);
    let interpreter_arguments =
        parse_arguments(&mut interpreter, executable.parameters(), &args.arguments)?;
    for _ in 0..interpreter_warmup {
        interpreter
            .execute_with_args(&executable, &interpreter_arguments)
            .map_err(|error| error.to_string())?;
    }
    let mut expected = None;
    let mut interpreter_samples = Vec::with_capacity(interpreter_iterations.get());
    for _ in 0..interpreter_iterations.get() {
        let execution = interpreter
            .execute_measured_with_args(&executable, &interpreter_arguments)
            .map_err(|error| error.to_string())?;
        match expected {
            Some(value) if value != execution.value => {
                return Err("interpreter benchmark samples returned different values".to_owned());
            }
            None => expected = Some(execution.value),
            Some(_) => {}
        }
        interpreter_samples.push(execution.metrics.total_time);
    }
    let expected = expected.ok_or_else(|| "benchmark produced no reference value".to_owned())?;
    let interpreter_stats = summarize_durations(interpreter_samples)?;

    let compiler_backend = CompilerBackend::from(args.backend);
    let adaptive_config = RuntimeConfig {
        execution_mode: ExecutionMode::Jit(compiler_backend),
        hot_threshold: args.hot_threshold,
    };
    let mut adaptive = match args.runtime_core {
        RuntimeCoreArg::Legacy => Runtime::new(adaptive_config),
        RuntimeCoreArg::AdaptiveV2 => Runtime::new_adaptive_v2(adaptive_config),
    };
    adaptive.set_jit_policy(args.jit_policy.into());
    adaptive.set_dump_wxir(args.dump_wxir);
    let adaptive_arguments =
        parse_arguments(&mut adaptive, executable.parameters(), &args.arguments)?;
    let cold = adaptive
        .execute_measured_with_args(&executable, &adaptive_arguments)
        .map_err(|error| error.to_string())?;
    validate_result("adaptive cold", cold.value, expected)?;
    for _ in 0..args.warmup {
        let value = adaptive
            .execute_with_args(&executable, &adaptive_arguments)
            .map_err(|error| error.to_string())?;
        validate_result("adaptive warmup", value, expected)?;
    }
    let measured_start = adaptive.last_adaptive_report().cloned();
    adaptive.begin_adaptive_report_batch();
    let mut adaptive_samples = Vec::with_capacity(args.iterations.get());
    for _ in 0..args.iterations.get() {
        let execution = adaptive
            .execute_measured_with_args(&executable, &adaptive_arguments)
            .map_err(|error| error.to_string())?;
        validate_result("adaptive measured", execution.value, expected)?;
        adaptive_samples.push(execution.metrics.total_time);
    }
    adaptive.end_adaptive_report_batch();
    let adaptive_stats = summarize_durations(adaptive_samples)?;

    print_benchmark(
        &args,
        compilation.metrics.frontend_time,
        cold.metrics.total_time,
        &cold.jit_report,
        &interpreter_stats,
        &adaptive_stats,
    );
    if args.debug_jit {
        print_jit_debug("benchmark cold", &JitOutput::snapshot(&cold.jit_report));
        if let Some(report) = adaptive.last_adaptive_report() {
            super::print_adaptive_debug(report);
            print_adaptive_delta(measured_start.as_ref(), report);
        }
    }
    Ok(())
}

fn print_adaptive_delta(start: Option<&AdaptiveReport>, end: &AdaptiveReport) {
    let start_machine = start.map_or(0, |report| report.machine_entries);
    let start_helpers = start.map_or(0, |report| report.helper_calls);
    let start_generic = start.map_or(0, |report| report.generic_dispatch_calls);
    let start_deopts = start.map_or(0, |report| report.deopts);
    eprintln!(
        "adaptive-v2 measured_delta machine_entries={} helper_calls={} generic_dispatch_calls={} deopts={}",
        end.machine_entries.saturating_sub(start_machine),
        end.helper_calls.saturating_sub(start_helpers),
        end.generic_dispatch_calls.saturating_sub(start_generic),
        end.deopts.saturating_sub(start_deopts),
    );
}

fn validate_result(
    sample: &str,
    actual: wustite::RuntimeValue,
    expected: wustite::RuntimeValue,
) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{sample} result mismatch: expected {expected:?}, got {actual:?}"
        ))
    }
}

fn print_benchmark(
    args: &BenchArgs,
    frontend_time: Duration,
    cold_time: Duration,
    cold_jit: &JitReport,
    interpreter: &DurationStats,
    adaptive: &DurationStats,
) {
    println!("Benchmark: {}", args.path.display());
    println!("Function: {}", args.function);
    println!("Warmup runs: {}", args.warmup);
    println!("Measured iterations: {}", args.iterations);
    println!(
        "Interpreter warmup runs: {}",
        args.interpreter_warmup.unwrap_or(args.warmup)
    );
    println!(
        "Interpreter measured iterations: {}",
        args.interpreter_iterations.unwrap_or(args.iterations)
    );
    println!("Adaptive warmup runs: {}", args.warmup);
    println!("Adaptive measured iterations: {}", args.iterations);
    println!(
        "Compiler backend: {}",
        CompilerBackend::from(args.backend).as_str()
    );
    println!("Hot threshold: {}", args.hot_threshold);
    println!();
    println!("Frontend compilation: {}", format_duration(frontend_time));
    println!();
    println!("Interpreter:");
    print_duration_stats(interpreter);
    println!();
    println!("Adaptive JIT cold:");
    println!("  Time: {}", format_duration(cold_time));
    println!("  Compilation attempts: {}", cold_jit.compilation_attempts);
    println!("  Compiled regions: {}", cold_jit.compiled_regions);
    println!(
        "  Tier-2 compiled regions: {}",
        cold_jit.tier2_compiled_regions
    );
    println!("  Native executions: {}", cold_jit.native_executions);
    println!();
    println!("Adaptive JIT warm:");
    print_duration_stats(adaptive);

    if let Some(speedup) = duration_ratio(interpreter.median, adaptive.median) {
        println!();
        println!("Warm speedup: {speedup:.2}x");
    }
    match estimated_break_even(cold_time, adaptive.median, interpreter.median) {
        Some(invocations) => println!("Estimated break-even: {invocations} invocation(s)"),
        None => println!("Estimated break-even: not reached"),
    }
}

fn print_duration_stats(stats: &DurationStats) {
    println!("  Median: {}", format_duration(stats.median));
    println!("  Median ns: {}", stats.median.as_nanos());
    println!("  P95:    {}", format_duration(stats.p95));
    println!("  Min:    {}", format_duration(stats.min));
    println!("  Max:    {}", format_duration(stats.max));
}

fn summarize_durations(mut samples: Vec<Duration>) -> Result<DurationStats, String> {
    samples.sort_unstable();
    let len = samples.len();
    let median = samples
        .get(len / 2)
        .copied()
        .ok_or_else(|| "benchmark produced no measured samples".to_owned())?;
    let p95_index = (len * 95).div_ceil(100).saturating_sub(1).min(len - 1);
    let min = samples
        .first()
        .copied()
        .ok_or_else(|| "benchmark produced no minimum sample".to_owned())?;
    let p95 = samples
        .get(p95_index)
        .copied()
        .ok_or_else(|| "benchmark produced no p95 sample".to_owned())?;
    let max = samples
        .last()
        .copied()
        .ok_or_else(|| "benchmark produced no maximum sample".to_owned())?;
    Ok(DurationStats {
        min,
        median,
        p95,
        max,
    })
}

fn duration_ratio(numerator: Duration, denominator: Duration) -> Option<f64> {
    let denominator = denominator.as_secs_f64();
    (denominator != 0.0).then(|| numerator.as_secs_f64() / denominator)
}

fn estimated_break_even(cold: Duration, warm: Duration, interpreter: Duration) -> Option<u64> {
    let cold = cold.as_secs_f64();
    let warm = warm.as_secs_f64();
    let interpreter = interpreter.as_secs_f64();
    if warm >= interpreter {
        return None;
    }
    Some(
        ((cold - warm).max(0.0) / (interpreter - warm))
            .ceil()
            .max(1.0) as u64,
    )
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds >= 1.0 {
        format!("{seconds:.3} s")
    } else if seconds >= 1e-3 {
        format!("{:.3} ms", seconds * 1e3)
    } else if seconds >= 1e-6 {
        format!("{:.3} μs", seconds * 1e6)
    } else {
        format!("{:.3} ns", seconds * 1e9)
    }
}

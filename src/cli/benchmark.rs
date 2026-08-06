use std::time::Duration;

use wustite::wvm::JitReport;
use wustite::{ExecutionMode, Runtime, RuntimeConfig};

use super::arguments::parse_arguments;
use super::{BenchArgs, read_source};

struct DurationStats {
    min: Duration,
    median: Duration,
    p95: Duration,
    max: Duration,
}

pub(super) fn run(args: BenchArgs) -> Result<(), String> {
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
    let interpreter_arguments =
        parse_arguments(&mut interpreter, executable.parameters(), &args.arguments)?;
    for _ in 0..args.warmup {
        interpreter
            .execute_with_args(&executable, &interpreter_arguments)
            .map_err(|error| error.to_string())?;
    }
    let mut interpreter_samples = Vec::with_capacity(args.iterations.get());
    for _ in 0..args.iterations.get() {
        let execution = interpreter
            .execute_measured_with_args(&executable, &interpreter_arguments)
            .map_err(|error| error.to_string())?;
        interpreter_samples.push(execution.metrics.total_time);
    }
    let interpreter_stats = summarize_durations(interpreter_samples)?;

    let mut adaptive = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: args.hot_threshold,
    });
    let adaptive_arguments =
        parse_arguments(&mut adaptive, executable.parameters(), &args.arguments)?;
    let cold = adaptive
        .execute_measured_with_args(&executable, &adaptive_arguments)
        .map_err(|error| error.to_string())?;
    for _ in 0..args.warmup {
        adaptive
            .execute_with_args(&executable, &adaptive_arguments)
            .map_err(|error| error.to_string())?;
    }
    let mut adaptive_samples = Vec::with_capacity(args.iterations.get());
    for _ in 0..args.iterations.get() {
        let execution = adaptive
            .execute_measured_with_args(&executable, &adaptive_arguments)
            .map_err(|error| error.to_string())?;
        adaptive_samples.push(execution.metrics.total_time);
    }
    let adaptive_stats = summarize_durations(adaptive_samples)?;

    print_benchmark(
        &args,
        compilation.metrics.frontend_time,
        cold.metrics.total_time,
        &cold.jit_report,
        &interpreter_stats,
        &adaptive_stats,
    );
    Ok(())
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

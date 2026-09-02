use std::process::{Command, Output};

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wustite"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn debug_metric(diagnostic: &str, name: &str) -> u64 {
    diagnostic
        .split_whitespace()
        .find_map(|field| {
            let (key, value) = field.split_once('=')?;
            (key == name).then(|| value.parse().unwrap())
        })
        .unwrap_or_else(|| panic!("missing {name} in {diagnostic}"))
}

#[test]
fn execution_commands_advertise_jit_debugging() {
    // Given: the two CLI commands that execute WVM bytecode.
    // When: their help documents are requested.
    let run = run_cli(&["run", "--help"]);
    let bench = run_cli(&["bench", "--help"]);

    // Then: both commands advertise the same JIT-specific debugging option.
    assert!(run.status.success());
    assert!(bench.status.success());
    assert!(stdout(&run).contains("--debug-jit"));
    assert!(stdout(&bench).contains("--debug-jit"));
}

#[test]
fn execution_commands_advertise_wxir_dumping() {
    // Given: the two CLI commands that can compile WXIR regions.
    // When: their help documents are requested.
    let run = run_cli(&["run", "--help"]);
    let bench = run_cli(&["bench", "--help"]);

    // Then: both commands expose the WXIR dump switch.
    assert!(run.status.success());
    assert!(bench.status.success());
    assert!(stdout(&run).contains("--dump-wxir"));
    assert!(stdout(&bench).contains("--dump-wxir"));
}

#[test]
fn debug_jit_reports_scalar_activity() {
    // Given: a program whose loop contains a proven SmallInt multiplication.
    // When: JIT debugging is enabled through its short compatibility alias.
    let output = run_cli(&[
        "run",
        "tests/fixtures/jit_failure.py",
        "--hot-threshold",
        "8",
        "--debug",
    ]);

    // Then: stderr reports successful native compilation without a silent fallback.
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "40320");
    let diagnostic = stderr(&output);
    assert!(diagnostic.contains("compiled_regions=1"));
    assert!(diagnostic.contains("native_executions=1"));
    assert!(
        diagnostic.contains("helper_calls call=0 get_item=0 set_item=0 length=0 object_access=0")
    );
    assert!(diagnostic.contains("runtime_ops load_constant=0 binary=0 compare=0"));
    assert!(diagnostic.contains("guest_calls direct_native=0 interpreter_fallback=0"));
    assert!(diagnostic.contains("exits region_exit=1 replay_instruction=0 deopt=0"));
    assert!(diagnostic.contains("calls main=1"));
    assert!(!diagnostic.contains("failure function="));
    assert!(!diagnostic.contains("wxir region"));
}

#[test]
fn run_dump_wxir_prints_each_region_to_stderr() {
    // Given: a program with one JIT-compilable region.
    // When: run executes it with WXIR dumping enabled.
    let output = run_cli(&[
        "run",
        "tests/fixtures/jit_failure.py",
        "--hot-threshold",
        "8",
        "--dump-wxir",
    ]);

    // Then: stdout remains the program result and stderr receives one WXIR function.
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "40320");
    assert_eq!(stderr(&output).matches("wxir region").count(), 1);
}

#[test]
fn bench_debug_jit_reports_cold_native() {
    // Given: the same semantically valid program with a generic native runtime call.
    // When: a minimal benchmark is run with JIT debugging enabled.
    let output = run_cli(&[
        "bench",
        "tests/fixtures/jit_failure.py",
        "--warmup",
        "0",
        "--iterations",
        "1",
        "--hot-threshold",
        "8",
        "--debug-jit",
    ]);

    // Then: benchmark output remains on stdout and cold native execution is diagnosed on stderr.
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Benchmark: tests/fixtures/jit_failure.py"));
    let diagnostic = stderr(&output);
    assert!(diagnostic.contains("benchmark cold: compilation_attempts=1 compiled_regions=1"));
    assert!(diagnostic.contains("native_executions=1"));
    assert!(!diagnostic.contains("failure function="));
}

#[test]
fn bench_can_bound_interpreter_control_independently() {
    // Given: adaptive timing needs several samples while the interpreter is only a
    // correctness control for application fixtures.
    let output = run_cli(&[
        "bench",
        "tests/fixtures/adaptive_add.py",
        "--arg",
        "20",
        "--arg",
        "22",
        "--warmup",
        "2",
        "--iterations",
        "3",
        "--interpreter-warmup",
        "0",
        "--interpreter-iterations",
        "1",
    ]);

    // Then: both budgets are explicit and the independent control does not alter
    // the adaptive sample contract.
    assert!(output.status.success(), "{}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains("Interpreter warmup runs: 0"));
    assert!(report.contains("Interpreter measured iterations: 1"));
    assert!(report.contains("Adaptive warmup runs: 2"));
    assert!(report.contains("Adaptive measured iterations: 3"));
}

#[test]
fn bench_reports_exact_measured_adaptive_window() {
    // Given: the list macro is already hot before a three-sample measured window.
    let output = run_cli(&[
        "bench",
        "benchmarks/adaptive_list_objects.py",
        "--runtime-core",
        "adaptive-v2",
        "--backend",
        "cranelift",
        "--warmup",
        "192",
        "--iterations",
        "3",
        "--interpreter-warmup",
        "0",
        "--interpreter-iterations",
        "1",
        "--debug-jit",
    ]);

    // Then: diagnostics account for exactly the measured executions, independently
    // of the cold and warmup phases.
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains(
        "adaptive-v2 measured_delta machine_entries=3 helper_calls=0 generic_dispatch_calls=0 deopts=0"
    ));
}

#[test]
fn bench_dump_wxir_prints_regions_to_stderr() {
    // Given: a one-sample benchmark whose adaptive runtime compiles one region.
    // When: benchmark execution enables WXIR dumping.
    let output = run_cli(&[
        "bench",
        "tests/fixtures/jit_failure.py",
        "--warmup",
        "0",
        "--iterations",
        "1",
        "--hot-threshold",
        "8",
        "--dump-wxir",
    ]);

    // Then: benchmark output stays on stdout and one WXIR function is emitted on stderr.
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Benchmark: tests/fixtures/jit_failure.py"));
    assert_eq!(stderr(&output).matches("wxir region").count(), 1);
}

#[test]
fn nested_objects_report_native_success() {
    // Given: spectral_norm executes JIT-eligible loops inside nested helper functions.
    // When: its JIT diagnostics are captured.
    let output = run_cli(&[
        "run",
        "examples/spectral_norm.py",
        "--hot-threshold",
        "8",
        "--debug",
    ]);

    // Then: guarded StructureMap selection compiles native regions without disabling a helper.
    assert!(output.status.success(), "{}", stderr(&output));
    let actual = stdout(&output).trim().parse::<f64>().unwrap();
    let expected = 1.623_642_239_802_079_6_f64;
    let difference = (actual - expected).abs();
    let scale = actual.abs().max(expected.abs());
    assert!(difference <= 1e-12 || difference <= 1e-12 * scale);
    let diagnostic = stderr(&output);
    let attempts = debug_metric(&diagnostic, "compilation_attempts");
    assert!(attempts > 0);
    assert_eq!(attempts, debug_metric(&diagnostic, "compiled_regions"));
    assert_eq!(
        debug_metric(&diagnostic, "tier2_compilation_attempts"),
        debug_metric(&diagnostic, "tier2_compiled_regions")
    );
    assert_eq!(debug_metric(&diagnostic, "disabled_regions"), 0);
    assert!(debug_metric(&diagnostic, "native_executions") > 0);
    debug_metric(&diagnostic, "deopt");
    assert!(!diagnostic.contains("failure function="));
}

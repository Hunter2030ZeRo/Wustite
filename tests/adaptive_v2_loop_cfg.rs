use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

fn adaptive_runtime() -> Runtime {
    Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    })
}

fn execute(source: &str) -> (RuntimeValue, wustite::AdaptiveReport) {
    let mut runtime = adaptive_runtime();
    let executable = runtime.compile_function(source, "main").unwrap();
    let value = runtime.execute(&executable).unwrap();
    let report = runtime.last_adaptive_report().unwrap().clone();
    (value, report)
}

#[test]
fn adaptive_loop_cfg_executes_divergent_body_and_merge_in_native_code() {
    // Given: a loop whose body has two arithmetic paths that merge before the backedge.
    let source = include_str!("fixtures/adaptive_loop_branch.py");

    // When: one execution crosses both live gates at the loop header.
    let (value, report) = execute(source);

    // Then: the merged loop state is exact and the accepted trace needs no generic dispatch.
    assert_eq!(value, RuntimeValue::SmallInt(330));
    assert!(report.machine_entries > 0, "{report:?}");
    assert!(report.native_executions > 0, "{report:?}");
    assert!(report.cache_hits > 0, "{report:?}");
    assert!(report.selected_snapshot_id.is_some(), "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

#[test]
fn adaptive_loop_cfg_executes_distinct_break_exit_in_native_code() {
    // Given: a loop with a condition exit and a separate forward break exit.
    let source = include_str!("fixtures/adaptive_loop_break.py");

    // When: execution becomes hot before taking the break edge.
    let (value, report) = execute(source);

    // Then: the distinct exit returns the exact value from native code.
    assert_eq!(value, RuntimeValue::SmallInt(7_140));
    assert!(report.machine_entries > 0, "{report:?}");
    assert!(report.native_executions > 0, "{report:?}");
    assert!(report.cache_hits > 0, "{report:?}");
    assert!(report.selected_snapshot_id.is_some(), "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

#[test]
fn integer_subtract_and_multiply_loop_execute_in_native_code() {
    // Given: a hot integer loop using typed subtract and multiply operations.
    let source = include_str!("fixtures/adaptive_loop_integer_arithmetic.py");

    // When: the loop crosses both live gates and compiles its general CFG.
    let (value, report) = execute(source);

    // Then: the exact result is produced by accepted native code without dispatch.
    assert_eq!(value, RuntimeValue::SmallInt(1));
    assert!(report.machine_entries > 0, "{report:?}");
    assert!(report.native_executions > 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert!(report.compile_failure.is_none(), "{report:?}");
}

#[test]
fn f64_divide_unary_and_boolean_loop_execute_in_native_code() {
    // Given: a hot loop carrying f64, bool, and integer header values.
    let source = include_str!("fixtures/adaptive_loop_f64.py");

    // When: float arithmetic, unary negate, and boolean-and are lowered together.
    let (value, report) = execute(source);

    // Then: Cranelift returns the exact f64 result with no generic dispatch.
    assert_eq!(value, RuntimeValue::Float(180.0));
    assert!(report.machine_entries > 0, "{report:?}");
    assert!(report.native_executions > 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

#[test]
fn floor_divide_without_exact_wxir_semantics_remains_cold() {
    // Given: signed floor division whose zero, floor, and BigInt semantics lack a WXIR opcode.
    let source = include_str!("fixtures/adaptive_loop_floor_divide.py");

    // When: the loop reaches recording through the real Python frontend.
    let (value, report) = execute(source);

    // Then: WVM stays authoritative and native success is never reported.
    assert_eq!(value, RuntimeValue::SmallInt(7));
    assert_eq!(report.machine_entries, 0, "{report:?}");
    assert_eq!(report.native_executions, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert!(report.compile_failure.is_some(), "{report:?}");
}

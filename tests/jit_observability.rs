use wustite::{CompilerBackend, ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const CALL_SOURCE: &str = "def helper(value: int):\n    return value\n\ndef main():\n    values = [0]\n    total = 0\n    for index in range(10):\n        values[0] = values[0] + 1\n        total += helper(len(values))\n    return total\n";

#[test]
fn native_runtime_calls_are_grouped_by_operation_and_source_function() {
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Jit(CompilerBackend::Cranelift),
        hot_threshold: 1,
    });

    let result = runtime.run_function(CALL_SOURCE, "main");

    assert!(
        matches!(result, Ok(RuntimeValue::SmallInt(10))),
        "{result:?}"
    );
    let report = runtime.last_jit_report();
    assert_eq!(report.helper_calls.call, 2, "{report:?}");
    assert_eq!(report.helper_calls.get_item, 2);
    assert_eq!(report.helper_calls.set_item, 2);
    assert_eq!(report.helper_calls.length, 2);
    assert!(report.helper_calls.object_access >= 2, "{report:?}");
    assert_eq!(report.guest_calls.direct_native, 12);
    assert_eq!(report.guest_calls.interpreter_fallback, 0);
    assert_eq!(report.call_sites.leaf_plans, 1);
    assert_eq!(report.call_sites.prepared_leaf_hit, 10);
    assert_eq!(report.call_sites.compiled_leaf_hit, 10);
    assert_eq!(report.calls.get("main"), Some(&1));
    assert_eq!(report.calls.get("helper"), Some(&10));
    assert_eq!(report.exits.region_exit, report.native_executions);
}

#[test]
fn interpreter_guest_calls_are_reported_as_fallbacks() {
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 1,
    });

    let result = runtime.run_function(CALL_SOURCE, "main");

    assert!(
        matches!(result, Ok(RuntimeValue::SmallInt(10))),
        "{result:?}"
    );
    let report = runtime.last_jit_report();
    assert_eq!(report.guest_calls.direct_native, 0);
    assert_eq!(report.guest_calls.interpreter_fallback, 10);
    assert_eq!(report.calls.get("main"), Some(&1));
    assert_eq!(report.calls.get("helper"), Some(&10));
    assert_eq!(report.native_executions, 0);
}

use std::process::Command;

use wustite::{CompilerBackend, ExecutionMode, JitPolicy, Runtime, RuntimeConfig, RuntimeValue};

fn interpreter_runtime() -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    })
}

#[test]
fn class_constructor_fields_direct_and_bound_methods_execute() {
    let mut runtime = interpreter_runtime();
    let source = include_str!("fixtures/classes.py");

    let result = runtime.run_function(source, "main").unwrap();

    assert_eq!(result, RuntimeValue::SmallInt(10));
}

#[test]
fn cli_runs_class_fixture() {
    let source = format!("{}/tests/fixtures/classes.py", env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(env!("CARGO_BIN_EXE_wustite"))
        .args(["run", &source, "--function", "main", "--json"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output["runs"][0]["value"]["value"], 10);
}

#[test]
fn object_method_loop_crosses_the_wxir_runtime_boundary() {
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Jit(CompilerBackend::Tiered),
        hot_threshold: 8,
    });
    runtime.set_jit_policy(JitPolicy::StructureMap);
    let executable = runtime
        .compile_function(include_str!("fixtures/classes.py"), "loop_main")
        .unwrap();

    let result = runtime.execute(&executable).unwrap();

    assert_eq!(result, RuntimeValue::SmallInt(20));
    assert_eq!(runtime.last_jit_report().compiled_regions, 1);
    assert!(runtime.last_jit_report().native_executions > 0);
}

#[test]
fn method_call_site_uses_two_shape_cases_then_megamorphic_fallback() {
    let mut runtime = interpreter_runtime();
    let executable = runtime
        .compile_function(include_str!("fixtures/classes.py"), "polymorphic_main")
        .unwrap();

    let result = runtime.execute(&executable).unwrap();

    assert_eq!(result, RuntimeValue::SmallInt(6));
    assert_eq!(runtime.last_jit_report().call_sites.call_guard_miss, 2);
    assert_eq!(runtime.last_jit_report().call_sites.megamorphic_fallback, 1);
}

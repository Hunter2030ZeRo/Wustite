use std::process::Command;

use wustite::value::Value;
use wustite::{ExecutionMode, Object, Runtime, RuntimeConfig, RuntimeValue};

fn interpreter_runtime() -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    })
}

#[test]
fn list_comprehension_maps_a_range_when_returned() {
    // Given: a Python function whose result is produced by one range comprehension.
    let mut runtime = interpreter_runtime();
    let source = "def squares():\n    return [value * value for value in range(1, 5)]\n";

    // When: the function is compiled and executed through the public runtime API.
    let result = runtime.run_function(source, "squares").unwrap();

    // Then: every range value is mapped into the returned list in iteration order.
    let RuntimeValue::Object(result) = result else {
        panic!("list comprehension must return an object reference");
    };
    assert!(matches!(
        runtime.object(result).unwrap(),
        Object::List(values)
            if values.to_vec() == [
                Value::SmallInt(1),
                Value::SmallInt(4),
                Value::SmallInt(9),
                Value::SmallInt(16),
            ]
    ));
}

#[test]
fn list_comprehension_maps_a_list_argument() {
    // Given: a runtime-owned list passed to a function that comprehends its argument.
    let mut runtime = interpreter_runtime();
    let source = "def values():\n    return [2, 4, 6]\n\ndef shift(items: list):\n    return [item + 1 for item in items]\n";
    let input = runtime.run_function(source, "values").unwrap();
    let executable = runtime.compile_function(source, "shift").unwrap();

    // When: the list argument is consumed by the comprehension.
    let result = runtime.execute_with_args(&executable, &[input]).unwrap();

    // Then: sequence iteration preserves order and maps each item exactly once.
    let RuntimeValue::Object(result) = result else {
        panic!("list comprehension must return an object reference");
    };
    assert!(matches!(
        runtime.object(result).unwrap(),
        Object::List(values)
            if values.to_vec() == [
                Value::SmallInt(3),
                Value::SmallInt(5),
                Value::SmallInt(7),
            ]
    ));
}

#[test]
fn list_comprehension_target_does_not_replace_an_outer_local() {
    // Given: a comprehension target that shadows an initialized function local.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    value = 9\n    mapped = [value * 2 for value in range(3)]\n    return value + len(mapped)\n";

    // When: the function executes after the comprehension finishes.
    let result = runtime.run_function(source, "main").unwrap();

    // Then: Python 3 comprehension scoping preserves the outer local value.
    assert_eq!(result, RuntimeValue::SmallInt(12));
}

#[test]
fn cli_runs_a_list_comprehension_fixture() {
    // Given: a Python fixture that returns a list comprehension result.
    let source = format!(
        "{}/tests/fixtures/list_comprehension.py",
        env!("CARGO_MANIFEST_DIR")
    );

    // When: the compiled CLI executes the fixture through its default tiered mode.
    let output = Command::new(env!("CARGO_BIN_EXE_wustite"))
        .args(["run", &source, "--function", "main", "--json"])
        .output()
        .unwrap();

    // Then: the process succeeds and reports a list object through its JSON contract.
    assert!(output.status.success());
    let output: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output["runs"][0]["value"]["value"]["kind"], "list");
}

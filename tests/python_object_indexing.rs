use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeError, RuntimeValue};

fn interpreter_runtime() -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 10,
    })
}

fn assert_execution_error(result: Result<RuntimeValue, RuntimeError>, expected: &str) {
    match result {
        Err(RuntimeError::Execution(message)) => assert_eq!(message, expected),
        other => panic!("expected execution error {expected:?}, got {other:?}"),
    }
}

#[test]
fn negative_list_index_selects_from_the_end() {
    // Given: a list indexed by the normal negative boundary.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    return [10, 20, 30][-1]\n";

    // When: the public interpreter runtime evaluates the subscript.
    let result = runtime.run_function(source, "main");

    // Then: index -1 selects the final element.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(30))));
}

#[test]
fn out_of_range_list_index_returns_the_exact_runtime_error() {
    // Given: a list index equal to the sequence length.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    return [10, 20, 30][3]\n";

    // When: the public interpreter runtime evaluates the invalid subscript.
    let result = runtime.run_function(source, "main");

    // Then: ObjectOps preserves its exact range error through Runtime.
    assert_execution_error(result, "sequence index out of range");
}

#[test]
fn forward_slice_preserves_selected_order() {
    // Given: a forward slice with explicit start, stop, and stride.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    selected = [0, 1, 2, 3, 4][1:5:2]\n    return selected[0] * 10 + selected[1]\n";

    // When: the public interpreter runtime evaluates the slice.
    let result = runtime.run_function(source, "main");

    // Then: the selected values remain ordered as 1, 3.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(13))));
}

#[test]
fn reverse_slice_preserves_descending_order() {
    // Given: a list sliced with an omitted range and a negative stride.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    selected = [0, 1, 2, 3, 4][::-1]\n    return selected[0] * 10 + selected[4]\n";

    // When: the public interpreter runtime evaluates the reverse slice.
    let result = runtime.run_function(source, "main");

    // Then: the first and final values demonstrate descending order.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(40))));
}

#[test]
fn zero_slice_step_returns_the_exact_runtime_error() {
    // Given: a slice whose explicit stride is zero.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    return [0, 1, 2][::0]\n";

    // When: the public interpreter runtime evaluates the invalid slice.
    let result = runtime.run_function(source, "main");

    // Then: ObjectOps preserves its exact zero-step error through Runtime.
    assert_execution_error(result, "slice step cannot be zero");
}

#[test]
fn extended_slice_replacement_size_mismatch_returns_the_exact_runtime_error() {
    // Given: two extended-slice targets and only one replacement value.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    values = [0, 1, 2, 3]\n    values[::2] = [7]\n    return 0\n";

    // When: the public interpreter runtime evaluates the invalid assignment.
    let result = runtime.run_function(source, "main");

    // Then: ObjectOps preserves its exact replacement-size error through Runtime.
    assert_execution_error(result, "extended slice assignment size mismatch");
}

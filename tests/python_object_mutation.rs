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
fn append_insert_and_default_pop_mutate_the_same_list_object() {
    // Given: two names referencing one list and three ordered mutations.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    values = [2, 3]\n    alias = values\n    alias.append(4)\n    values.insert(0, 1)\n    removed = alias.pop()\n    return values[0] * 100 + values[1] * 10 + removed\n";

    // When: the public interpreter runtime applies the mutations through both names.
    let result = runtime.run_function(source, "main");

    // Then: append, insert, and default pop preserve aliasing and Python ordering.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(124))));
}

#[test]
fn insert_clips_positions_at_both_ends_and_normalizes_negative_positions() {
    // Given: insertions before, beyond, and just before a one-element list.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    values = [2]\n    values.insert(-100, 1)\n    values.insert(100, 4)\n    values.insert(-1, 3)\n    return values[0] * 1000 + values[1] * 100 + values[2] * 10 + values[3]\n";

    // When: the public interpreter runtime normalizes each insertion index.
    let result = runtime.run_function(source, "main");

    // Then: the clipped and negative positions produce 1, 2, 3, 4.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(1234))));
}

#[test]
fn explicit_negative_pop_removes_the_selected_item() {
    // Given: a three-element list and an explicit negative pop index.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    values = [1, 2, 3]\n    removed = values.pop(-2)\n    return removed * 100 + values[0] * 10 + values[1]\n";

    // When: the public interpreter runtime executes the indexed pop.
    let result = runtime.run_function(source, "main");

    // Then: pop returns 2 and leaves the list ordered as 1, 3.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(213))));
}

#[test]
fn empty_list_pop_returns_the_exact_runtime_error() {
    // Given: an empty list and a default pop call.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    values = []\n    return values.pop()\n";

    // When: the public interpreter runtime executes the invalid pop.
    let result = runtime.run_function(source, "main");

    // Then: ObjectOps preserves its exact range error through Runtime.
    assert_execution_error(result, "sequence index out of range");
}

#[test]
fn dictionary_replacement_and_insertion_mutate_the_same_object() {
    // Given: two names referencing a dictionary before replacement and insertion.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    values = {\"answer\": 1}\n    alias = values\n    alias[\"answer\"] = 4\n    values[\"extra\"] = 2\n    return alias[\"answer\"] * 10 + alias[\"extra\"]\n";

    // When: the public interpreter runtime applies both dictionary assignments.
    let result = runtime.run_function(source, "main");

    // Then: replacement and insertion are observable through the alias.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(42))));
}

#[test]
fn missing_dictionary_key_returns_the_exact_runtime_error() {
    // Given: a dictionary lookup for an absent key.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    values = {\"answer\": 42}\n    return values[\"missing\"]\n";

    // When: the public interpreter runtime executes the missing-key lookup.
    let result = runtime.run_function(source, "main");

    // Then: ObjectOps preserves its exact missing-key error through Runtime.
    assert_execution_error(result, "dictionary key not found");
}

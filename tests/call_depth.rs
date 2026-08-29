use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeError, RuntimeValue};

const RECURSIVE_SOURCE: &str = r#"def recurse():
    return recurse()
"#;

const PROFILED_RECURSIVE_SOURCE: &str = r#"def recurse():
    index = 0
    limit = 1
    step = 1
    while index < limit:
        index = index + step
    return recurse()
"#;

const FINITE_CHAIN_SOURCE: &str = r#"def leaf():
    return 42

def middle():
    return leaf()

def root():
    return middle()
"#;

fn interpreter_runtime() -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    })
}

#[test]
fn direct_guest_recursion_returns_an_execution_error_at_the_call_depth_limit() {
    // Given: a directly recursive closureless guest function.
    let mut runtime = interpreter_runtime();
    let executable = runtime
        .compile_function(RECURSIVE_SOURCE, "recurse")
        .unwrap();

    // When: the guest function recursively calls itself without a base case.
    let error = runtime.execute(&executable).unwrap_err();

    // Then: execution stops with a controlled typed error before exhausting the host stack.
    assert!(
        matches!(error, RuntimeError::Execution(message) if message.contains("guest call depth limit"))
    );
}

#[test]
fn finite_nested_guest_calls_succeed_after_a_depth_limit_error() {
    // Given: a runtime whose prior recursive execution exhausted the guest call-depth budget.
    let mut runtime = interpreter_runtime();
    let recursive = runtime
        .compile_function(RECURSIVE_SOURCE, "recurse")
        .unwrap();
    assert!(runtime.execute(&recursive).is_err());
    let root = runtime
        .compile_function(FINITE_CHAIN_SOURCE, "root")
        .unwrap();

    // When: the same runtime executes a finite root-to-middle-to-leaf call chain.
    let result = runtime.execute(&root).unwrap();

    // Then: unwound depth bookkeeping permits every nested call to complete.
    assert_eq!(result, RuntimeValue::SmallInt(42));
}

#[test]
fn same_function_nested_activations_keep_interpreter_profile_inert() {
    // Given: a recursive function that enters one loop in every activation.
    let mut runtime = interpreter_runtime();
    let executable = runtime
        .compile_function(PROFILED_RECURSIVE_SOURCE, "recurse")
        .unwrap();

    // When: recursive execution reaches the guest call-depth limit.
    let result = runtime.execute(&executable);

    // Then: pure interpreter activations never collect JIT profile entries.
    assert!(result.is_err());
    assert_eq!(
        runtime
            .profile_for(&executable)
            .unwrap()
            .entry_count(wustite::structure_map::RegionId(0)),
        0
    );
}

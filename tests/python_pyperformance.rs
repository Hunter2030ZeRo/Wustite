use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const FANNKUCH_SOURCE: &str = include_str!("../examples/fannkuch.py");
const NBODY_SOURCE: &str = include_str!("../examples/nbody.py");

fn interpreter_runtime() -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 10,
    })
}

#[test]
fn pyperformance_fannkuch_kernel_executes() {
    // Given: the PyPerformance fannkuch kernel and an interpreter runtime.
    let mut runtime = interpreter_runtime();

    // When: the public runtime compiles and executes the benchmark entry point.
    let result = runtime.run_function(FANNKUCH_SOURCE, "main");

    // Then: fannkuch(9) reports the reference maximum flip count.
    assert!(
        matches!(&result, Ok(RuntimeValue::SmallInt(30))),
        "fannkuch result: {result:?}"
    );
}

#[test]
fn pyperformance_nbody_kernel_executes() {
    // Given: the PyPerformance nbody kernel and an interpreter runtime.
    let mut runtime = interpreter_runtime();

    // When: the public runtime advances the system and reports its energy.
    let result = runtime.run_function(NBODY_SOURCE, "main");

    // Then: the mutated system retains the CPython reference energy.
    assert!(
        matches!(
            &result,
            Ok(RuntimeValue::Float(value)) if (-0.1691..-0.1690).contains(value)
        ),
        "nbody result: {result:?}"
    );
}

#[test]
fn list_slices_insert_pop_keep_python_ordering() {
    // Given: list slice reads, slice replacement, insert, and pop in one function.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    values = [0, 1, 2, 3]\n    reverse = values[2::-1]\n    values[1:3] = reverse\n    values.insert(1, values.pop(0))\n    return values[0] * 10000 + values[1] * 1000 + values[2] * 100 + values[3] * 10 + values[4]\n";

    // When: the public runtime executes each mutation against the same list object.
    let result = runtime.run_function(source, "main");

    // Then: every operation observes the exact ordering produced by CPython.
    assert!(
        matches!(&result, Ok(RuntimeValue::SmallInt(20103))),
        "slice/list mutation result: {result:?}"
    );
}

#[test]
fn break_skips_else_normal_exit_runs_else() {
    // Given: a broken range loop and a normally completed while loop.
    let mut runtime = interpreter_runtime();
    let source = "def main():\n    total = 0\n    for index in range(5):\n        if index == 3:\n            break\n        total += index\n    else:\n        total = 100\n    cursor = 0\n    while cursor < 2:\n        cursor += 1\n    else:\n        total += 10\n    return total\n";

    // When: both loop exit paths execute through lowered WVM control flow.
    let result = runtime.run_function(source, "main");

    // Then: only the normal while exit contributes its else body.
    assert!(
        matches!(&result, Ok(RuntimeValue::SmallInt(13))),
        "loop else result: {result:?}"
    );
}

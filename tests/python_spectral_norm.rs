use wustite::bytecode::Instruction;
use wustite::frontend::python::compile_python_function;
use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const SPECTRAL_NORM_SOURCE: &str = include_str!("../examples/spectral_norm.py");

#[test]
fn spectral_norm_example_compiles_and_executes_through_the_python_frontend() {
    // Given: the unchanged spectral-norm benchmark source and an interpreter runtime.
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 10,
    });

    // When: the public runtime compiles and executes its main function.
    let result = runtime.run_function(SPECTRAL_NORM_SOURCE, "main");

    // Then: the benchmark produces the expected finite spectral-norm range.
    assert!(
        matches!(
            &result,
            Ok(RuntimeValue::Float(value)) if (1.62..1.63).contains(value)
        ),
        "spectral norm result: {result:?}"
    );
}

#[test]
fn floor_division_and_list_repetition_follow_python_semantics() {
    // Given: negative floor division and list repetition in one Python expression.
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    });
    let source = "def main():\n    return (-3 // 2) + len([1, 2] * 3)\n";

    // When: the expression executes through the public runtime API.
    let result = runtime.run_function(source, "main");

    // Then: floor division rounds down and repetition preserves all elements.
    assert!(
        matches!(&result, Ok(RuntimeValue::SmallInt(4))),
        "floor division/list repetition result: {result:?}"
    );
}

#[test]
fn tuple_assignment_enumerate_zip_and_augmented_assignment_execute() {
    // Given: the statement forms used by the spectral-norm reduction loops.
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    });
    let source = "def main():\n    pair = (2, [3, 4])\n    first, values = pair\n    total = 0\n    for index, value in enumerate(values):\n        total += index + value\n    for left, right in zip(values, values):\n        total += left * right\n    return first + total\n";

    // When: tuple binding and both indexed iterator forms execute.
    let result = runtime.run_function(source, "main");

    // Then: every bound value contributes exactly once to the reduction.
    assert!(
        matches!(&result, Ok(RuntimeValue::SmallInt(35))),
        "tuple/enumerate/zip result: {result:?}"
    );
}

#[test]
fn module_constant_and_list_copy_feed_collection_length() {
    // Given: a module constant, repeated list, and list-copy constructor.
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    });
    let source =
        "COUNT = 3\n\ndef main():\n    values = [2] * COUNT\n    return len(list(values))\n";

    // When: the function resolves and consumes the module constant.
    let result = runtime.run_function(source, "main");

    // Then: list construction produces the requested number of elements.
    assert!(
        matches!(&result, Ok(RuntimeValue::SmallInt(3))),
        "module constant/list copy result: {result:?}"
    );
}

#[test]
fn fixed_non_empty_range_initializes_body_assignments() {
    // Given: a local first assigned inside a statically non-empty range loop.
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    });
    let source =
        "def main():\n    for index in range(1):\n        result = index + 7\n    return result\n";

    // When: the function returns the body-defined local after the loop.
    let result = runtime.run_function(source, "main");

    // Then: frontend definite assignment recognizes the guaranteed iteration.
    assert!(
        matches!(&result, Ok(RuntimeValue::SmallInt(7))),
        "fixed range initialization result: {result:?}"
    );
}

#[test]
fn nonescaping_builtin_list_copy_length_uses_the_exact_list_input() {
    let source = "def main(values: list):\n    return len(list(values))\n";
    let executable = compile_python_function(source, "main").expect("exact list parameter");
    assert!(
        executable
            .bytecode()
            .code
            .iter()
            .any(|instruction| { matches!(instruction, Instruction::Length { .. }) })
    );
    assert!(!executable.bytecode().code.iter().any(|instruction| {
        matches!(
            instruction,
            Instruction::BuildList { .. } | Instruction::ListAppend { .. }
        )
    }));
    assert!(executable.structure_map().loop_regions().next().is_none());
}

#[test]
fn escaping_or_nonlist_copy_is_not_scalar_replaced() {
    let escaping = compile_python_function(
        "def main(values: list):\n    copied = list(values)\n    return len(copied)\n",
        "main",
    )
    .expect("escaping list copy");
    assert!(escaping.bytecode().code.iter().any(|instruction| {
        matches!(
            instruction,
            Instruction::BuildList { .. } | Instruction::ListAppend { .. }
        )
    }));
    let custom = compile_python_function(
        "def main(values: object):\n    return len(list(values))\n",
        "main",
    )
    .expect("non-list iterable");
    assert!(custom.bytecode().code.iter().any(|instruction| {
        matches!(
            instruction,
            Instruction::BuildList { .. } | Instruction::ListAppend { .. }
        )
    }));
}

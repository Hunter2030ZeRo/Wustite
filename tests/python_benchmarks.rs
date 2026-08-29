use num_bigint::BigInt;
use wustite::frontend::python::compile_python_function;
use wustite::object::Object;
use wustite::value::Value;
use wustite::wvm::Vm;
use wustite::wxir::WxExitKind;
use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const CONTROL_FLOW_SOURCE: &str = r#"def control(value: int, scale: float, enabled: bool):
    result = 0.0
    if enabled and value > 0:
        result = scale * value
    else:
        result = scale / 2
    return result
"#;

const RANGE_SOURCE: &str = r#"def nested_range(rows: int, columns: int):
    total = 0
    for row in range(rows):
        for column in range(1, columns):
            total = total + row * column
    return total

def descending_range():
    total = 0
    for value in range(5, 0, -2):
        total = total + value
    return total

def early_return(limit: int):
    for value in range(limit):
        if value == 3:
            return value
    return -1

def empty_range_preserves_target(value: int):
    for value in range(value + 1, value):
        value = value + 100
    return value
"#;

const NESTED_WHILE_SOURCE: &str = r#"def nested_while(limit: int):
    total = 0
    outer = 0
    while outer < limit:
        inner = 0
        while inner < outer:
            total = total + inner
            inner = inner + 1
        outer = outer + 1
    return total
"#;

const FUNCTION_CALL_SOURCE: &str = r#"def double(value: int):
    return value * 2

def caller(value: int):
    local = double(value)
    return local + 1
"#;

const OVERFLOW_SOURCE: &str = r#"def overflow_loop(seed: int):
    value = seed
    index = 0
    limit = 2
    step = 1
    while index < limit:
        value = value + step
        index = index + step
    return value
"#;

fn interpreter_runtime() -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    })
}

#[test]
fn scalar_control_flow_preserves_typed_values() {
    // Given: typed scalar arguments and an if/else with a Boolean expression.
    let mut runtime = interpreter_runtime();
    let executable = runtime
        .compile_function(CONTROL_FLOW_SOURCE, "control")
        .unwrap();

    // When: both control-flow arms are executed.
    let enabled = runtime
        .execute_with_args(
            &executable,
            &[
                RuntimeValue::SmallInt(4),
                RuntimeValue::Float(1.5),
                RuntimeValue::Bool(true),
            ],
        )
        .unwrap();
    let disabled = runtime
        .execute_with_args(
            &executable,
            &[
                RuntimeValue::SmallInt(4),
                RuntimeValue::Float(1.5),
                RuntimeValue::Bool(false),
            ],
        )
        .unwrap();

    // Then: arithmetic and branch results retain Python scalar semantics.
    assert_eq!(enabled, RuntimeValue::Float(6.0));
    assert_eq!(disabled, RuntimeValue::Float(0.75));
}

#[test]
fn range_and_nested_loops_execute_common_benchmark_shapes() {
    // Given: nested positive ranges, a negative-step range, and nested whiles.
    let mut runtime = interpreter_runtime();
    let nested_range = runtime
        .compile_function(RANGE_SOURCE, "nested_range")
        .unwrap();
    let descending_range = runtime
        .compile_function(RANGE_SOURCE, "descending_range")
        .unwrap();
    let early_return = runtime
        .compile_function(RANGE_SOURCE, "early_return")
        .unwrap();
    let empty_range = runtime
        .compile_function(RANGE_SOURCE, "empty_range_preserves_target")
        .unwrap();
    let nested_while = runtime
        .compile_function(NESTED_WHILE_SOURCE, "nested_while")
        .unwrap();

    // When: representative compiler benchmark loops execute.
    let range_result = runtime
        .execute_with_args(
            &nested_range,
            &[RuntimeValue::SmallInt(4), RuntimeValue::SmallInt(5)],
        )
        .unwrap();
    let descending_result = runtime.execute(&descending_range).unwrap();
    let early_result = runtime
        .execute_with_args(&early_return, &[RuntimeValue::SmallInt(10)])
        .unwrap();
    let empty_result = runtime
        .execute_with_args(&empty_range, &[RuntimeValue::SmallInt(4)])
        .unwrap();
    let while_result = runtime
        .execute_with_args(&nested_while, &[RuntimeValue::SmallInt(5)])
        .unwrap();

    // Then: every loop shape produces the Python result.
    assert_eq!(range_result, RuntimeValue::SmallInt(60));
    assert_eq!(descending_result, RuntimeValue::SmallInt(9));
    assert_eq!(early_result, RuntimeValue::SmallInt(3));
    assert_eq!(empty_result, RuntimeValue::SmallInt(4));
    assert_eq!(while_result, RuntimeValue::SmallInt(10));
}

#[test]
fn local_function_calls_return_values_to_the_caller() {
    // Given: a typed helper called from another compiled Python function.
    let mut runtime = interpreter_runtime();
    let executable = runtime
        .compile_function(FUNCTION_CALL_SOURCE, "caller")
        .unwrap();

    // When: the caller stores and uses the helper's return value.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::SmallInt(20)])
        .unwrap();

    // Then: the returned local participates in subsequent arithmetic.
    assert_eq!(result, RuntimeValue::SmallInt(41));
}

#[test]
#[cfg_attr(miri, ignore)]
fn frontend_loop_overflow_replays_with_arbitrary_precision() {
    // Given: frontend-generated loop bytecode that overflows i64 in native code.
    let executable = compile_python_function(OVERFLOW_SOURCE, "overflow_loop").unwrap();
    let mut vm = Vm::with_hot_threshold(0);
    for _ in 0..3 {
        vm.execute_with_args(&executable, &[Value::SmallInt(i64::MAX)])
            .unwrap();
    }

    // When: the optimized loop crosses the small-integer boundary.
    let result = vm
        .execute_with_args(&executable, &[Value::SmallInt(i64::MAX)])
        .unwrap();

    // Then: semantic replay preserves the exact arbitrary-precision result.
    let Value::Object(reference) = result.value else {
        panic!("overflow must promote to a BigInt object")
    };
    assert_eq!(
        vm.object(reference).unwrap(),
        &Object::BigInt(BigInt::from(i64::MAX) + 2)
    );
    assert_eq!(vm.jit_report().native_executions, 1);
    assert_eq!(
        vm.jit_report().last_exit_kind,
        Some(WxExitKind::ReplayInstruction)
    );
}

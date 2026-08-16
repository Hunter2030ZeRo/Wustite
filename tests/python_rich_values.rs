use num_bigint::BigInt;
use wustite::object::ObjectKind;
use wustite::structure_map::{SlotType, TypeFact};
use wustite::{ExecutionMode, Object, Runtime, RuntimeConfig, RuntimeValue};

const FLOAT_SOURCE: &str = r#"def add(left: float, right: float):
    return left + right
"#;

const BOOLEAN_SOURCE: &str = r#"def expression(value: bool):
    return not False and value
"#;

const STRING_FACTORY_SOURCE: &str = r#"def main():
    return "wustite"
"#;

const STRING_ECHO_SOURCE: &str = r#"def echo(value: str):
    return value
"#;

const BIG_INTEGER_SOURCE: &str = r#"def main():
    return 9223372036854775808 + 1
"#;

const BIG_INTEGER_ECHO_SOURCE: &str = r#"def echo(value: BigInt):
    return value
"#;

const INTEGER_DIVISION_SOURCE: &str = r#"def divide(left: int, right: int):
    return left / right
"#;

const TUPLE_INDEX_SOURCE: &str = r#"def main():
    return (17, 23)[1]
"#;

const LIST_FACTORY_SOURCE: &str = r#"def main():
    return [17, 23, 31]
"#;

const LIST_LENGTH_SOURCE: &str = r#"def length(values: list):
    return len(values)
"#;

const DICT_FACTORY_SOURCE: &str = r#"def main():
    return {"answer": 42}
"#;

const DICT_INDEX_SOURCE: &str = r#"def answer(values: dict):
    return values["answer"]
"#;

const FUNCTION_FACTORY_SOURCE: &str = r#"def identity(value: int):
    return value

def current_function():
    return identity
"#;

const FUNCTION_CALL_SOURCE: &str = r#"def call(callback: function, value: int):
    return callback(value)
"#;

fn interpreter_runtime() -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    })
}

#[test]
fn float_arithmetic_returns_a_float_in_interpreter_mode() {
    // Given: a Python function with explicitly typed float arguments.
    let mut runtime = interpreter_runtime();
    let executable = runtime.compile_function(FLOAT_SOURCE, "add").unwrap();

    // When: the interpreter evaluates floating-point addition.
    let result = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::Float(1.5), RuntimeValue::Float(2.25)],
        )
        .unwrap();

    // Then: the public runtime result preserves the floating-point value.
    assert_eq!(result, RuntimeValue::Float(3.75));
}

#[test]
fn boolean_not_and_and_return_a_boolean_in_interpreter_mode() {
    // Given: a Python function with an explicitly typed Boolean argument.
    let mut runtime = interpreter_runtime();
    let executable = runtime
        .compile_function(BOOLEAN_SOURCE, "expression")
        .unwrap();

    // When: the interpreter evaluates `not False and True`.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::Bool(true)])
        .unwrap();

    // Then: the expression is returned through the public Boolean variant.
    assert_eq!(result, RuntimeValue::Bool(true));
}

#[test]
fn string_values_round_trip_through_a_typed_argument_in_interpreter_mode() {
    // Given: a string literal and a Python function with an explicitly typed str argument.
    let mut runtime = interpreter_runtime();
    let string = runtime.run_function(STRING_FACTORY_SOURCE, "main").unwrap();
    let RuntimeValue::Object(string) = string else {
        panic!("string literal must be represented by an object reference");
    };
    let executable = runtime
        .compile_function(STRING_ECHO_SOURCE, "echo")
        .unwrap();

    // When: the interpreter returns the string object supplied as an argument.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::Object(string)])
        .unwrap();

    // Then: the returned object is the original string value.
    let RuntimeValue::Object(result) = result else {
        panic!("string result must be represented by an object reference");
    };
    assert!(matches!(
        runtime.object(result).unwrap(),
        Object::String(value) if value == "wustite"
    ));
}

#[test]
fn integers_beyond_i64_return_a_big_integer_object_in_interpreter_mode() {
    // Given: a Python expression whose result exceeds the signed 64-bit range.
    let mut runtime = interpreter_runtime();

    // When: the interpreter evaluates the large integer expression.
    let result = runtime.run_function(BIG_INTEGER_SOURCE, "main").unwrap();

    // Then: the public result identifies the value as a BigInt object.
    let RuntimeValue::Object(result) = result else {
        panic!("large integer result must be represented by an object reference");
    };
    assert!(matches!(runtime.object(result).unwrap(), Object::BigInt(_)));
}

#[test]
fn bigint_annotations_accept_heap_objects_through_the_execution_abi() {
    // Given: a BigInt-annotated function and a runtime-owned arbitrary-size integer.
    let mut runtime = interpreter_runtime();
    let executable = runtime
        .compile_function(BIG_INTEGER_ECHO_SOURCE, "echo")
        .unwrap();
    let value = runtime
        .allocate_object(Object::BigInt(BigInt::from(i64::MAX) + 1))
        .unwrap();

    // When: the object crosses the typed positional execution ABI.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::Object(value)])
        .unwrap();

    // Then: the annotation maps to BigInt and the same heap value is returned.
    assert_eq!(
        executable.parameters()[0].ty,
        SlotType::Object(ObjectKind::BigInt)
    );
    assert_eq!(result, RuntimeValue::Object(value));
}

#[test]
fn integer_division_returns_float_and_records_float_result_metadata() {
    // Given: integer operands for Python's true-division operator.
    let mut runtime = interpreter_runtime();
    let executable = runtime
        .compile_function(INTEGER_DIVISION_SOURCE, "divide")
        .unwrap();

    // When: the interpreter evaluates the division.
    let result = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::SmallInt(5), RuntimeValue::SmallInt(2)],
        )
        .unwrap();

    // Then: both the value and immutable operation metadata identify a float.
    assert_eq!(result, RuntimeValue::Float(2.5));
    assert_eq!(
        executable
            .structure_map()
            .operation_site(wustite::structure_map::OperationSiteId(0))
            .unwrap()
            .result,
        TypeFact::Exact(SlotType::Float)
    );
}

#[test]
fn tuple_literals_support_indexing_in_interpreter_mode() {
    // Given: a Python tuple literal with an indexed element.
    let mut runtime = interpreter_runtime();

    // When: the interpreter evaluates the tuple subscript.
    let result = runtime.run_function(TUPLE_INDEX_SOURCE, "main").unwrap();

    // Then: indexing returns the selected small integer value.
    assert_eq!(result, RuntimeValue::SmallInt(23));
}

#[test]
fn list_literals_support_len_through_typed_arguments_in_interpreter_mode() {
    // Given: a list literal and a Python function with an explicitly typed list argument.
    let mut runtime = interpreter_runtime();
    let list = runtime.run_function(LIST_FACTORY_SOURCE, "main").unwrap();
    let RuntimeValue::Object(list) = list else {
        panic!("list literal must be represented by an object reference");
    };
    let executable = runtime
        .compile_function(LIST_LENGTH_SOURCE, "length")
        .unwrap();

    // When: the interpreter evaluates len for the list argument.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::Object(list)])
        .unwrap();

    // Then: len returns the number of literal elements as a small integer.
    assert_eq!(result, RuntimeValue::SmallInt(3));
}

#[test]
fn dictionary_literals_support_indexing_through_typed_arguments_in_interpreter_mode() {
    // Given: a dictionary literal and a Python function with an explicitly typed dict argument.
    let mut runtime = interpreter_runtime();
    let dictionary = runtime.run_function(DICT_FACTORY_SOURCE, "main").unwrap();
    let RuntimeValue::Object(dictionary) = dictionary else {
        panic!("dictionary literal must be represented by an object reference");
    };
    let executable = runtime
        .compile_function(DICT_INDEX_SOURCE, "answer")
        .unwrap();

    // When: the interpreter indexes the dictionary argument by its literal key.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::Object(dictionary)])
        .unwrap();

    // Then: dictionary indexing returns the associated small integer value.
    assert_eq!(result, RuntimeValue::SmallInt(42));
}

#[test]
fn closureless_function_values_can_be_called_through_typed_arguments_in_interpreter_mode() {
    // Given: a closureless Python function value and a function-typed callback parameter.
    let mut runtime = interpreter_runtime();
    let function = runtime
        .run_function(FUNCTION_FACTORY_SOURCE, "current_function")
        .unwrap();
    let RuntimeValue::Object(function) = function else {
        panic!("function values must be represented by object references");
    };
    assert!(matches!(
        runtime.object(function).unwrap(),
        Object::Function(_)
    ));
    let executable = runtime
        .compile_function(FUNCTION_CALL_SOURCE, "call")
        .unwrap();

    // When: the interpreter calls the function object passed to the typed callback parameter.
    let result = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::Object(function), RuntimeValue::SmallInt(42)],
        )
        .unwrap();

    // Then: the callback returns its integer argument through the public small-integer variant.
    assert_eq!(result, RuntimeValue::SmallInt(42));
}

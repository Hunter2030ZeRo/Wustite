use wustite::{ExecutionMode, Object, Runtime, RuntimeConfig, RuntimeValue};

const MUTATE_AND_LENGTH: &str = r#"
def main(values: list):
    values.append(42)
    return len(values)
"#;

const MUTATE_THROUGH_CALL: &str = r#"
def mutate(values: list):
    values.append(9)
    return 0

def main(values: list):
    values.append(1)
    mutate(values)
    return values[1] + len(values)
"#;

#[test]
fn adaptive_write_visible_to_wvm_and_public_api() {
    // Given: a host-owned public list passed into an adaptive-v2 function.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let list = runtime.allocate_object(Object::list(Vec::new())).unwrap();
    let executable = runtime.compile_function(MUTATE_AND_LENGTH, "main").unwrap();

    // When: an adaptive-supported append is followed by WVM-only length.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::Object(list)])
        .unwrap();

    // Then: both consumers observe one authoritative mutation.
    assert_eq!(result, RuntimeValue::SmallInt(1));
    let Object::List(values) = runtime.object(list).unwrap() else {
        panic!("host object must remain a list");
    };
    assert_eq!(values.len(), 1);
}

#[test]
fn aliasing_call_returns_ownership_pre_mutation() {
    // Given: a host list first mapped and mutated by the adaptive adapter.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let list = runtime.allocate_object(Object::list(Vec::new())).unwrap();
    let executable = runtime
        .compile_function(MUTATE_THROUGH_CALL, "main")
        .unwrap();

    // When: an unsupported guest call aliases and mutates that same public list.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::Object(list)])
        .unwrap();

    // Then: the prior operation already handed ownership back, so the call sees no shadow binding.
    assert_eq!(result, RuntimeValue::SmallInt(11));
    let Object::List(values) = runtime.object(list).unwrap() else {
        panic!("host object must remain a list");
    };
    assert_eq!(values.len(), 2);
    let report = runtime.last_adaptive_report().expect("adaptive report");
    assert_eq!(report.invalidations, 0, "{report:?}");
}

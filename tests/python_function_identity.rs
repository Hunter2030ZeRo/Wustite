use wustite::executable::ExecutableConstant;
use wustite::frontend::python::compile_python_function;
use wustite::value::Value;
use wustite::wvm::Vm;

const SHADOWING_SOURCE: &str = r#"def helper():
    return 99

def parameter_shadow(helper: int):
    return helper

def current_function_parameter_shadow(current_function_parameter_shadow: int):
    return current_function_parameter_shadow

def local_shadow():
    helper = 7
    return helper
"#;

const HELPER_IDENTITY_SOURCE: &str = r#"def increment(value: int):
    return value + 1

def main():
    first = increment
    second = increment
    return first == second and first(41) == 42 and second(99) == 100
"#;

const FUTURE_LOCAL_SOURCE: &str = r#"def helper():
    return 99

def main():
    value = helper
    helper = 7
    return value
"#;

const CYCLE_SOURCE: &str = r#"def main():
    return helper

def helper():
    return main
"#;

#[test]
fn initialized_params_locals_shadow_module_fn_names() {
    // Given: top-level functions whose names are also parameter and local names.
    let parameter = compile_python_function(SHADOWING_SOURCE, "parameter_shadow").unwrap();
    let current =
        compile_python_function(SHADOWING_SOURCE, "current_function_parameter_shadow").unwrap();
    let local = compile_python_function(SHADOWING_SOURCE, "local_shadow").unwrap();
    let mut vm = Vm::with_hot_threshold(u64::MAX);

    // When: each function resolves an initialized name in its own lexical scope.
    let parameter_result = vm
        .execute_with_args(&parameter, &[Value::SmallInt(17)])
        .unwrap()
        .value;
    let current_result = vm
        .execute_with_args(&current, &[Value::SmallInt(23)])
        .unwrap()
        .value;
    let local_result = vm.execute(&local).unwrap().value;

    // Then: initialized bindings win over both top-level and current function values.
    assert_eq!(parameter_result, Value::SmallInt(17));
    assert_eq!(current_result, Value::SmallInt(23));
    assert_eq!(local_result, Value::SmallInt(7));
}

#[test]
fn helper_refs_keep_identity_and_callability() {
    // Given: one helper referenced twice by a compiled top-level function.
    let executable = compile_python_function(HELPER_IDENTITY_SOURCE, "main").unwrap();
    let mut vm = Vm::with_hot_threshold(u64::MAX);

    // When: the function compares and invokes both helper values.
    let helper_ids: Vec<_> = executable
        .constants()
        .iter()
        .filter_map(|constant| match constant {
            ExecutableConstant::Function(function) => Some(function.id()),
            ExecutableConstant::String(_)
            | ExecutableConstant::BigInt(_)
            | ExecutableConstant::Class(_) => None,
        })
        .collect();
    let result = vm.execute(&executable).unwrap().value;

    // Then: both values have the same function identity and each call succeeds.
    assert_eq!(helper_ids.len(), 2);
    assert_eq!(helper_ids[0], helper_ids[1]);
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn later_local_assignment_blocks_module_fn_resolution() {
    // Given: a helper name which is assigned locally after its first use.

    // When: the compiler lowers the use before local initialization.
    let result = compile_python_function(FUTURE_LOCAL_SOURCE, "main");

    // Then: lexical local resolution reports the use-before-assignment error.
    let Err(error) = result else {
        panic!("a future local assignment must not resolve the top-level helper");
    };
    assert!(error.message().contains("name `helper` is not initialized"));
}

#[test]
fn function_reference_cycles_remain_rejected() {
    // Given: two unresolved top-level functions that reference one another.

    // When: compilation recursively resolves the function values.
    let result = compile_python_function(CYCLE_SOURCE, "main");

    // Then: the existing cycle guard still rejects the unsupported closure.
    let Err(error) = result else {
        panic!("recursive function reference cycles must remain unsupported");
    };
    assert!(
        error
            .message()
            .contains("recursive function reference cycle")
    );
}

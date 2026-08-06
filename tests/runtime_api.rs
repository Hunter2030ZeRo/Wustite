use wustite::bytecode::{Function, Instruction};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::structure_map::{LiveSlot, RegionId, SlotType, StructureMap};
use wustite::value::Value;
use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeError, RuntimeValue};

const SUM_SOURCE: &str = r#"def main():
    acc = 0
    index = 1
    step = 1
    limit = 101
    while index < limit:
        acc = acc + index
        index = index + step
    return acc
"#;

const ADD_SOURCE: &str = r#"def add(left: int, right: int):
    return left + right
"#;

const IDENTITY_SOURCE: &str = r#"def identity(value: bool):
    return value
"#;

const SUM_TO_SOURCE: &str = r#"def sum_to(limit: int):
    acc = 0
    index = 1
    step = 1
    while index < limit:
        acc = acc + index
        index = index + step
    return acc
"#;

fn adaptive_runtime(hot_threshold: u64) -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold,
    })
}

#[test]
fn run_function_and_interpreter_mode_return_sum() {
    assert_eq!(
        RuntimeConfig::default(),
        RuntimeConfig {
            execution_mode: ExecutionMode::AdaptiveJit,
            hot_threshold: wustite::wvm::DEFAULT_HOT_THRESHOLD,
        }
    );

    let mut adaptive = adaptive_runtime(10);
    assert_eq!(
        adaptive.run_function(SUM_SOURCE, "main").unwrap(),
        RuntimeValue::SmallInt(5050)
    );

    let config = RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    };
    let mut interpreter = Runtime::new(config.clone());
    let executable = interpreter.compile_function(SUM_SOURCE, "main").unwrap();
    assert_eq!(interpreter.config(), &config);
    assert_eq!(
        interpreter.execute(&executable).unwrap(),
        RuntimeValue::SmallInt(5050)
    );
    assert_eq!(interpreter.last_jit_report().compilation_attempts, 0);
    assert_eq!(interpreter.last_jit_report().native_executions, 0);
}

#[test]
fn typed_positional_arguments_cross_the_execution_abi() {
    // Given: a compiled Python function with two typed positional parameters.
    let mut runtime = adaptive_runtime(1);
    let executable = runtime.compile_function(ADD_SOURCE, "add").unwrap();

    // When: host values are passed through the public execution ABI.
    let result = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::SmallInt(20), RuntimeValue::SmallInt(22)],
        )
        .unwrap();

    // Then: parameters describe their register ABI and the function sees both values.
    assert_eq!(result, RuntimeValue::SmallInt(42));
    assert_eq!(
        runtime.inspect(&executable).parameters,
        vec![
            ExecutableParameter {
                name: "left".to_string(),
                register: 0,
                ty: SlotType::SmallInt,
            },
            ExecutableParameter {
                name: "right".to_string(),
                register: 1,
                ty: SlotType::SmallInt,
            },
        ]
    );
}

#[test]
fn execution_abi_rejects_wrong_arity() {
    // Given: a function that requires two positional arguments.
    let mut runtime = adaptive_runtime(1);
    let executable = runtime.compile_function(ADD_SOURCE, "add").unwrap();

    // When: the host supplies only one argument.
    let error = runtime
        .execute_with_args(&executable, &[RuntimeValue::SmallInt(20)])
        .unwrap_err();

    // Then: execution stops at the ABI boundary with an arity error.
    assert!(
        matches!(error, RuntimeError::Execution(message) if message.contains("expected 2 arguments, got 1"))
    );
}

#[test]
fn execution_abi_rejects_wrong_argument_type() {
    // Given: a function whose first positional argument is an integer.
    let mut runtime = adaptive_runtime(1);
    let executable = runtime.compile_function(ADD_SOURCE, "add").unwrap();

    // When: the host supplies a Boolean in that position.
    let error = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::Bool(true), RuntimeValue::SmallInt(22)],
        )
        .unwrap_err();

    // Then: execution stops at the ABI boundary with the parameter name and type.
    assert!(
        matches!(error, RuntimeError::Execution(message) if message.contains("argument 0 `left`") && message.contains("expected small_int, got bool"))
    );
}

#[test]
fn run_function_with_args_accepts_boolean_parameters() {
    // Given: a source function with one Boolean positional parameter.
    let mut runtime = adaptive_runtime(1);

    // When: compilation and execution use the convenience ABI in one call.
    let result = runtime
        .run_function_with_args(IDENTITY_SOURCE, "identity", &[RuntimeValue::Bool(true)])
        .unwrap();

    // Then: the Boolean value crosses both public runtime boundaries unchanged.
    assert_eq!(result, RuntimeValue::Bool(true));
}

#[test]
fn function_arguments_remain_live_across_native_region_execution() {
    // Given: a loop function whose annotated parameter is live at the JIT entry.
    let mut runtime = adaptive_runtime(1);
    let executable = runtime.compile_function(SUM_TO_SOURCE, "sum_to").unwrap();

    // When: execution tiers up while using the host-provided loop bound.
    let result = runtime
        .execute_with_args(&executable, &[RuntimeValue::SmallInt(101)])
        .unwrap();

    // Then: native execution observes the parameter and returns the expected sum.
    assert_eq!(result, RuntimeValue::SmallInt(5050));
    assert_eq!(runtime.last_jit_report().native_executions, 1);
}

#[test]
fn repeated_execute_and_clone_reuse_native_region() {
    let mut runtime = adaptive_runtime(10);
    let executable = runtime.compile_function(SUM_SOURCE, "main").unwrap();

    assert_eq!(
        runtime.execute(&executable).unwrap(),
        RuntimeValue::SmallInt(5050)
    );
    assert_eq!(runtime.last_jit_report().compilation_attempts, 1);
    assert_eq!(runtime.last_jit_report().compiled_regions, 1);

    assert_eq!(
        runtime.execute(&executable).unwrap(),
        RuntimeValue::SmallInt(5050)
    );
    assert_eq!(runtime.last_jit_report().compilation_attempts, 0);
    assert_eq!(runtime.last_jit_report().compiled_regions, 0);
    assert_eq!(runtime.last_jit_report().native_executions, 1);

    let clone = executable.clone();
    assert_eq!(clone.id(), executable.id());
    assert_eq!(
        runtime.execute(&clone).unwrap(),
        RuntimeValue::SmallInt(5050)
    );
    assert_eq!(runtime.last_jit_report().compilation_attempts, 0);
    assert_eq!(runtime.last_jit_report().native_executions, 1);
}

#[test]
fn different_executables_keep_independent_persistent_runtimes() {
    let mut runtime = adaptive_runtime(5);
    let executable_a = runtime.compile_function(SUM_SOURCE, "main").unwrap();
    let source_b = SUM_SOURCE.replace("limit = 101", "limit = 11");
    let executable_b = runtime.compile_function(&source_b, "main").unwrap();
    assert_ne!(executable_a.id(), executable_b.id());

    assert_eq!(
        runtime.execute(&executable_a).unwrap(),
        RuntimeValue::SmallInt(5050)
    );
    assert_eq!(
        runtime.execute(&executable_b).unwrap(),
        RuntimeValue::SmallInt(55)
    );
    let b_entries = runtime
        .profile_for(&executable_b)
        .unwrap()
        .entry_count(RegionId(0));
    let a_entries = runtime
        .profile_for(&executable_a)
        .unwrap()
        .entry_count(RegionId(0));

    assert_eq!(
        runtime.execute(&executable_a).unwrap(),
        RuntimeValue::SmallInt(5050)
    );
    assert_eq!(runtime.last_jit_report().compilation_attempts, 0);
    assert_eq!(runtime.last_jit_report().native_executions, 1);
    assert!(
        runtime
            .profile_for(&executable_a)
            .unwrap()
            .entry_count(RegionId(0))
            > a_entries
    );
    assert_eq!(
        runtime
            .profile_for(&executable_b)
            .unwrap()
            .entry_count(RegionId(0)),
        b_entries
    );
}

#[test]
fn errors_preserve_frontend_locations_and_cached_executables() {
    let mut runtime = adaptive_runtime(5);
    let frontend_error = runtime
        .compile_function(
            "def main():\n    if 1 < 2:\n        return 1\n    return 0\n",
            "main",
        )
        .err()
        .unwrap();
    let RuntimeError::Frontend(frontend_error) = frontend_error else {
        panic!("expected frontend error");
    };
    assert_eq!(frontend_error.location().unwrap().line, 2);

    let valid = runtime.compile_function(SUM_SOURCE, "main").unwrap();
    assert_eq!(
        runtime.execute(&valid).unwrap(),
        RuntimeValue::SmallInt(5050)
    );
    let invalid = ExecutableFunction::new(
        Function {
            register_count: 0,
            code: vec![Instruction::Return { src: 0 }],
        },
        StructureMap::default(),
    );
    assert!(matches!(
        runtime.execute(&invalid),
        Err(RuntimeError::Execution(_))
    ));
    assert!(runtime.profile_for(&invalid).is_none());

    assert_eq!(
        runtime.execute(&valid).unwrap(),
        RuntimeValue::SmallInt(5050)
    );
    assert_eq!(runtime.last_jit_report().compilation_attempts, 0);
    assert_eq!(runtime.last_jit_report().native_executions, 1);
}

#[test]
fn runtime_value_conversion_rejects_uninitialized_values() {
    assert_eq!(
        RuntimeValue::try_from(Value::SmallInt(42)).unwrap(),
        RuntimeValue::SmallInt(42)
    );
    assert_eq!(
        RuntimeValue::try_from(Value::Bool(true)).unwrap(),
        RuntimeValue::Bool(true)
    );
    assert!(matches!(
        RuntimeValue::try_from(Value::Uninitialized),
        Err(RuntimeError::InvalidResult(_))
    ));
}

#[test]
fn inspect_is_deterministic_and_has_no_runtime_side_effects() {
    let mut runtime = adaptive_runtime(1);
    let executable = runtime.compile_function(SUM_SOURCE, "main").unwrap();
    assert!(runtime.profile_for(&executable).is_none());

    let info = runtime.inspect(&executable);
    assert_eq!(info.id, executable.id());
    assert_eq!(info.register_count, 11);
    assert_eq!(info.instruction_count, 16);
    assert_eq!(info.regions.len(), 1);
    assert_eq!(info.regions[0].id, RegionId(0));
    assert_eq!(info.regions[0].header, 8);
    assert_eq!(info.regions[0].backedge, 14);
    assert_eq!(info.regions[0].exits, vec![15]);
    assert_eq!(
        info.regions[0].live_slots,
        vec![
            LiveSlot {
                register: 0,
                ty: SlotType::SmallInt,
            },
            LiveSlot {
                register: 2,
                ty: SlotType::SmallInt,
            },
            LiveSlot {
                register: 4,
                ty: SlotType::SmallInt,
            },
            LiveSlot {
                register: 6,
                ty: SlotType::SmallInt,
            },
        ]
    );
    assert!(runtime.profile_for(&executable).is_none());
    assert_eq!(runtime.last_jit_report().compilation_attempts, 0);
    assert_eq!(runtime.last_jit_report().native_executions, 0);
}

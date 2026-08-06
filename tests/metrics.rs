use wustite::runtime::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const SUM_SOURCE: &str = include_str!("../examples/sum.py");

#[test]
fn measured_execution_preserves_jit_reuse() {
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 10,
    });

    let compilation = runtime
        .compile_function_measured(SUM_SOURCE, "main")
        .unwrap();

    let first = runtime.execute_measured(&compilation.executable).unwrap();

    let second = runtime.execute_measured(&compilation.executable).unwrap();

    assert_eq!(first.value, RuntimeValue::SmallInt(5050));
    assert_eq!(second.value, RuntimeValue::SmallInt(5050));

    assert_eq!(first.jit_report.compilation_attempts, 1);
    assert_eq!(first.jit_report.compiled_regions, 1);

    assert_eq!(second.jit_report.compilation_attempts, 0);
    assert_eq!(second.jit_report.compiled_regions, 0);
    assert_eq!(second.jit_report.native_executions, 1);
}

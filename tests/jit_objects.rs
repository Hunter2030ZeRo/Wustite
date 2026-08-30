use wustite::{CompilerBackend, ExecutionMode, Runtime, RuntimeConfig, RuntimeError, RuntimeValue};

const SPECTRAL_NORM_SOURCE: &str = include_str!("../examples/spectral_norm.py");
const FANNKUCH_SOURCE: &str = include_str!("../examples/fannkuch.py");
const NBODY_SOURCE: &str = include_str!("../examples/nbody.py");
const COMPILER_KERNELS_SOURCE: &str = include_str!("../benchmarks/compiler_kernels.py");

fn cranelift_runtime() -> Runtime {
    Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Jit(CompilerBackend::Cranelift),
        hot_threshold: 1,
    })
}

fn assert_native_execution(runtime: &Runtime) {
    let report = runtime.last_jit_report();
    assert!(report.compiled_regions > 0, "JIT report: {report:?}");
    assert!(report.native_executions > 0, "JIT report: {report:?}");
}

#[test]
fn object_loop_stays_inside_native() {
    // Given: a list-indexing loop and the Cranelift runtime with immediate tier-up.
    let source = "def main():\n    values = [1, 2, 3, 4]\n    total = 0\n    for index in range(200):\n        total += values[index // 50]\n    return total\n";
    let mut runtime = cranelift_runtime();

    // When: the public runtime executes the object-heavy loop.
    let result = runtime.run_function(source, "main");

    // Then: list reads remain correct and execute through compiled native code.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(500))));
    assert_native_execution(&runtime);
    assert_eq!(
        runtime.last_jit_report().helper_calls.get_item,
        0,
        "stable typed list reads should use the borrowed sequence view"
    );
}

#[test]
fn list_mutation_stays_inside_native() {
    // Given: repeated list reads and writes in a loop with immediate tier-up.
    let source = "def main():\n    values = [0]\n    for index in range(200):\n        values[0] = values[0] + 1\n    return values[0]\n";
    let mut runtime = cranelift_runtime();

    // When: the public runtime executes the mutating loop.
    let result = runtime.run_function(source, "main");

    // Then: native object mutation preserves the shared list state.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(200))));
    assert_native_execution(&runtime);
}

#[test]
fn native_object_error_replays_at_failing_instruction() {
    // Given: an invalid list pop inside a loop selected for native execution.
    let source = "def main():\n    values = []\n    for index in range(2):\n        values.pop()\n    return 0\n";
    let mut runtime = cranelift_runtime();

    // When: the compiled region reaches the invalid mutation.
    let result = runtime.run_function(source, "main");

    // Then: the interpreter replay preserves the exact WVM error.
    assert!(matches!(
        result,
        Err(RuntimeError::Execution(message)) if message == "sequence index out of range"
    ));
}

#[test]
fn spectral_norm_runs_object_regions_native() {
    // Given: the spectral-norm implementation benchmark and immediate Cranelift tier-up.
    let mut runtime = cranelift_runtime();

    // When: the benchmark executes through the public runtime.
    let result = runtime.run_function(SPECTRAL_NORM_SOURCE, "main");

    // Then: its reference result is preserved and at least one object region ran natively.
    assert!(matches!(result, Ok(RuntimeValue::Float(value)) if (1.62..1.63).contains(&value)));
    assert_native_execution(&runtime);
    let native_calls = &runtime.last_jit_report().native_calls;
    assert!(native_calls.get("part_A_times_u").copied().unwrap_or(0) > 2_500);
    assert!(native_calls.get("part_At_times_u").copied().unwrap_or(0) > 2_500);
}

#[test]
fn spectral_norm_reprofiles_numeric_entries() {
    // Given: spectral_norm with enough profiling iterations to observe its int-to-float transition.
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Jit(CompilerBackend::Cranelift),
        hot_threshold: 10,
    });

    // When: both matrix-vector helpers repeatedly enter their compiled inner loops.
    let result = runtime.run_function(SPECTRAL_NORM_SOURCE, "main");

    // Then: transient entry types do not permanently disable either stable float region.
    assert!(matches!(result, Ok(RuntimeValue::Float(value)) if (1.62..1.63).contains(&value)));
    let report = runtime.last_jit_report();
    assert!(report.failures.is_empty(), "JIT report: {report:?}");
    assert!(
        report
            .native_calls
            .get("part_A_times_u")
            .copied()
            .unwrap_or(0)
            > 2_500,
        "JIT report: {report:?}"
    );
    assert!(
        report
            .native_calls
            .get("part_At_times_u")
            .copied()
            .unwrap_or(0)
            > 2_500,
        "JIT report: {report:?}"
    );
}

#[test]
fn branch_joins_compile_without_dead_temps() {
    // Given: nested compiler kernels whose diamond branches define distinct temporaries.
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Jit(CompilerBackend::Cranelift),
        hot_threshold: 10,
    });

    // When: the benchmark executes through profiled WXIR compilation.
    let result = runtime.run_function(COMPILER_KERNELS_SOURCE, "main");

    // Then: its result is correct and both nested loop regions compile and execute natively.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(2755))));
    let report = runtime.last_jit_report();
    assert_eq!(report.compiled_regions, 2, "JIT report: {report:?}");
    assert!(report.native_executions > 0, "JIT report: {report:?}");
    assert!(report.failures.is_empty(), "JIT report: {report:?}");
}

#[test]
fn fannkuch_executes_object_regions_natively() {
    // Given: the fannkuch implementation benchmark and immediate Cranelift tier-up.
    let mut runtime = cranelift_runtime();

    // When: the benchmark executes through the public runtime.
    let result = runtime.run_function(FANNKUCH_SOURCE, "main");

    // Then: its reference result is preserved and list-mutation regions ran natively.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(30))));
    assert_native_execution(&runtime);
    assert!(
        runtime.last_jit_report().failures.is_empty(),
        "JIT report: {:?}",
        runtime.last_jit_report()
    );
}

#[test]
fn nbody_executes_object_regions_natively() {
    // Given: the nbody implementation benchmark and immediate Cranelift tier-up.
    let mut runtime = cranelift_runtime();

    // When: the benchmark executes through the public runtime.
    let result = runtime.run_function(NBODY_SOURCE, "main");

    // Then: its reference energy is preserved and object-mutation regions ran natively.
    assert!(
        matches!(result, Ok(RuntimeValue::Float(value)) if (-0.1691..-0.1690).contains(&value))
    );
    assert_native_execution(&runtime);
}

#[cfg(feature = "inkwell")]
#[test]
fn llvm_executes_list_indexing_inside_native() {
    // Given: a list-indexing loop and direct LLVM compilation.
    let source = "def main():\n    values = [1, 2, 3, 4]\n    total = 0\n    for index in range(200):\n        total += values[index // 50]\n    return total\n";
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Jit(CompilerBackend::Llvm),
        hot_threshold: 1,
    });

    // When: the object-heavy loop executes through the public runtime.
    let result = runtime.run_function(source, "main");

    // Then: LLVM preserves the value and records Tier-2 native execution.
    assert!(matches!(result, Ok(RuntimeValue::SmallInt(500))));
    let report = runtime.last_jit_report();
    assert!(report.tier2_compiled_regions > 0, "JIT report: {report:?}");
    assert!(report.tier2_native_executions > 0, "JIT report: {report:?}");
}

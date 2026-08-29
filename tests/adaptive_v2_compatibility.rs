use wustite::jit::{CompileError, CompiledRegion, CraneliftRegionCompiler, RegionCompiler};
use wustite::object::{Object, ObjectKind};
use wustite::value::Value;
use wustite::wxir::WxFunction;
use wustite::{CompilerBackend, ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const LOOP_SOURCE: &str = r#"def main():
    total = 0
    index = 0
    while index < 8:
        total = total + index
        index = index + 1
    return total
"#;

#[test]
fn public_runtime_config_and_values_keep_legacy_round_trips() {
    // Given: the v1 configuration and public value variants.
    let default_config = RuntimeConfig::default();
    let values = [
        RuntimeValue::SmallInt(-7),
        RuntimeValue::Float(2.5),
        RuntimeValue::Bool(true),
        RuntimeValue::None,
    ];

    // When: callers configure and cross the stable runtime-value boundary.
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 0,
    });

    // Then: the legacy default, modes, and value conversions stay observable.
    assert_eq!(
        default_config,
        RuntimeConfig {
            execution_mode: ExecutionMode::Jit(CompilerBackend::Tiered),
            hot_threshold: wustite::wvm::DEFAULT_HOT_THRESHOLD,
        }
    );
    for value in values {
        let internal: Value = value.into();
        assert_eq!(RuntimeValue::try_from(internal).unwrap(), value);
    }
    assert!(RuntimeValue::try_from(Value::Uninitialized).is_err());

    let reference = runtime
        .allocate_object(Object::String("compatibility".to_owned()))
        .unwrap();
    assert_eq!(runtime.object_kind(reference).unwrap(), ObjectKind::String);
    let internal: Value = RuntimeValue::Object(reference).into();
    assert_eq!(internal, Value::Object(reference));
    assert_eq!(
        RuntimeValue::try_from(internal).unwrap(),
        RuntimeValue::Object(reference)
    );

    let mut adaptive = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    assert_eq!(
        adaptive.run_function(LOOP_SOURCE, "main").unwrap(),
        RuntimeValue::SmallInt(28)
    );
    assert!(adaptive.last_jit_report().native_executions >= 1);
}

#[test]
fn legacy_wxir_compiler_signature_and_jit_report_remain_usable() {
    // Given: the established compiler trait signature and a hot loop.
    let compile: fn(
        &mut CraneliftRegionCompiler,
        &WxFunction,
    ) -> Result<CompiledRegion, CompileError> = |compiler, function| compiler.compile(function);
    let mut runtime = Runtime::new(RuntimeConfig {
        execution_mode: ExecutionMode::Jit(CompilerBackend::Cranelift),
        hot_threshold: 1,
    });

    // When: the public facade executes the loop through the legacy JIT path.
    let value = runtime.run_function(LOOP_SOURCE, "main").unwrap();
    let report = runtime.last_jit_report();

    // Then: WXIR compilation remains callable and the report fields retain meaning.
    assert_eq!(value, RuntimeValue::SmallInt(28));
    assert!(report.compilation_attempts >= 1, "{report:?}");
    assert!(report.compiled_regions >= 1, "{report:?}");
    assert!(report.native_executions >= 1, "{report:?}");
    assert_eq!(report.last_exit_kind_name(), Some("region_exit"));
    assert_eq!(report.exits.region_exit, report.native_executions);
    let _ = compile;
}

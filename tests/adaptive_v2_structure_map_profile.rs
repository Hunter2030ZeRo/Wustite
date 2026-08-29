use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const ADD: &str = r#"
def main(left: int, right: int):
    return left + right
"#;

#[test]
fn verified_structure_facts_classify_live_entry_observations_without_granting_readiness() {
    // Given: an adaptive entry whose parameter provenance and integer types are statically known.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(ADD, "main").unwrap();

    // When: fewer than the mandatory 64 live entries confirm the static facts.
    for _ in 0..63 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(20), RuntimeValue::SmallInt(22)],
                )
                .unwrap(),
            RuntimeValue::SmallInt(42)
        );
    }

    // Then: StructureMap reduced classification work but did not authorize recording or native code.
    let report = runtime.last_adaptive_report().unwrap();
    assert_eq!(report.readiness.live, 63, "{report:?}");
    assert_eq!(report.readiness.static_analysis, 0, "{report:?}");
    assert_eq!(report.static_fact_matches, 126, "{report:?}");
    assert_eq!(report.traces, 0, "{report:?}");
    assert_eq!(report.machine_entries, 0, "{report:?}");
}

#[test]
fn structure_facts_preserve_both_live_stability_windows() {
    // Given: the same verified entry retained across both live profiling windows.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(ADD, "main").unwrap();

    // When: exactly 96 live calls cross 64+32 while compilation still happens after observation.
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(20), RuntimeValue::SmallInt(22)],
                )
                .unwrap(),
            RuntimeValue::SmallInt(42)
        );
    }

    // Then: static facts were consumed, but all readiness came from live executions.
    let report = runtime.last_adaptive_report().unwrap();
    assert_eq!(report.readiness.live, 96, "{report:?}");
    assert_eq!(report.readiness.static_analysis, 0, "{report:?}");
    assert_eq!(report.static_fact_matches, 192, "{report:?}");
    assert_eq!(report.traces, 1, "{report:?}");
    assert_eq!(report.machine_entries, 0, "{report:?}");
}

use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const THRESHOLD: u64 = 10;

const ADD: &str = r#"
def main(left: int, right: int):
    return left + right
"#;

#[test]
fn verified_facts_classify_entry_without_readiness() {
    // Given: an adaptive entry whose parameter provenance and integer types are statically known.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: THRESHOLD,
    });
    let executable = runtime.compile_function(ADD, "main").unwrap();

    // When: fewer than the configured live entries confirm the static facts.
    for _ in 0..THRESHOLD - 1 {
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
    assert_eq!(report.readiness.live, THRESHOLD - 1, "{report:?}");
    assert_eq!(report.readiness.static_analysis, 0, "{report:?}");
    assert_eq!(
        report.static_fact_matches,
        2 * (THRESHOLD - 1),
        "{report:?}"
    );
    assert_eq!(report.traces, 0, "{report:?}");
    assert_eq!(report.machine_entries, 0, "{report:?}");
}

#[test]
fn structure_facts_keep_both_live_stability_windows() {
    // Given: the same verified entry retained across both live profiling windows.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: THRESHOLD,
    });
    let executable = runtime.compile_function(ADD, "main").unwrap();

    // When: exactly two configured windows of live calls pass while compilation still happens after observation.
    for _ in 0..2 * THRESHOLD {
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
    assert_eq!(report.readiness.live, 2 * THRESHOLD, "{report:?}");
    assert_eq!(report.readiness.static_analysis, 0, "{report:?}");
    assert_eq!(report.static_fact_matches, 4 * THRESHOLD, "{report:?}");
    assert_eq!(report.traces, 1, "{report:?}");
    assert_eq!(report.machine_entries, 0, "{report:?}");
}

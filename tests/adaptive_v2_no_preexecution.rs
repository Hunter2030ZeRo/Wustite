use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const EFFECTFUL_SOURCE: &str = r#"
class Counter:
    def __init__(self):
        self.value = 0

    def bump(self):
        self.value = self.value + 1
        return self.value

def main():
    counter = Counter()
    return counter.bump() + counter.bump()
"#;

#[test]
fn effectful_program_skips_preexecution() {
    // Given: a closed parameter-free program whose result depends on ordered object mutations.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(EFFECTFUL_SOURCE, "main").unwrap();

    // When: live execution crosses both entry profiling gates.
    for _ in 0..100 {
        assert_eq!(
            runtime.execute(&executable).unwrap(),
            RuntimeValue::SmallInt(3)
        );
    }

    // Then: only operation-derived sites become native. The effectful entry itself remains
    // unsupported instead of being executed once and replayed as a constant snapshot.
    let report = runtime.last_adaptive_report().unwrap();
    assert!(report.machine_entries > 0, "{report:?}");
    assert_eq!(
        report.machine_entries, report.native_executions,
        "{report:?}"
    );
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert!(report.compile_failure.is_some(), "{report:?}");
    assert!(
        report.regions.iter().any(|region| region.entry_pc == 0
            && region.lifecycle == "recording"
            && region.reason == "unsupported WVM entry"),
        "{report:?}"
    );
}

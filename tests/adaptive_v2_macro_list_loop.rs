use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

#[test]
fn hot_list_loops_enter_native_code_once_per_loop_invocation() {
    // Given: the retained adaptive-v2 runtime has warmed every loop in the public list workload.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime
        .compile_function(
            include_str!("../benchmarks/adaptive_list_objects.py"),
            "main",
        )
        .expect("adaptive list fixture");
    for _ in 0..100 {
        assert_eq!(
            runtime.execute(&executable).expect("warm list execution"),
            RuntimeValue::SmallInt(2_016)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm adaptive report")
        .machine_entries;

    // When: one more complete invocation executes against the compiled loop snapshots.
    let value = runtime
        .execute(&executable)
        .expect("compiled list execution");
    let report = runtime
        .last_adaptive_report()
        .expect("compiled adaptive report");
    let entries = report.machine_entries.saturating_sub(before);

    // Then: guarded preheader/wrapper fusion executes the three loops as one native invocation.
    assert_eq!(value, RuntimeValue::SmallInt(2_016));
    assert_eq!(entries, 1, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

#[test]
fn all_mutating_list_loops_compile_without_constant_replay() {
    const SOURCE: &str = r#"
def main(limit: int, rotations: int):
    values = []
    index = 0
    while index < limit:
        values.append(index)
        index = index + 1
    index = 0
    while index < rotations:
        values.insert(0, values.pop())
        index = index + 1
    total = 0
    index = 0
    while index < limit:
        total = total + values[index] * (index + 1)
        index = index + 1
    return total
"#;
    // Given: two runtime inputs exercise list growth, default pop, clamped insert, and reads.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..100 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(7), RuntimeValue::SmallInt(2)],
                )
                .expect("warm execution"),
            RuntimeValue::SmallInt(77)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("report")
        .machine_entries;

    // When: operation-derived values change without recompiling or replaying constants.
    let value = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::SmallInt(5), RuntimeValue::SmallInt(1)],
        )
        .expect("mutated execution");
    let report = runtime.last_adaptive_report().expect("report");

    // Then: guarded preheader/wrapper fusion preserves all three loops in one native invocation.
    assert_eq!(value, RuntimeValue::SmallInt(30));
    assert_eq!(
        report.machine_entries.saturating_sub(before),
        1,
        "{report:?}"
    );
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

#[test]
fn parameter_free_local_list_pipeline_compiles_from_function_entry() {
    const SOURCE: &str = r#"
def main():
    values = []
    index = 0
    while index < 64:
        values.append(index)
        index = index + 1
    index = 0
    while index < 32:
        values.insert(0, values.pop())
        index = index + 1
    total = 0
    for value in values:
        total = total + value
    return total
"#;
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..100 {
        assert_eq!(
            runtime.execute(&executable).expect("warm execution"),
            RuntimeValue::SmallInt(2016)
        );
    }

    let before = runtime
        .last_adaptive_report()
        .expect("warm report")
        .machine_entries;
    assert_eq!(
        runtime
            .execute(&executable)
            .expect("compiled entry execution"),
        RuntimeValue::SmallInt(2016)
    );
    let report = runtime.last_adaptive_report().expect("report");
    assert_eq!(report.compile_failure, None, "{report:?}");
    assert!(
        report
            .regions
            .iter()
            .any(|region| region.entry_pc == 0 && region.lifecycle == "compiled"),
        "{report:?}"
    );
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.guest_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
    assert_eq!(
        report.machine_entries.saturating_sub(before),
        1,
        "{report:?}"
    );
}

#[test]
fn native_list_indices_preserve_negative_pop_and_insert_clamping() {
    const SOURCE: &str = r#"
def main(limit: int, rotations: int):
    values = []
    index = 0
    while index < limit:
        values.append(index)
        index = index + 1
    moved = 0
    index = 0
    while index < rotations:
        moved = values.pop(-2)
        values.insert(-100, moved)
        values.insert(100, values.pop())
        index = index + 1
    total = 0
    index = 0
    while index < limit:
        total = total + values[index] * (index + 1)
        index = index + 1
    return total
"#;
    // Given: live negative and oversized indices exercise Python list normalization.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..110 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(5), RuntimeValue::SmallInt(2)],
                )
                .expect("warm execution"),
            RuntimeValue::SmallInt(32)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("report")
        .machine_entries;

    // When: the compiled loops repeat with the same runtime-derived contents.
    let value = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::SmallInt(5), RuntimeValue::SmallInt(2)],
        )
        .expect("compiled execution");
    let report = runtime.last_adaptive_report().expect("report");

    // Then: guarded preheader/wrapper fusion keeps list semantics in one native invocation.
    assert_eq!(value, RuntimeValue::SmallInt(32));
    assert_eq!(
        report.machine_entries.saturating_sub(before),
        1,
        "{report:?}"
    );
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

#[test]
fn fannkuch_reverse_prefix_slice_enters_one_native_invocation() {
    const SOURCE: &str = r#"
def main(n: int, rounds: int):
    perm = list(range(n))
    total = 0
    round = 0
    while round < rounds:
        perm[0] = 3
        perm[1] = 2
        perm[2] = 1
        perm[3] = 0
        k = perm[0]
        while k:
            perm[: k + 1] = perm[k::-1]
            k = perm[0]
        total = total + perm[0] * 10000000 + perm[1] * 1000000 + perm[2] * 100000 + perm[3] * 10000 + perm[4] * 1000 + perm[5] * 100 + perm[6] * 10 + perm[7]
        round = round + 1
    return total
"#;
    const ARGS: &[RuntimeValue] = &[RuntimeValue::SmallInt(8), RuntimeValue::SmallInt(80)];
    const EXPECTED: RuntimeValue = RuntimeValue::SmallInt(98_765_360);

    // Given: Fannkuch's reverse-prefix topology is hot without an earlier full-copy slice.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    assert_eq!(
        runtime
            .execute_with_args(&executable, ARGS)
            .expect("warm fannkuch execution"),
        EXPECTED
    );
    let before = runtime
        .last_adaptive_report()
        .expect("warm adaptive report")
        .machine_entries;

    // When: one more complete invocation executes the reverse-prefix loop snapshots.
    let value = runtime
        .execute_with_args(&executable, ARGS)
        .expect("compiled fannkuch execution");
    let report = runtime
        .last_adaptive_report()
        .expect("compiled adaptive report");
    let entries = report.machine_entries.saturating_sub(before);

    // Then: slice reversal remains exact and the two architectural trace units (the outer
    // permutation loop and its nested reversal loop) each enter native code once. The count is
    // bounded by trace topology, not by the eighty reversal rounds.
    assert_eq!(value, EXPECTED);
    assert_eq!(report.compile_failure, None, "{report:?}");
    assert_eq!(entries, 2, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");

    let before_small = report.machine_entries;
    assert_eq!(
        runtime
            .execute_with_args(
                &executable,
                &[RuntimeValue::SmallInt(8), RuntimeValue::SmallInt(8)],
            )
            .expect("short compiled fannkuch execution"),
        RuntimeValue::SmallInt(9_876_536)
    );
    let short_report = runtime
        .last_adaptive_report()
        .expect("short compiled adaptive report");
    assert_eq!(
        short_report.machine_entries.saturating_sub(before_small),
        1,
        "{short_report:?}"
    );
    assert_eq!(short_report.helper_calls, 0, "{short_report:?}");
    assert_eq!(short_report.generic_dispatch_calls, 0, "{short_report:?}");
    assert_eq!(short_report.deopts, 0, "{short_report:?}");
}

#[test]
fn production_fannkuch_rotation_loop_does_not_deopt_per_permutation() {
    // Given: the production Fannkuch function has compiled its rotation/count loop from live
    // observations. This is the real fixture, including the two independently rooted lists.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime
        .compile_function(include_str!("../examples/fannkuch.py"), "fannkuch")
        .expect("production fannkuch fixture");
    assert_eq!(
        runtime
            .execute_with_args(&executable, &[RuntimeValue::SmallInt(7)])
            .expect("warm production fannkuch"),
        RuntimeValue::SmallInt(16)
    );
    let before = runtime
        .last_adaptive_report()
        .map(|report| (report.machine_entries, report.deopts))
        .expect("warm adaptive report");

    // When: another complete invocation uses the retained native loop.
    assert_eq!(
        runtime
            .execute_with_args(&executable, &[RuntimeValue::SmallInt(7)])
            .expect("compiled production fannkuch"),
        RuntimeValue::SmallInt(16)
    );
    let after = runtime
        .last_adaptive_report()
        .expect("compiled adaptive report");

    // Then: native entries are bounded by loop topology and no compiled entry immediately
    // deoptimizes once per permutation.
    let machine_delta = after.machine_entries.saturating_sub(before.0);
    assert!(
        machine_delta <= 8,
        "machine delta={machine_delta}, {after:?}"
    );
    assert_eq!(after.deopts.saturating_sub(before.1), 0, "{after:?}");
    assert_eq!(after.helper_calls, 0, "{after:?}");
    assert_eq!(after.generic_dispatch_calls, 0, "{after:?}");
}

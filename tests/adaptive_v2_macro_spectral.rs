use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

#[test]
fn list_repeat_root_survives_native_loop_with_changed_inputs() {
    const SOURCE: &str = r#"
def main(size: int, seed: int):
    values = [seed] * size
    total = 0
    index = 0
    while index < size:
        total = total + values[index] * (index + 1)
        index = index + 1
    return total
"#;
    // Given: a parameter-derived list repeat has warmed past the real entry threshold.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(5), RuntimeValue::SmallInt(3)],
                )
                .expect("warm execution"),
            RuntimeValue::SmallInt(45)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm report")
        .machine_entries;

    // When: both the repeated value and list length change without recompilation.
    let value = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::SmallInt(7), RuntimeValue::SmallInt(2)],
        )
        .expect("changed execution");
    let report = runtime.last_adaptive_report().expect("changed report");

    // Then: the live Handle root reaches native code and preserves exact semantics.
    assert_eq!(value, RuntimeValue::SmallInt(56));
    assert!(report.machine_entries > before, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

#[test]
fn list_repeat_entry_stays_authoritative_while_float_leaf_enters_native() {
    const SOURCE: &str = r#"
def scale(value: float):
    return value * 1.5

def main(size: int, seed: float):
    values = [seed] * size
    return scale(values[size - 1])
"#;
    // Given: a parameter-derived repeated list feeds a typed floating-point callee.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(4), RuntimeValue::Float(3.0)],
                )
                .expect("warm execution"),
            RuntimeValue::Float(4.5)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm report")
        .machine_entries;

    // When: the repeated value and list extent both change without recompilation.
    let value = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::SmallInt(7), RuntimeValue::Float(2.0)],
        )
        .expect("changed execution");
    let report = runtime.last_adaptive_report().expect("changed report");

    // Then: cold list setup stays authoritative while the verified F64 leaf runs natively.
    assert_eq!(value, RuntimeValue::Float(3.0));
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
fn rooted_float_list_loop_inlines_leaf_and_enters_machine_once() {
    const SOURCE: &str = r#"
def scale(value: float, factor: float):
    return value * factor

def main(size: int, seed: float, factor: float):
    values = [seed] * size
    index = 0
    while index < size:
        values[index] = scale(values[index], factor)
        index = index + 1
    return values[size - 1]
"#;
    // Given: a rooted repeated list and floating-point leaf loop have fully warmed.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[
                        RuntimeValue::SmallInt(5),
                        RuntimeValue::Float(3.0),
                        RuntimeValue::Float(1.5),
                    ],
                )
                .expect("warm execution"),
            RuntimeValue::Float(4.5)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm report")
        .machine_entries;

    // When: list extent, element, and factor all change without recompilation.
    let value = runtime
        .execute_with_args(
            &executable,
            &[
                RuntimeValue::SmallInt(7),
                RuntimeValue::Float(2.0),
                RuntimeValue::Float(2.0),
            ],
        )
        .expect("changed execution");
    let report = runtime.last_adaptive_report().expect("changed report");

    // Then: one helper-free loop entry computes the live operation-derived result.
    assert_eq!(value, RuntimeValue::Float(4.0));
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
fn rooted_float_loop_inlines_altered_signed_floor_leaf_exactly() {
    const SOURCE: &str = r#"
def weighted(index: int, bias: int, item: float):
    return (0.5 + ((index + bias) // 3)) * item

def main(size: int, seed: float, bias: int):
    values = [seed] * size
    total = 0.0
    index = 0
    while index < size:
        total = total + weighted(index, bias, values[index])
        index = index + 1
    return total
"#;
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[
                        RuntimeValue::SmallInt(5),
                        RuntimeValue::Float(2.0),
                        RuntimeValue::SmallInt(-4),
                    ],
                )
                .expect("warm execution"),
            RuntimeValue::Float(-5.0)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm report")
        .machine_entries;

    let value = runtime
        .execute_with_args(
            &executable,
            &[
                RuntimeValue::SmallInt(7),
                RuntimeValue::Float(1.5),
                RuntimeValue::SmallInt(-5),
            ],
        )
        .expect("changed execution");
    let report = runtime.last_adaptive_report().expect("changed report");

    assert_eq!(value, RuntimeValue::Float(-5.25));
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
fn loop_strategy_transition_selects_distinct_integer_and_float_snapshots() {
    const SOURCE: &str = r#"
def main(size: int, seed: object):
    values = [seed] * size
    observed = seed
    index = 0
    while index < size:
        observed = values[index]
        index = index + 1
    return observed
"#;
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(5), RuntimeValue::SmallInt(-7)],
                )
                .expect("integer warm execution"),
            RuntimeValue::SmallInt(-7)
        );
    }
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(5), RuntimeValue::Float(-0.0)],
                )
                .expect("float warm execution"),
            RuntimeValue::Float(-0.0)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("float report")
        .machine_entries;
    let value = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::SmallInt(7), RuntimeValue::SmallInt(11)],
        )
        .expect("integer re-entry");
    let report = runtime
        .last_adaptive_report()
        .expect("integer re-entry report");

    assert_eq!(value, RuntimeValue::SmallInt(11));
    assert_eq!(
        report.machine_entries.saturating_sub(before),
        1,
        "{report:?}"
    );
    assert_eq!(report.deopts, 0, "{report:?}");
    assert!(
        report
            .regions
            .iter()
            .any(|region| region.specialized_cases == 2)
    );
}

#[test]
fn fifth_live_loop_profile_case_becomes_generic_without_compilation_credit() {
    const SOURCE: &str = r#"
def main(size: int, seed: object, marker: object):
    values = [seed] * size
    observed = marker
    index = 0
    while index < size:
        ignored = values[index]
        observed = marker
        index = index + 1
    return observed
"#;
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    let cases = [
        (RuntimeValue::SmallInt(1), RuntimeValue::SmallInt(1)),
        (RuntimeValue::SmallInt(1), RuntimeValue::Float(1.0)),
        (RuntimeValue::SmallInt(1), RuntimeValue::Bool(true)),
        (RuntimeValue::Float(1.0), RuntimeValue::SmallInt(1)),
        (RuntimeValue::Float(1.0), RuntimeValue::Float(1.0)),
    ];
    for (seed, marker) in cases {
        runtime
            .execute_with_args(&executable, &[RuntimeValue::SmallInt(2), seed, marker])
            .expect("live case");
    }
    let report = runtime.last_adaptive_report().expect("generic report");
    assert!(
        report
            .regions
            .iter()
            .any(|region| { region.generic && region.specialized_cases == 5 }),
        "{report:?}"
    );
    assert_eq!(report.machine_entries, 0, "{report:?}");
}

#[test]
fn empty_append_target_gets_no_credit_before_live_integer_and_float_strategies() {
    const SOURCE: &str = r#"
def main(size: int, seed: object):
    result = []
    index = 0
    while index < size:
        result.append(seed)
        index = index + 1
    return seed
"#;
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(0), RuntimeValue::SmallInt(7)],
                )
                .expect("empty append target"),
            RuntimeValue::SmallInt(7)
        );
    }
    let empty = runtime.last_adaptive_report().expect("empty report");
    assert_eq!(empty.machine_entries, 0, "{empty:?}");
    assert!(
        !empty
            .regions
            .iter()
            .any(|region| region.reason.contains("loop region")),
        "{empty:?}"
    );

    assert_eq!(
        runtime
            .execute_with_args(
                &executable,
                &[RuntimeValue::SmallInt(2), RuntimeValue::SmallInt(7)],
            )
            .expect("integer strategy"),
        RuntimeValue::SmallInt(7)
    );
    assert_eq!(
        runtime
            .execute_with_args(
                &executable,
                &[RuntimeValue::SmallInt(2), RuntimeValue::Float(7.0)],
            )
            .expect("float strategy"),
        RuntimeValue::Float(7.0)
    );
    let live = runtime
        .last_adaptive_report()
        .expect("live strategy report");
    assert!(
        live.regions
            .iter()
            .any(|region| region.specialized_cases == 2),
        "{live:?}"
    );
    assert_eq!(live.machine_entries, 0, "{live:?}");

    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[RuntimeValue::SmallInt(2), RuntimeValue::Float(7.0)],
                )
                .expect("warm float strategy"),
            RuntimeValue::Float(7.0)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("compiled float report")
        .machine_entries;
    assert_eq!(
        runtime
            .execute_with_args(
                &executable,
                &[RuntimeValue::SmallInt(0), RuntimeValue::Float(9.0)],
            )
            .expect("empty no-mutation execution"),
        RuntimeValue::Float(9.0)
    );
    let empty = runtime
        .last_adaptive_report()
        .expect("empty no-mutation report");
    assert_eq!(empty.machine_entries, before, "{empty:?}");
}

#[test]
fn two_list_dynamic_reduction_call_tree_has_bounded_machine_entries() {
    const SOURCE: &str = r#"
def reduce_a(args: object):
    i = args[0]
    values = args[1]
    total = 0.0
    j = 0
    length = len(values)
    while j < length:
        total = total + (i + j + 1) * values[j]
        j = j + 1
    return total

def reduce_b(args: object):
    i = args[0]
    values = args[1]
    total = 0.0
    j = 0
    length = len(values)
    while j < length:
        total = total + (i + j + 2) * values[j]
        j = j + 1
    return total

def apply(fn: object, values: object, size: int):
    result = []
    i = 0
    while i < size:
        result.append(fn((i, values)))
        i = i + 1
    return result[size - 1]

def main(size: int, seed: float, alternate: bool):
    values = [seed] * size
    if alternate:
        return apply(reduce_b, values, size)
    return apply(reduce_a, values, size)
"#;
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[
                        RuntimeValue::SmallInt(2),
                        RuntimeValue::Float(2.0),
                        RuntimeValue::Bool(false),
                    ],
                )
                .expect("warm reduction"),
            RuntimeValue::Float(10.0)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm report")
        .machine_entries;
    let value = runtime
        .execute_with_args(
            &executable,
            &[
                RuntimeValue::SmallInt(3),
                RuntimeValue::Float(1.0),
                RuntimeValue::Bool(false),
            ],
        )
        .expect("changed reduction");
    let report = runtime.last_adaptive_report().expect("changed report");

    assert_eq!(value, RuntimeValue::Float(12.0));
    assert_eq!(
        report.machine_entries.saturating_sub(before),
        1,
        "{report:?}"
    );
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");

    let before = report.machine_entries;
    let value = runtime
        .execute_with_args(
            &executable,
            &[
                RuntimeValue::SmallInt(20),
                RuntimeValue::Float(1.0),
                RuntimeValue::Bool(false),
            ],
        )
        .expect("long changed reduction");
    let report = runtime.last_adaptive_report().expect("long changed report");
    assert_eq!(value, RuntimeValue::Float(590.0));
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
fn whole_main_outer_call_tree_and_zip_reduction_compile_as_one_region() {
    const SOURCE: &str = r#"
def reduce(args: object):
    i = args[0]
    values = args[1]
    bias = args[2]
    total = 0
    j = 0
    length = len(values)
    while j < length:
        total = total + (i + j + bias) * values[j]
        j = j + 1
    return total

def transform(fn: object, values: object, bias: int):
    result = []
    i = 0
    length = len(values)
    while i < length:
        result.append(fn((i, values, bias)))
        i = i + 1
    return result

def main(size: int, seed: float, rounds: int, bias: int):
    u = [seed] * size
    v = u
    iteration = 0
    while iteration < rounds:
        v = transform(reduce, u, bias)
        u = transform(reduce, v, bias)
        iteration = iteration + 1
    left = 0
    right = 0
    for ue, ve in zip(u, v):
        left = left + ue * ve
        right = right + ve * ve
    return left / right
"#;
    // Given: the real two-list call tree, outer loop, and two-accumulator reduction are warm.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[
                        RuntimeValue::SmallInt(2),
                        RuntimeValue::Float(1.0),
                        RuntimeValue::SmallInt(1),
                        RuntimeValue::SmallInt(1),
                    ],
                )
                .expect("warm execution"),
            RuntimeValue::Float(72.0 / 17.0)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm report")
        .machine_entries;

    // When: the loop count and the verified callee arithmetic both change.
    let value = runtime
        .execute_with_args(
            &executable,
            &[
                RuntimeValue::SmallInt(2),
                RuntimeValue::Float(1.0),
                RuntimeValue::SmallInt(2),
                RuntimeValue::SmallInt(2),
            ],
        )
        .expect("changed execution");
    let report = runtime.last_adaptive_report().expect("changed report");

    // Then: operation-derived native code covers the outer loop and final reduction once.
    assert_eq!(value, RuntimeValue::Float(657_552.0 / 106_706.0));
    assert_eq!(report.compile_failure, None, "{report:?}");
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
fn loopless_wrapper_owned_intermediate_compiles_as_one_invocation_transaction() {
    const SOURCE: &str = r#"
def reduce(args: object):
    i = args[0]
    values = args[1]
    bias = args[2]
    total = 0
    j = 0
    length = len(values)
    while j < length:
        total = total + (i + j + bias) * values[j]
        j = j + 1
    return total

def transform(fn: object, values: object, bias: int):
    result = []
    i = 0
    length = len(values)
    while i < length:
        result.append(fn((i, values, bias)))
        i = i + 1
    return result

def transform_twice(fn: object, values: object, bias: int):
    intermediate = transform(fn, values, bias)
    return transform(fn, intermediate, bias)

def main(size: int, seed: float, rounds: int, bias: int):
    u = [seed] * size
    v = u
    iteration = 0
    while iteration < rounds:
        v = transform_twice(reduce, u, bias)
        u = transform_twice(reduce, v, bias)
        iteration = iteration + 1
    left = 0
    right = 0
    for ue, ve in zip(u, v):
        left = left + ue * ve
        right = right + ve * ve
    return left / right
"#;
    // Given: a loopless wrapper owns the first transform result across the second child loop.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    for _ in 0..96 {
        assert_eq!(
            runtime
                .execute_with_args(
                    &executable,
                    &[
                        RuntimeValue::SmallInt(2),
                        RuntimeValue::Float(1.0),
                        RuntimeValue::SmallInt(1),
                        RuntimeValue::SmallInt(1),
                    ],
                )
                .expect("warm execution"),
            RuntimeValue::Float(17.944262295081966)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm report")
        .machine_entries;

    // When: the invocation changes its extent, rounds, and inner arithmetic dependency.
    let value = runtime
        .execute_with_args(
            &executable,
            &[
                RuntimeValue::SmallInt(2),
                RuntimeValue::Float(1.0),
                RuntimeValue::SmallInt(2),
                RuntimeValue::SmallInt(2),
            ],
        )
        .expect("changed execution");
    let report = runtime.last_adaptive_report().expect("changed report");

    // Then: the wrapper and both child loops enter one helper-free native transaction.
    assert_eq!(value, RuntimeValue::Float(37.973665961010276));
    assert_eq!(report.compile_failure, None, "{report:?}");
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
fn production_spectral_wrapper_chain_enters_one_native_region_per_invocation() {
    // Given: the unchanged production spectral program has completed every live/stable gate.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime
        .compile_function(include_str!("../examples/spectral_norm.py"), "main")
        .expect("production spectral fixture");
    for _ in 0..192 {
        assert_eq!(
            runtime
                .execute_with_args(&executable, &[])
                .expect("warm production execution"),
            RuntimeValue::Float(1.6236422398020804)
        );
    }
    let before = runtime
        .last_adaptive_report()
        .expect("warm production report")
        .machine_entries;

    // When: one complete invocation runs after compilation.
    let value = runtime
        .execute_with_args(&executable, &[])
        .expect("measured production execution");
    let report = runtime.last_adaptive_report().expect("measured report");

    // Then: the outer iterations and final reduction stay in one owned transaction.
    assert_eq!(value, RuntimeValue::Float(1.6236422398020804));
    assert_eq!(report.compile_failure, None, "{report:?}");
    assert_eq!(
        report.machine_entries.saturating_sub(before),
        1,
        "{report:?}"
    );
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

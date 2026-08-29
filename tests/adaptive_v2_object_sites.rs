use wustite::{ExecutionMode, Runtime, RuntimeConfig, RuntimeValue};

const SHAPES: &str = include_str!("../benchmarks/adaptive_shape_objects.py");
const LISTS: &str = include_str!("../benchmarks/adaptive_list_objects.py");
const CALLS: &str = include_str!("../benchmarks/adaptive_call_objects.py");
const NBODY: &str = include_str!("../examples/nbody.py");
const NESTED_LIST_LOOP: &str = r#"
def main():
    bodies = [([0.0], [1.0]), ([10.0], [2.0])]
    for body_index in range(2):
        body = bodies[body_index]
        for index in range(100):
            position = body[0]
            velocity = body[1]
            position[0] += velocity[0]
    return bodies[0][0][0] + bodies[1][0][0]
"#;
const NESTED_STORAGE_SHAPE_CHANGE: &str = r#"
def main():
    bodies = [([1.0], [2.0]), ((3.0,), (4.0,))]
    total = 0.0
    for body_index in range(2):
        body = bodies[body_index]
        for index in range(100):
            position = body[0]
            velocity = body[1]
            total += position[0] + velocity[0]
    return total
"#;
const MAXIMAL_NESTED_LOOP_CALL: &str = r#"
def step(bodies: list, limit: int):
    for body_index in range(2):
        body = bodies[body_index]
        for index in range(limit):
            position = body[0]
            velocity = body[1]
            position[0] += velocity[0]
    return bodies

def main(limit: int):
    bodies = [([0.0], [1.0]), ([10.0], [2.0])]
    for invocation in range(100):
        bodies = step(bodies, limit)
    return bodies[0][0][0] + bodies[1][0][0]
"#;
const MAXIMAL_ADVANCE_CALL: &str = r#"
def advance(dt: float, n: int, bodies: list, pairs: list):
    for _ in range(n):
        for body1, body2 in pairs:
            (x1, y1, z1), v1, m1 = body1
            (x2, y2, z2), v2, m2 = body2
            dx = x1 - x2
            dy = y1 - y2
            dz = z1 - z2
            mag = dt * ((dx * dx + dy * dy + dz * dz) ** (-1.5))
            b1m = m1 * mag
            b2m = m2 * mag
            v1[0] -= dx * b2m
            v1[1] -= dy * b2m
            v1[2] -= dz * b2m
            v2[0] += dx * b1m
            v2[1] += dy * b1m
            v2[2] += dz * b1m
        for position, velocity, _ in bodies:
            vx, vy, vz = velocity
            position[0] += dt * vx
            position[1] += dt * vy
            position[2] += dt * vz
    return 0

def main(n: int):
    bodies = [
        ([0.0, 0.0, 0.0], [0.0, 0.1, 0.0], 10.0),
        ([1.0, 0.0, 0.0], [0.0, -0.1, 0.0], 2.0),
        ([0.0, 2.0, 0.0], [0.05, 0.0, 0.0], 1.0),
    ]
    pairs = [
        (bodies[0], bodies[1]),
        (bodies[0], bodies[2]),
        (bodies[1], bodies[2]),
    ]
    for invocation in range(100):
        advance(0.01, n, bodies, pairs)
    return (
        bodies[0][0][0] + bodies[0][0][1] + bodies[0][0][2]
        + bodies[1][0][0] + bodies[1][0][1] + bodies[1][0][2]
        + bodies[2][0][0] + bodies[2][0][1] + bodies[2][0][2]
    )
"#;
const SCALED_PRODUCTION_ADVANCE: &str = r#"
def advance(dt: float, n: int, bodies: list, pairs: list):
    for _ in range(n):
        for body1, body2 in pairs:
            (x1, y1, z1), v1, m1 = body1
            (x2, y2, z2), v2, m2 = body2
            dx = x1 - x2
            dy = y1 - y2
            dz = z1 - z2
            mag = dt * ((dx * dx + dy * dy + dz * dz) ** (-1.5))
            b1m = m1 * mag
            b2m = m2 * mag
            v1[0] -= dx * b2m
            v1[1] -= dy * b2m
            v1[2] -= dz * b2m
            v2[0] += dx * b1m
            v2[1] += dy * b1m
            v2[2] += dz * b1m
        for position, velocity, _ in bodies:
            vx, vy, vz = velocity
            position[0] += dt * vx
            position[1] += dt * vy
            position[2] += dt * vz
    return 0

def main(steps: int):
    bodies = [
        ([0.0, 0.0, 0.0], [0.0, 0.1, 0.0], 10.0),
        ([1.0, 0.0, 0.0], [0.0, -0.1, 0.0], 2.0),
        ([0.0, 2.0, 0.0], [0.05, 0.0, 0.0], 1.0),
    ]
    pairs = [
        (bodies[0], bodies[1]),
        (bodies[0], bodies[2]),
        (bodies[1], bodies[2]),
    ]
    advance(0.01, steps, bodies, pairs)
    return (
        bodies[0][0][0] + bodies[0][0][1] + bodies[0][0][2]
        + bodies[1][0][0] + bodies[1][0][1] + bodies[1][0][2]
        + bodies[2][0][0] + bodies[2][0][1] + bodies[2][0][2]
    )
"#;
const FIVE_SHAPES: &str = r#"
class A:
    def marker(self):
        return 0
class B:
    def marker(self):
        return 0
class C:
    def marker(self):
        return 0
class D:
    def marker(self):
        return 0
class E:
    def marker(self):
        return 0

def main():
    a = A()
    b = B()
    c = C()
    d = D()
    e = E()
    a.number = 1
    b.number = 2
    c.number = 3
    d.number = 4
    e.number = 5
    items = [a, b, c, d, e]
    index = 0
    total = 0
    while index < 5:
        total = total + items[index].number
        index = index + 1
    return total
"#;

const COMPILED_THEN_GENERIC: &str = r#"
class A:
    def marker(self):
        return 0
class B:
    def marker(self):
        return 0
class C:
    def marker(self):
        return 0
class D:
    def marker(self):
        return 0
class E:
    def marker(self):
        return 0

def read(value: object):
    return value.number

def main():
    a = A()
    b = B()
    c = C()
    d = D()
    e = E()
    a.number = 1
    b.number = 2
    c.number = 3
    d.number = 4
    e.number = 5
    count = 0
    total = 0
    while count < 24:
        total = total + read(a)
        total = total + read(b)
        total = total + read(c)
        total = total + read(d)
        count = count + 1
    return total + read(e)
"#;

fn runtime() -> Runtime {
    Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    })
}

#[test]
fn actual_python_shape_sites_enter_direct_machine_storage() {
    // Given: the accepted shape-heavy Python fixture and one persistent adaptive runtime.
    let mut runtime = runtime();
    let executable = runtime.compile_function(SHAPES, "main").expect("compile");

    // When: live object sites cross both profiling gates across fresh instances.
    for _ in 0..3 {
        assert_eq!(
            runtime.execute(&executable).expect("shape execution"),
            RuntimeValue::SmallInt(4_096)
        );
    }

    // Then: direct field-storage entries retain exact Python semantics without helper dispatch.
    let report = runtime.last_adaptive_report().expect("adaptive report");
    assert!(report.machine_entries > 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
    assert!(report.selected_snapshot_id.is_some(), "{report:?}");
    assert!(report.tier1_snapshot_id.is_some(), "{report:?}");
    assert_eq!(
        report.selected_snapshot_id, report.tier1_snapshot_id,
        "{report:?}"
    );
    assert_eq!(
        report.compile_tier.as_deref(),
        Some("cranelift"),
        "{report:?}"
    );
    assert!(report.cache_misses > 0, "{report:?}");
    assert!(report.cache_hits > 0, "{report:?}");
    assert!(report.cache_bytes > 0, "{report:?}");
}

#[test]
fn actual_python_list_read_uses_owned_direct_storage() {
    // Given: the accepted mutation-heavy list fixture and one persistent adaptive runtime.
    let mut runtime = runtime();
    let executable = runtime.compile_function(LISTS, "main").expect("compile");

    // When: the list sites receive enough live samples to record and compile.
    for _ in 0..3 {
        assert_eq!(
            runtime.execute(&executable).expect("list execution"),
            RuntimeValue::SmallInt(2_016)
        );
    }

    // Then: owned read storage enters machine code while mutations remain authoritative.
    let report = runtime.last_adaptive_report().expect("adaptive report");
    assert!(report.machine_entries > 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
    assert!(report.selected_snapshot_id.is_some(), "{report:?}");
    assert!(report.tier1_snapshot_id.is_some(), "{report:?}");
    assert_ne!(
        report.selected_snapshot_id, report.tier1_snapshot_id,
        "{report:?}"
    );
    assert_eq!(
        report.compile_tier.as_deref(),
        Some("cranelift"),
        "{report:?}"
    );
    assert!(report.cache_misses > 0, "{report:?}");
    assert!(report.cache_hits > 0, "{report:?}");
    assert!(report.cache_bytes > 0, "{report:?}");
}

#[test]
fn nbody_nested_list_reads_enter_native_loop_code() {
    // Given: nbody's object lists and one persistent adaptive runtime.
    let mut runtime = runtime();
    let executable = runtime.compile_function(NBODY, "main").expect("compile");

    // When: the real nbody fixture executes through adaptive-v2.
    let result = runtime.execute(&executable).expect("nbody execution");

    // Then: nested list reads preserve their handles and execute without fallback dispatch.
    assert!(
        matches!(result, RuntimeValue::Float(value) if (value - -0.169_089_262_755_271_72).abs() <= 1e-12),
        "nbody result: {result:?}"
    );
    let report = runtime.last_adaptive_report().expect("adaptive report");
    assert!(report.native_executions > 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
    assert!(report.compile_failure.is_none(), "{report:?}");
}

#[test]
fn dynamically_selected_nested_lists_use_direct_native_storage() {
    // Given: a hot loop selecting two nested float-list pairs through an outer object list.
    let mut runtime = runtime();
    let executable = runtime
        .compile_function(NESTED_LIST_LOOP, "main")
        .expect("compile");

    // When: the loop mutates both selected nested lists through adaptive-v2.
    let result = runtime.execute(&executable).expect("nested execution");

    // Then: direct storage preserves the mutations without helper dispatch or deoptimization.
    assert_eq!(result, RuntimeValue::Float(310.0));
    let report = runtime.last_adaptive_report().expect("adaptive report");
    assert!(report.native_executions > 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
    assert!(report.compile_failure.is_none(), "{report:?}");
}

#[test]
fn nested_storage_shape_change_rejects_native_entry_safely() {
    // Given: one compiled list-backed case followed by a tuple-backed shape change.
    let mut runtime = runtime();
    let executable = runtime
        .compile_function(NESTED_STORAGE_SHAPE_CHANGE, "main")
        .expect("compile");

    // When: both shapes execute through the same adaptive loop site.
    let result = runtime
        .execute(&executable)
        .expect("shape-change execution");

    // Then: the incompatible storage is interpreted without native misuse or helper dispatch.
    assert_eq!(result, RuntimeValue::Float(1000.0));
    let report = runtime.last_adaptive_report().expect("adaptive report");
    assert!(report.native_executions > 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
}

#[test]
fn maximal_nested_loop_call_does_not_scale_machine_entries_with_inner_trip_count() {
    // Given: one maximal nested-loop callee executed with N and 2N inner iterations.
    let mut runtime = runtime();
    let executable = runtime
        .compile_function(MAXIMAL_NESTED_LOOP_CALL, "main")
        .expect("compile");

    // When: both executions cross the same persistent adaptive compilation lifecycle.
    let n = runtime
        .execute_with_args(&executable, &[RuntimeValue::SmallInt(8)])
        .expect("N execution");
    let n_entries = runtime
        .last_adaptive_report()
        .expect("N report")
        .machine_entries;
    let twice_n = runtime
        .execute_with_args(&executable, &[RuntimeValue::SmallInt(16)])
        .expect("2N execution");
    let after = runtime.last_adaptive_report().expect("2N report");
    let twice_n_entries = after.machine_entries.saturating_sub(n_entries);

    // Then: exact semantics hold and the native transaction count is outer-invocation bounded.
    assert_eq!(n, RuntimeValue::Float(2_410.0));
    assert_eq!(twice_n, RuntimeValue::Float(4_810.0));
    assert!(n_entries > 0);
    assert!(twice_n_entries > 0, "{after:?}");
    assert!(
        twice_n_entries <= n_entries.saturating_add(200),
        "N entries={n_entries}, 2N entries={twice_n_entries}, report={after:?}"
    );
    assert_eq!(after.helper_calls, 0, "{after:?}");
    assert_eq!(after.generic_dispatch_calls, 0, "{after:?}");
    assert_eq!(after.deopts, 0, "{after:?}");
}

#[test]
fn maximal_advance_call_uses_one_native_transaction_for_all_three_loops() {
    // Given: the real advance(dt, n, bodies, pairs) topology on one persistent runtime.
    let mut runtime = runtime();
    let executable = runtime
        .compile_function(MAXIMAL_ADVANCE_CALL, "main")
        .expect("compile");
    for _ in 0..3 {
        assert_eq!(
            runtime
                .execute_with_args(&executable, &[RuntimeValue::SmallInt(0)])
                .expect("warm execution"),
            RuntimeValue::Float(3.0)
        );
    }
    let warm = runtime.last_adaptive_report().expect("warm report");
    let entries_before = warm.machine_entries;
    let native_before = warm.native_executions;

    // When: post-compilation calls execute N and 2N outer iterations.
    let n = runtime
        .execute_with_args(&executable, &[RuntimeValue::SmallInt(2)])
        .expect("N execution");
    let after_n = runtime.last_adaptive_report().expect("N report");
    let n_entries = after_n.machine_entries.saturating_sub(entries_before);
    let n_native = after_n.native_executions.saturating_sub(native_before);
    let after_n_debug = format!("{after_n:?}");
    let after_n_entries = after_n.machine_entries;
    let after_n_native = after_n.native_executions;
    let twice_n = runtime
        .execute_with_args(&executable, &[RuntimeValue::SmallInt(4)])
        .expect("2N execution");
    let after = runtime.last_adaptive_report().expect("2N report");
    let twice_n_entries = after.machine_entries.saturating_sub(after_n_entries);
    let twice_n_native = after.native_executions.saturating_sub(after_n_native);

    // Then: both nested loop SCCs share the outer call's one native transaction boundary.
    assert_eq!(n, RuntimeValue::Float(-69.22303365949371));
    assert_eq!(twice_n, RuntimeValue::Float(-155.30141706622493));
    assert_eq!(
        (n_entries, n_native, twice_n_entries, twice_n_native),
        (1, 1, 1, 1),
        "N report={after_n_debug}, 2N report={after:?}"
    );
    assert_eq!(after.helper_calls, 0, "{after:?}");
    assert_eq!(after.generic_dispatch_calls, 0, "{after:?}");
    assert_eq!(after.deopts, 0, "{after:?}");
    assert!(after.compile_failure.is_none(), "{after:?}");
    assert!(after.selected_snapshot_id.is_some(), "{after:?}");
    assert!(after.tier1_snapshot_id.is_some(), "{after:?}");
    assert_ne!(after.selected_snapshot_id, after.tier1_snapshot_id);
}

#[test]
fn scaled_production_advance_has_one_transaction_per_outer_call() {
    use std::time::{Duration, Instant};

    let mut interpreter = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 1,
    });
    let interpreted = interpreter
        .compile_function(SCALED_PRODUCTION_ADVANCE, "main")
        .expect("interpreter compile");
    let expected = [200_i64, 400, 800].map(|steps| {
        interpreter
            .execute_with_args(&interpreted, &[RuntimeValue::SmallInt(steps)])
            .expect("interpreter execution")
    });

    let mut runtime = runtime();
    let executable = runtime
        .compile_function(SCALED_PRODUCTION_ADVANCE, "main")
        .expect("adaptive compile");
    runtime
        .execute_with_args(&executable, &[RuntimeValue::SmallInt(200)])
        .expect("warm execution");
    let warm = runtime.last_adaptive_report().expect("warm report");
    let mut previous_entries = warm.machine_entries;
    let mut previous_native = warm.native_executions;
    let mut durations = Vec::new();
    let mut deltas = Vec::new();
    for (index, steps) in [200_i64, 400, 800].into_iter().enumerate() {
        let started = Instant::now();
        let actual = runtime
            .execute_with_args(&executable, &[RuntimeValue::SmallInt(steps)])
            .expect("adaptive execution");
        let elapsed = started.elapsed();
        let report = runtime.last_adaptive_report().expect("adaptive report");
        let entries = report.machine_entries.saturating_sub(previous_entries);
        let native = report.native_executions.saturating_sub(previous_native);
        eprintln!(
            "scaled advance steps={steps} result={actual:?} machine_delta={entries} native_delta={native} elapsed_ms={} snapshot={:?} tier1={:?} helper={} generic={} deopt={} regions={:?}",
            elapsed.as_millis(),
            report.selected_snapshot_id,
            report.tier1_snapshot_id,
            report.helper_calls,
            report.generic_dispatch_calls,
            report.deopts,
            report.regions
        );
        assert_eq!(actual, expected[index]);
        assert_eq!(report.helper_calls, 0, "{report:?}");
        assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
        assert_eq!(report.deopts, 0, "{report:?}");
        assert!(report.compile_failure.is_none(), "{report:?}");
        assert!(report.selected_snapshot_id.is_some(), "{report:?}");
        assert!(report.tier1_snapshot_id.is_some(), "{report:?}");
        assert_ne!(report.selected_snapshot_id, report.tier1_snapshot_id);
        durations.push(elapsed);
        deltas.push((entries, native));
        previous_entries = report.machine_entries;
        previous_native = report.native_executions;
    }

    assert_eq!(deltas, vec![(1, 1), (1, 1), (1, 1)]);
    assert!(
        durations[1] <= durations[0].saturating_mul(3) + Duration::from_millis(500),
        "durations={durations:?}"
    );
    assert!(
        durations[2] <= durations[1].saturating_mul(3) + Duration::from_millis(500),
        "durations={durations:?}"
    );
}

#[test]
fn actual_python_method_call_enters_the_fused_function_entry() {
    // Given: the accepted Python method-call fixture and persistent adaptive state.
    let mut runtime = runtime();
    let executable = runtime.compile_function(CALLS, "main").expect("compile");

    // When: the same live call site crosses profiling and compilation gates.
    for _ in 0..3 {
        assert_eq!(
            runtime.execute(&executable).expect("call execution"),
            RuntimeValue::SmallInt(24_512)
        );
    }

    // Then: the live-gated callee entry executes machine code without helper dispatch.
    let report = runtime.last_adaptive_report().expect("adaptive report");
    assert!(report.machine_entries > 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
    assert!(report.selected_snapshot_id.is_some(), "{report:?}");
    assert_eq!(
        report.selected_snapshot_id, report.tier1_snapshot_id,
        "{report:?}"
    );
    #[cfg(feature = "inkwell")]
    assert_eq!(
        report.compile_tier.as_deref(),
        Some("llvm-o3"),
        "{report:?}"
    );
    #[cfg(not(feature = "inkwell"))]
    assert_eq!(
        report.compile_tier.as_deref(),
        Some("cranelift"),
        "{report:?}"
    );
    assert!(report.cache_misses > 0, "{report:?}");
    assert!(report.cache_hits > 0, "{report:?}");
    assert!(report.cache_bytes > 0, "{report:?}");
}

#[test]
fn fifth_live_receiver_shape_turns_the_public_site_generic() {
    // Given: one Python attribute site receiving five distinct runtime classes.
    let mut runtime = runtime();
    let executable = runtime
        .compile_function(FIVE_SHAPES, "main")
        .expect("compile");

    // When: four specialized cases are followed by the fifth live case.
    assert_eq!(
        runtime.execute(&executable).expect("shape execution"),
        RuntimeValue::SmallInt(15)
    );

    // Then: the site is invalidated to generic before claiming native execution.
    let report = runtime.last_adaptive_report().expect("adaptive report");
    let generic = report
        .regions
        .iter()
        .find(|region| region.generic && region.specialized_cases == 4)
        .unwrap_or_else(|| panic!("five-case generic site: {report:?}"));
    assert_eq!(generic.live_entries, 5, "{report:?}");
    assert_eq!(report.invalidations, 1, "{report:?}");
    assert_eq!(report.native_executions, 0, "{report:?}");
}

#[test]
fn fifth_case_evicts_compiled_site_and_releases_accounted_cache_bytes() {
    let mut runtime = runtime();
    let executable = runtime
        .compile_function(COMPILED_THEN_GENERIC, "main")
        .expect("compile");

    assert_eq!(
        runtime.execute(&executable).expect("shape execution"),
        RuntimeValue::SmallInt(245)
    );

    let report = runtime.last_adaptive_report().expect("adaptive report");
    assert!(
        report.regions.iter().any(|region| region.generic),
        "{report:?}"
    );
    assert_eq!(report.cache_evictions, 1, "{report:?}");
    assert_eq!(report.cache_bytes, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
}

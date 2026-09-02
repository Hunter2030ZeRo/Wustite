use std::sync::{Arc, Barrier};
use std::thread;

use wustite::SharedRuntime;
use wustite::value::Value;
use wustite::{ExecutionMode, Object, Runtime, RuntimeConfig, RuntimeCore, RuntimeValue, Vm};

const ADD_SOURCE: &str = include_str!("fixtures/adaptive_add.py");
const LOOP_SOURCE: &str = include_str!("fixtures/adaptive_loop.py");
const GUARDED_BOOL_SOURCE: &str = include_str!("fixtures/adaptive_guarded_bool.py");
const RETURN_LIST_SOURCE: &str = r#"
def main():
    values = []
    values.append(42)
    return values
"#;
const EXECUTION_LIST_SOURCE: &str = r#"
def main(value: int):
    values = []
    values.append(value)
    return values
"#;
const SHARED_LIST_SOURCE: &str = include_str!("../benchmarks/adaptive_list_objects.py");
const SHARED_CALL_SOURCE: &str = include_str!("../benchmarks/adaptive_call_objects.py");

fn config() -> RuntimeConfig {
    RuntimeConfig {
        execution_mode: ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    }
}

#[test]
fn adaptive_runtime_enters_native_after_gates() {
    // Given: a fresh adaptive runtime and actual Python-to-WVM integer function.
    let mut runtime = Runtime::new_adaptive_v2(config());
    let executable = runtime.compile_function(ADD_SOURCE, "main").unwrap();

    // When: the same entry accumulates 32 pre-record and 32 post-record live samples.
    for _ in 0..100 {
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

    // Then: Task5 machine code, not generic dispatch, produced warm results.
    let report = runtime.last_adaptive_report().unwrap();
    assert_eq!(report.runtime_core, RuntimeCore::AdaptiveV2);
    assert!(report.machine_entries > 0, "{report:?}");
    assert_eq!(
        report.machine_entries, report.native_executions,
        "{report:?}"
    );
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert!(report.selected_snapshot_id.is_some(), "{report:?}");
    assert_eq!(report.selected_snapshot_id, report.tier1_snapshot_id);
    assert_eq!(report.compile_tier.as_deref(), Some("cranelift"));
    assert_eq!(report.cache_misses, 1);
    assert_eq!(report.cache_hits, report.machine_entries);
    assert!(report.cache_bytes > 0, "{report:?}");
    assert_eq!(report.readiness.live, 64);
    assert_eq!(report.readiness.cached, 0);
    assert_eq!(report.readiness.static_analysis, 0);
}

#[cfg(feature = "inkwell")]
#[test]
fn adaptive_tiered_runtime_promotes_executed_tier1_snapshot_to_llvm() {
    // Given: a retained public adaptive entry that has crossed both live-profile gates.
    let mut runtime = Runtime::new_adaptive_v2(config());
    let executable = runtime.compile_function(ADD_SOURCE, "main").unwrap();

    // When: ten warm Cranelift entries establish the receipt required by tier 2.
    for _ in 0..107 {
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

    // Then: LLVM O3 executes the exact immutable snapshot accepted by tier 1.
    let report = runtime.last_adaptive_report().unwrap();
    assert_eq!(report.selected_snapshot_id, report.tier1_snapshot_id);
    assert_eq!(report.tier1_snapshot_id, report.tier2_snapshot_id);
    assert_eq!(report.compile_tier.as_deref(), Some("llvm-o3"));
    assert!(report.native_executions >= 11, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.compile_failure, None, "{report:?}");
}

#[cfg(not(feature = "inkwell"))]
#[test]
fn tiered_runtime_keeps_cranelift_without_llvm() {
    // Given: a default-feature adaptive runtime with no LLVM backend compiled in.
    let mut runtime = Runtime::new_adaptive_v2(config());
    let executable = runtime.compile_function(ADD_SOURCE, "main").unwrap();

    // When: the same retained entry runs beyond the normal tier-2 threshold.
    for _ in 0..107 {
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

    // Then: the report names only the tier that actually executed.
    let report = runtime.last_adaptive_report().unwrap();
    assert_eq!(report.selected_snapshot_id, report.tier1_snapshot_id);
    assert_eq!(report.tier2_snapshot_id, None);
    assert_eq!(report.compile_tier.as_deref(), Some("cranelift"));
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.compile_failure, None, "{report:?}");
}

#[test]
fn adaptive_public_guard_exit_links_executes_verified_bridge() {
    // Given: a boolean trace recorded on true and retained by the Cranelift-only public runtime.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::Jit(wustite::CompilerBackend::Cranelift),
        hot_threshold: 1,
    });
    let executable = runtime
        .compile_function(GUARDED_BOOL_SOURCE, "main")
        .unwrap();
    for _ in 0..97 {
        assert_eq!(
            runtime
                .execute_with_args(&executable, &[RuntimeValue::Bool(true)])
                .unwrap(),
            RuntimeValue::Bool(true)
        );
    }
    let parent_snapshot = runtime
        .last_adaptive_report()
        .and_then(|report| report.selected_snapshot_id.clone())
        .expect("compiled parent snapshot");

    // When: the opposite live case fails the parent guard exactly thirty-two times.
    for _ in 0..33 {
        assert_eq!(
            runtime
                .execute_with_args(&executable, &[RuntimeValue::Bool(false)])
                .unwrap(),
            RuntimeValue::Bool(false)
        );
    }

    // Then: the linked child handles the next false case without another parent deopt.
    let report = runtime.last_adaptive_report().unwrap();
    assert_eq!(report.bridges, 1, "{report:?}");
    assert_eq!(report.guard_failures.get(&1), Some(&32), "{report:?}");
    assert_eq!(report.deopts, 32, "{report:?}");
    assert!(report.native_executions >= 2, "{report:?}");
    assert_ne!(
        report.selected_snapshot_id.as_deref(),
        Some(parent_snapshot.as_str()),
        "{report:?}"
    );
    assert_eq!(report.selected_snapshot_id, report.tier1_snapshot_id);
    assert_eq!(report.tier2_snapshot_id, None);
    assert_eq!(report.compile_tier.as_deref(), Some("cranelift"));
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.compile_failure, None, "{report:?}");
}

#[test]
fn entry_type_change_falls_back_to_wvm() {
    // Given: an entry specialized and compiled from live SmallInt observations.
    let mut runtime = Runtime::new_adaptive_v2(config());
    let executable = runtime
        .compile_function(
            "def main(left: object, right: object):\n    return left + right\n",
            "main",
        )
        .unwrap();
    for _ in 0..100 {
        runtime
            .execute_with_args(
                &executable,
                &[RuntimeValue::SmallInt(20), RuntimeValue::SmallInt(22)],
            )
            .unwrap();
    }

    // When: the retained site receives an incompatible but valid scalar schema.
    let value = runtime
        .execute_with_args(
            &executable,
            &[RuntimeValue::Float(1.5), RuntimeValue::Float(2.25)],
        )
        .unwrap();

    // Then: native rejection is a cold fallback and WVM preserves exact semantics.
    assert_eq!(value, RuntimeValue::Float(3.75));
    assert_eq!(
        runtime.last_adaptive_report().unwrap().compile_failure,
        None
    );
}

#[test]
fn public_entry_gate_requires_exact_32_32_live_samples() {
    // Given: an adaptive function with no cached or static readiness credit.
    let mut runtime = Runtime::new_adaptive_v2(config());
    let executable = runtime.compile_function(ADD_SOURCE, "main").unwrap();
    let arguments = [RuntimeValue::SmallInt(20), RuntimeValue::SmallInt(22)];

    // When: execution stops on each live threshold boundary.
    for _ in 0..31 {
        runtime.execute_with_args(&executable, &arguments).unwrap();
    }
    let before_record = runtime.last_adaptive_report().unwrap().clone();
    runtime.execute_with_args(&executable, &arguments).unwrap();
    let record_started = runtime.last_adaptive_report().unwrap().clone();
    for _ in 0..31 {
        runtime.execute_with_args(&executable, &arguments).unwrap();
    }
    let before_compile = runtime.last_adaptive_report().unwrap().clone();
    runtime.execute_with_args(&executable, &arguments).unwrap();
    let compiled = runtime.last_adaptive_report().unwrap();

    // Then: neither phase advances early and only live observations contribute.
    assert_eq!(before_record.readiness.live, 31);
    assert_eq!(before_record.regions[0].lifecycle, "profiling");
    assert_eq!(record_started.readiness.live, 32);
    assert_eq!(record_started.regions[0].lifecycle, "recording");
    assert_eq!(before_compile.readiness.live, 63);
    assert_eq!(before_compile.regions[0].stable_observations, 31);
    assert_eq!(compiled.readiness.live, 64);
    assert_eq!(compiled.regions[0].lifecycle, "compiled");
    assert_eq!(compiled.machine_entries, 0);
    assert_eq!(compiled.readiness.cached, 0);
    assert_eq!(compiled.readiness.static_analysis, 0);
}

#[test]
fn adaptive_loop_header_osr_finishes_live_python_loop_in_native() {
    // Given: a pure integer loop with a StructureMap loop-header entry.
    let mut runtime = Runtime::new_adaptive_v2(config());
    let executable = runtime.compile_function(LOOP_SOURCE, "main").unwrap();

    // When: one execution crosses both live gates inside the loop.
    let value = runtime.execute(&executable).unwrap();

    // Then: native loop-header OSR returns the exact semantic result without dispatch.
    assert_eq!(value, RuntimeValue::SmallInt(4_950));
    let report = runtime.last_adaptive_report().unwrap();
    assert!(report.machine_entries > 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert!(report.cache_hits > 0, "{report:?}");
    assert!(report.cache_bytes > 0, "{report:?}");
    assert!(
        report
            .regions
            .iter()
            .any(|region| region.entry_pc == 4 && region.lifecycle == "compiled"),
        "{report:?}"
    );
}

#[test]
fn runtime_clones_share_code_and_own_results() {
    // Given: two clones sharing one adaptive profile and native-code core.
    let runtime = SharedRuntime::new_adaptive_v2(config());
    let executable = runtime.compile_function(ADD_SOURCE, "main").unwrap();
    for _ in 0..100 {
        let rooted = runtime
            .execute_rooted(
                &executable,
                &[RuntimeValue::SmallInt(1), RuntimeValue::SmallInt(2)],
            )
            .unwrap();
        assert_eq!(
            runtime.resolve_rooted(&rooted).unwrap(),
            RuntimeValue::SmallInt(3)
        );
    }

    // When: two clones enter execution together and a foreign runtime inspects a result.
    let barrier = Arc::new(Barrier::new(3));
    let workers = [(40, 2), (19, 23)].map(|(left, right)| {
        let clone = runtime.clone();
        let function = executable.clone();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            clone.execute_rooted(
                &function,
                &[RuntimeValue::SmallInt(left), RuntimeValue::SmallInt(right)],
            )
        })
    });
    barrier.wait();
    let [first, second] = workers.map(|worker| worker.join().unwrap().unwrap());
    let foreign = SharedRuntime::new_adaptive_v2(config());

    // Then: both executions reuse shared code and cross-runtime rooted access is rejected.
    assert_eq!(
        runtime.resolve_rooted(&first).unwrap(),
        RuntimeValue::SmallInt(42)
    );
    assert_eq!(
        runtime.resolve_rooted(&second).unwrap(),
        RuntimeValue::SmallInt(42)
    );
    assert!(foreign.resolve_rooted(&first).is_err());
    let boxed = runtime
        .execute_rooted(
            &executable,
            &[RuntimeValue::SmallInt(i64::MAX), RuntimeValue::SmallInt(0)],
        )
        .unwrap();
    let boxed_clone = boxed.clone();
    drop(boxed);
    runtime.collect_garbage().unwrap();
    assert_eq!(
        runtime.resolve_rooted(&boxed_clone).unwrap(),
        RuntimeValue::SmallInt(i64::MAX)
    );
    let report = runtime.adaptive_report().unwrap().unwrap();
    assert!(report.machine_entries > 0, "{report:?}");
    assert_eq!(report.cache_misses, 1, "{report:?}");
    assert_eq!(report.readiness.live, 64, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
}

#[test]
fn vm_adaptive_constructor_opt_in_legacy_default_unchanged() {
    // Given/When: callers construct each VM core explicitly.
    let _adaptive = Vm::new_adaptive_v2();
    let legacy = Runtime::new(RuntimeConfig::default());

    // Then: existing constructors remain legacy and expose no adaptive report.
    assert!(legacy.last_adaptive_report().is_none());
}

#[test]
fn interpreter_never_reports_native_success() {
    // Given: adaptive-v2 is explicitly selected with native compilation disabled.
    let mut runtime = Runtime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(ADD_SOURCE, "main").unwrap();

    // When: live profiling crosses both compilation gates.
    for _ in 0..100 {
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

    // Then: WVM fallback remains correct and the failed tier is explicit.
    let report = runtime.last_adaptive_report().unwrap();
    assert_eq!(report.machine_entries, 0);
    assert_eq!(report.native_executions, 0);
    assert_eq!(report.generic_dispatch_calls, 0);
    assert!(
        report
            .compile_failure
            .as_deref()
            .is_some_and(|failure| failure.contains("interpreter mode")),
        "{report:?}"
    );
}

#[test]
fn boxed_scalar_lives_until_last_clone() {
    // Given: a shared interpreter result whose i64 requires an adaptive boxed scalar.
    let runtime = SharedRuntime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(ADD_SOURCE, "main").unwrap();
    let rooted = runtime
        .execute_rooted(
            &executable,
            &[RuntimeValue::SmallInt(i64::MAX), RuntimeValue::SmallInt(0)],
        )
        .unwrap();

    // When: one owner drops and a major collection runs while its clone remains live.
    let clone = rooted.clone();
    drop(rooted);
    runtime.collect_garbage().unwrap();

    // Then: the counted lease remains valid and a foreign runtime still rejects it.
    assert_eq!(
        runtime.resolve_rooted(&clone).unwrap(),
        RuntimeValue::SmallInt(i64::MAX)
    );
    let foreign = SharedRuntime::new_adaptive_v2(RuntimeConfig {
        execution_mode: ExecutionMode::Interpreter,
        hot_threshold: 1,
    });
    assert!(foreign.resolve_rooted(&clone).is_err());
    let report = runtime.adaptive_report().unwrap().unwrap();
    assert_eq!(report.gc_allocations, 1, "{report:?}");
    assert_eq!(report.gc_major_collections, 1, "{report:?}");
    assert!(report.gc_bytes > 0, "{report:?}");
    assert!(report.gc_pause_micros > 0, "{report:?}");
}

#[test]
fn rooted_object_keeps_both_adaptive_public_compat_heaps_alive() {
    // Given: a shared adaptive runtime returning a newly allocated, adaptively mutated list.
    let runtime = SharedRuntime::new_adaptive_v2(config());
    let executable = runtime
        .compile_function(RETURN_LIST_SOURCE, "main")
        .unwrap();

    // When: the result is cloned and the adaptive heap completes a major collection.
    let rooted = runtime.execute_rooted(&executable, &[]).unwrap();
    let clone = rooted.clone();
    drop(rooted);
    runtime.collect_garbage().unwrap();

    // Then: the counted adaptive pin and retained WVM heap preserve the same public object value.
    let RuntimeValue::Object(reference) = runtime.resolve_rooted(&clone).unwrap() else {
        panic!("rooted result must retain an object reference");
    };
    assert_eq!(clone.value(), RuntimeValue::Object(reference));
    let Object::List(values) = clone.object().unwrap() else {
        panic!("rooted object must remain a list");
    };
    assert_eq!(values.len(), 1);

    let other = SharedRuntime::new_adaptive_v2(config());
    assert!(other.resolve_rooted(&clone).is_err());
}

#[test]
fn sequential_object_runs_keep_adapter_ownership() {
    let runtime = SharedRuntime::new_adaptive_v2(config());
    let executable = runtime
        .compile_function(EXECUTION_LIST_SOURCE, "main")
        .unwrap();
    let first = runtime
        .execute_rooted(&executable, &[RuntimeValue::SmallInt(11)])
        .unwrap();
    for expected in 12..=110 {
        let rooted = runtime
            .execute_rooted(&executable, &[RuntimeValue::SmallInt(expected)])
            .unwrap();
        let Object::List(values) = rooted.object().unwrap() else {
            panic!("result must be a list");
        };
        assert_eq!(values.to_vec(), vec![Value::SmallInt(expected)]);
    }
    let Object::List(first_values) = first.object().unwrap() else {
        panic!("first result must be a list");
    };
    assert_eq!(first_values.to_vec(), vec![Value::SmallInt(11)]);
    let report = runtime.adaptive_report().unwrap().unwrap();
    assert_eq!(report.cache_misses, 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
}

#[test]
fn shared_mutators_overlap_gc_without_global_lock() {
    // Given: two object-heavy programs and a collector sharing one adaptive runtime core.
    let runtime = SharedRuntime::new_adaptive_v2(config());
    let list = runtime
        .compile_function(SHARED_LIST_SOURCE, "main")
        .expect("compile list fixture");
    let call = runtime
        .compile_function(SHARED_CALL_SOURCE, "main")
        .expect("compile call fixture");
    let start = Arc::new(Barrier::new(4));
    let mutators = [(list, 2_016), (call, 24_512)].map(|(function, expected)| {
        let runtime = runtime.clone();
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            for _ in 0..3 {
                let rooted = runtime
                    .execute_rooted(&function, &[])
                    .expect("execute fixture");
                assert_eq!(
                    runtime.resolve_rooted(&rooted).expect("resolve result"),
                    RuntimeValue::SmallInt(expected)
                );
            }
        })
    });
    let collector = {
        let runtime = runtime.clone();
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            for _ in 0..16 {
                runtime.collect_garbage().expect("concurrent collection");
                thread::yield_now();
            }
        })
    };

    // When: both mutators and the collector are released together.
    start.wait();
    for mutator in mutators {
        mutator.join().expect("mutator join");
    }
    collector.join().expect("collector join");

    // Then: fused native paths avoid helpers and roots survive every collection.
    let report = runtime.adaptive_report().unwrap().unwrap();
    assert!(report.machine_entries > 0, "{report:?}");
    assert_eq!(report.helper_calls, 0, "{report:?}");
    assert_eq!(report.generic_dispatch_calls, 0, "{report:?}");
    assert_eq!(report.deopts, 0, "{report:?}");
    assert_eq!(report.gc_major_collections, 16, "{report:?}");
}

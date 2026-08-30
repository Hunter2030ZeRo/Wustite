use super::super::integration::{BridgeSite, execute_cached_bridge};
use super::super::native::bridge::{
    BridgeDecision, BridgeKey, BridgeLinkOutcome, BridgeRegistry, BridgeRuntime, FailureOrigin,
};
use super::super::native::cache::{CacheKey, NativeTier, SharedCodeCache};
use super::super::native::optimizer::OptimizerPipeline;
use super::super::native::{CachedNativeExecutor, NativeCompiler, NativeValue};
use super::task4_support::{compile_permit, dependencies, identity};
use crate::adaptive_v2::trace::EntryKind;
use crate::adaptive_v2::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
};
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, RootMap, SafepointId,
    SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
};
use crate::adaptive_v2::wxir_v2::{SnapshotId, VerifiedSnapshot};

fn add_snapshot() -> VerifiedSnapshot {
    let parameters = vec![
        ValueDef::new(ValueId::new(0), ValueType::I64),
        ValueDef::new(ValueId::new(1), ValueType::I64),
    ];
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            parameters,
            vec![
                Instruction::new(
                    InstructionKind::IntegerAdd,
                    vec![ValueId::new(0), ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(2), ValueType::I64)),
                    Effect::Pure,
                ),
                Instruction::new(
                    InstructionKind::Constant(Constant::Integer(1)),
                    vec![],
                    Some(ValueDef::new(ValueId::new(3), ValueType::I64)),
                    Effect::Pure,
                ),
                Instruction::new(
                    InstructionKind::IntegerAdd,
                    vec![ValueId::new(2), ValueId::new(3)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::I64)),
                    Effect::Pure,
                ),
            ],
            Terminator::Return {
                values: vec![ValueId::new(4)],
            },
        )],
        vec![],
        vec![],
        dependencies(7),
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("valid live-ready snapshot")
}

fn power_snapshot() -> VerifiedSnapshot {
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::F64),
                ValueDef::new(ValueId::new(1), ValueType::F64),
            ],
            vec![Instruction::new(
                InstructionKind::FloatPower,
                vec![ValueId::new(0), ValueId::new(1)],
                Some(ValueDef::new(ValueId::new(2), ValueType::F64)),
                Effect::Pure,
            )],
            Terminator::Return {
                values: vec![ValueId::new(2)],
            },
        )],
        vec![],
        vec![],
        dependencies(7),
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("valid float-power snapshot")
}

fn duplicate_add_snapshot() -> VerifiedSnapshot {
    let parameters = vec![
        ValueDef::new(ValueId::new(0), ValueType::I64),
        ValueDef::new(ValueId::new(1), ValueType::I64),
    ];
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            parameters,
            vec![
                Instruction::new(
                    InstructionKind::IntegerAdd,
                    vec![ValueId::new(0), ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(2), ValueType::I64)),
                    Effect::Pure,
                ),
                Instruction::new(
                    InstructionKind::IntegerAdd,
                    vec![ValueId::new(0), ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(3), ValueType::I64)),
                    Effect::Pure,
                ),
            ],
            Terminator::Return {
                values: vec![ValueId::new(3)],
            },
        )],
        vec![],
        vec![],
        dependencies(7),
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("valid duplicate-add snapshot")
}

fn guard_snapshot() -> VerifiedSnapshot {
    let point = SafepointId::new(7);
    let deps = dependencies(7);
    let recipe = DeoptRecipe::new(
        7,
        identity(),
        4,
        ResumeMode::ReplayBeforePc,
        vec![FrameRecipe::new(
            9,
            4,
            vec![RegisterRecipe::new(
                0,
                RegisterSource::Ssa(ValueId::new(0)),
                ValueType::Bool,
            )],
        )],
        point,
    )
    .with_dependencies(deps.clone());
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![ValueDef::new(ValueId::new(0), ValueType::Bool)],
            vec![Instruction::new(
                InstructionKind::Guard { guard: 7 },
                vec![ValueId::new(0)],
                None,
                Effect::Pure,
            )],
            Terminator::Return { values: vec![] },
        )],
        vec![RootMap::new(point, Default::default())],
        vec![recipe],
        deps,
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("valid guarded snapshot")
}

fn backedge_snapshot() -> VerifiedSnapshot {
    let point = SafepointId::new(11);
    let deps = dependencies(7);
    let recipe = DeoptRecipe::new(
        99,
        identity(),
        12,
        ResumeMode::ResumeAfterPc,
        vec![FrameRecipe::new(
            9,
            12,
            vec![RegisterRecipe::new(
                0,
                RegisterSource::Ssa(ValueId::new(0)),
                ValueType::I64,
            )],
        )],
        point,
    )
    .with_dependencies(deps.clone());
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::LoopHeader {
            header_pc: 12,
            backedge_pc: 19,
            preheader: None,
        },
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![ValueDef::new(ValueId::new(0), ValueType::I64)],
            vec![],
            Terminator::Backedge {
                target_pc: 12,
                safepoint: point,
            },
        )],
        vec![RootMap::new(point, Default::default())],
        vec![recipe],
        deps,
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("real backedge snapshot")
}

#[test]
fn cranelift_executes_verified_snapshot_native_direct() {
    // Given: an immutable verified snapshot produced with a live compile permit.
    let snapshot = add_snapshot();
    let mut compiler = NativeCompiler::new();

    // When: Tier 1 compiles and executes the snapshot.
    let code = compiler
        .compile_tier1(&snapshot)
        .expect("supported snapshot compiles");
    let outcome = code
        .execute(&[NativeValue::Integer(20), NativeValue::Integer(21)])
        .expect("native execution succeeds");

    // Then: actual machine code returns the expected value and no generic dispatcher ran.
    assert_eq!(outcome.values, vec![NativeValue::Integer(42)]);
    assert_eq!(outcome.counters.machine_entries, 1);
    assert_eq!(outcome.counters.generic_dispatch_calls, 0);
    assert_eq!(code.snapshot_id(), snapshot.id());
}

#[test]
fn native_boundary_rejects_wrong_arity_tags_pre_entry() {
    // Given: compiled code whose entry requires exactly two integers.
    let snapshot = add_snapshot();
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compile");

    // When: malformed callers provide too few, too many, or wrongly tagged values.
    let short = code.execute(&[NativeValue::Integer(1)]);
    let long = code.execute(&[
        NativeValue::Integer(1),
        NativeValue::Integer(2),
        NativeValue::Integer(3),
    ]);
    let wrong = code.execute(&[NativeValue::Boolean(true), NativeValue::Integer(2)]);

    // Then: every call is rejected before machine entry.
    assert!(short.is_err());
    assert!(long.is_err());
    assert!(wrong.is_err());
}

#[test]
fn guard_failure_returns_exact_deopt_meta() {
    // Given: a verified snapshot with a complete guard deopt recipe.
    let snapshot = guard_snapshot();
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compile");

    // When: native execution fails guard 7.
    let outcome = code
        .execute(&[NativeValue::Boolean(false)])
        .expect("deopt exit");

    // Then: the exit identifies the guard and counts a native deopt without dispatch.
    assert_eq!(outcome.guard_id, 7);
    assert_eq!(outcome.counters.deopts, 1);
    assert_eq!(outcome.counters.generic_dispatch_calls, 0);
}

#[test]
fn real_backedge_returns_osr_resume_meta() {
    let snapshot = backedge_snapshot();
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("backedge compile");
    let outcome = code
        .execute(&[NativeValue::Integer(41)])
        .expect("backedge exit");
    assert_eq!(
        (outcome.exit_id, outcome.safepoint_id, outcome.deopt_id),
        (12, 11, 99)
    );
    assert_eq!(outcome.counters.machine_entries, 1);
    assert_eq!(outcome.counters.generic_dispatch_calls, 0);
}

#[test]
#[cfg(feature = "inkwell")]
fn llvm_cranelift_publish_identical_real_backedge_exit() {
    let snapshot = backedge_snapshot();
    let mut compiler = NativeCompiler::new();
    let tier1 = compiler.compile_tier1(&snapshot).expect("backedge tier1");
    let tier1_outcome = tier1
        .execute(&[NativeValue::Integer(41)])
        .expect("tier1 exit");
    compiler
        .observe_tier1(&tier1_outcome)
        .expect("observed backedge tier1");
    let tier2 = compiler.compile_tier2(&snapshot).expect("backedge tier2");
    let tier2_outcome = tier2
        .execute(&[NativeValue::Integer(41)])
        .expect("tier2 exit");
    assert_eq!(tier1.snapshot_id(), tier2.snapshot_id());
    assert_eq!(
        (
            tier1_outcome.exit_id,
            tier1_outcome.safepoint_id,
            tier1_outcome.deopt_id
        ),
        (
            tier2_outcome.exit_id,
            tier2_outcome.safepoint_id,
            tier2_outcome.deopt_id
        ),
    );
}

#[test]
fn optimizer_derivations_ordered_deterministic_non_mutating() {
    // Given: one immutable verified snapshot and the staged optimizer.
    let snapshot = add_snapshot();
    let pipeline = OptimizerPipeline;

    // When: each cumulative gate is derived twice.
    for count in 0..=6 {
        let first = pipeline.run(&snapshot, count).expect("valid pass prefix");
        let second = pipeline
            .run(&snapshot, count)
            .expect("deterministic pass prefix");

        // Then: derivation IDs are deterministic and retain the original snapshot.
        assert_eq!(first.selected_id(), second.selected_id());
        assert_eq!(first.original_id(), snapshot.id());
        assert_eq!(first.verified().id(), snapshot.id());
        assert_eq!(first.passes().len(), count);
    }
}

#[test]
fn bridges_require_32_live_failures_cap_four_cases() {
    // Given: a parent snapshot and empty live bridge registry.
    let parent = add_snapshot().id();
    let mut registry = BridgeRegistry::default();

    // When: cached/static failures and only 31 live failures are observed.
    let first = BridgeKey {
        parent,
        guard: 9,
        observed_case: 1,
    };
    assert_eq!(
        registry.observe(first, FailureOrigin::Cached),
        BridgeDecision::Profiling(0)
    );
    assert_eq!(
        registry.observe(first, FailureOrigin::Static),
        BridgeDecision::Profiling(0)
    );
    for expected in 1..32 {
        assert_eq!(
            registry.observe(first, FailureOrigin::Live),
            BridgeDecision::Profiling(expected)
        );
    }
    let compiled = registry.observe(first, FailureOrigin::Live);

    // Then: the 32nd live failure compiles, four cases specialize, and the fifth is generic.
    assert!(matches!(compiled, BridgeDecision::Compile { key, .. } if key == first));
    for observed_case in 2..=4 {
        let key = BridgeKey {
            parent,
            guard: 9,
            observed_case,
        };
        for _ in 0..31 {
            let _ = registry.observe(key, FailureOrigin::Live);
        }
        assert!(matches!(
            registry.observe(key, FailureOrigin::Live),
            BridgeDecision::Compile { .. }
        ));
    }
    let fifth = BridgeKey {
        parent,
        guard: 9,
        observed_case: 5,
    };
    assert_eq!(
        registry.observe(fifth, FailureOrigin::Live),
        BridgeDecision::Generic
    );
}

#[test]
fn live_threshold_compiles_links_executes_separate_child_snapshot() {
    let parent = guard_snapshot();
    let child_source = add_snapshot();
    let mut bridges = BridgeRuntime::new(4, 4_096);
    for expected in 1..32 {
        assert_eq!(
            bridges.observe_and_link(&parent, 7, 1, FailureOrigin::Live, child_source.body()),
            BridgeLinkOutcome::Profiling(expected),
        );
    }
    let linked = bridges.observe_and_link(&parent, 7, 1, FailureOrigin::Live, child_source.body());
    let BridgeLinkOutcome::Linked(child_id) = linked else {
        panic!("bridge was not linked")
    };
    assert_ne!(child_id, parent.id());
    assert_eq!(child_source.body().parent, None);
    let outcome = bridges
        .execute_guard_target(
            parent.id(),
            7,
            1,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("patched bridge target")
        .expect("linked target");
    assert_eq!(outcome.values, vec![NativeValue::Integer(42)]);
    assert_eq!(outcome.counters.generic_dispatch_calls, 0);
    bridges.invalidate_parent(parent.id());
    assert!(
        bridges
            .execute_guard_target(
                parent.id(),
                7,
                1,
                &[NativeValue::Integer(20), NativeValue::Integer(21)],
            )
            .expect("invalidated lookup")
            .is_none()
    );
}

#[test]
fn invalid_bridge_body_keeps_parent_fallback() {
    let parent = guard_snapshot();
    let mut invalid = add_snapshot().body().clone();
    invalid.dependencies[0].observed_epoch = invalid.dependencies[0].observed_epoch.wrapping_add(1);
    let mut bridges = BridgeRuntime::new(4, 4_096);
    for _ in 0..31 {
        let _ = bridges.observe_and_link(&parent, 7, 2, FailureOrigin::Live, &invalid);
    }
    assert_eq!(
        bridges.observe_and_link(&parent, 7, 2, FailureOrigin::Live, &invalid),
        BridgeLinkOutcome::Fallback,
    );
    assert!(
        bridges
            .execute_guard_target(parent.id(), 7, 2, &[])
            .expect("fallback lookup")
            .is_none()
    );
}

#[test]
#[cfg(feature = "inkwell")]
fn bridge_child_uses_same_snapshot_in_cranelift_llvm() {
    let parent = guard_snapshot();
    let child_source = duplicate_add_snapshot();
    let original_body = child_source.body().clone();
    let unselected = parent
        .derive_bridge(7, 3, child_source.body().clone())
        .expect("unselected bridge child");
    let mut bridges = BridgeRuntime::new(4, 4_096);
    for _ in 0..31 {
        assert!(matches!(
            bridges.observe_and_link(&parent, 7, 3, FailureOrigin::Live, child_source.body()),
            BridgeLinkOutcome::Profiling(_)
        ));
    }
    let BridgeLinkOutcome::Linked(linked_id) =
        bridges.observe_and_link(&parent, 7, 3, FailureOrigin::Live, child_source.body())
    else {
        panic!("bridge was not linked")
    };
    let tier1 = bridges
        .execute_guard_target(
            parent.id(),
            7,
            3,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("bridge tier1")
        .expect("linked bridge");
    let tier2 = bridges
        .execute_tier2(
            parent.id(),
            7,
            3,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("bridge tier2");
    assert_ne!(linked_id, unselected.id());
    assert_eq!(linked_id, tier1.snapshot_id());
    assert_eq!(linked_id, tier2.snapshot_id());
    assert_eq!(child_source.body(), &original_body);
    assert_eq!(tier1.values, tier2.values);
    assert_eq!(tier2.values, vec![NativeValue::Integer(41)]);
    bridges.invalidate_parent(parent.id());
    assert!(
        bridges
            .execute_guard_target(
                parent.id(),
                7,
                3,
                &[NativeValue::Integer(20), NativeValue::Integer(21)],
            )
            .expect("invalidated bridge lookup")
            .is_none()
    );
    assert_eq!(
        bridges.execute_tier2(
            parent.id(),
            7,
            3,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        ),
        Err(super::super::native::NativeError::Tier1NotObserved)
    );
}

#[test]
fn tiered_bridge_cache_key_matches_selected_native_receipt() {
    let parent = duplicate_add_snapshot();
    let original_body = parent.body().clone();
    let mut compiler = NativeCompiler::new();
    let code = compiler.compile_tier1(&parent).expect("parent tier1");
    let mut attempted = code
        .execute(&[NativeValue::Integer(20), NativeValue::Integer(21)])
        .expect("parent execution");
    attempted.guard_id = 7;
    let derivation = SnapshotId::from_bytes([17; 32]);
    let mut bridge = BridgeSite::new(derivation);
    for _ in 0..31 {
        let observation = bridge
            .observe(
                &parent,
                &mut compiler,
                &attempted,
                &[NativeValue::Integer(20), NativeValue::Integer(21)],
            )
            .expect("bridge profiling");
        assert!(observation.child.is_none());
    }
    let observation = bridge
        .observe(
            &parent,
            &mut compiler,
            &attempted,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("bridge linking");
    let key = observation.child.expect("selected bridge cache key");
    let tier1 = execute_cached_bridge(key, &[NativeValue::Integer(20), NativeValue::Integer(21)])
        .expect("selected bridge cache entry");
    let cached = execute_cached_bridge(key, &[NativeValue::Integer(20), NativeValue::Integer(21)])
        .expect("selected bridge cache hit");
    assert_eq!(key.snapshot, tier1.snapshot_id());
    assert_eq!(key.snapshot, cached.snapshot_id());
    assert_eq!(tier1.values, vec![NativeValue::Integer(41)]);
    assert_eq!(parent.body(), &original_body);
}

#[test]
fn code_cache_never_evicts_active_code_invalidates_stale_epochs() {
    // Given: a one-entry cache containing active Tier 1 code metadata.
    let snapshot = add_snapshot();
    let first = snapshot.id();
    let second = OptimizerPipeline
        .run(&snapshot, 1)
        .expect("derived")
        .selected_id();
    let first_key = CacheKey::new(
        first,
        &snapshot.body().dependencies,
        NativeTier::Cranelift,
        first,
    );
    let second_key = CacheKey::new(
        first,
        &snapshot.body().dependencies,
        NativeTier::Llvm,
        second,
    );
    let cache = SharedCodeCache::new(1, 64);
    let first_lease = cache.insert_and_lease(first_key, 32, 1, "first");
    assert_eq!(first_lease.with(|value| *value), Some("first"));

    // When: another entry is inserted while the first is active.
    let second_lease = cache.insert_and_lease(second_key, 32, 2, "second");

    // Then: the inactive newcomer is evicted, then stale inactive code is removable.
    assert!(cache.drain_evicted().is_empty());
    assert!(cache.contains(first_key));
    assert!(cache.contains(second_key));
    drop(second_lease);
    assert_eq!(cache.drain_evicted(), vec!["second"]);
    assert!(cache.contains(first_key));
    drop(first_lease);
}

#[test]
fn bounded_cache_owns_executes_real_native_code() {
    let snapshot = add_snapshot();
    let mut executor = CachedNativeExecutor::new(2, 1_024);
    let first = executor
        .execute_tier1(
            &snapshot,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("cached native execution");
    let second = executor
        .execute_tier1(
            &snapshot,
            &[NativeValue::Integer(19), NativeValue::Integer(22)],
        )
        .expect("cache hit execution");
    assert_eq!(first.values, vec![NativeValue::Integer(42)]);
    assert_eq!(second.values, vec![NativeValue::Integer(42)]);
    assert_eq!(executor.cached_tiers(&snapshot), (true, false));
    executor.invalidate(snapshot.id());
    assert_eq!(executor.cached_tiers(&snapshot), (false, false));
}

#[test]
fn concurrent_cache_leases_keep_active_entries() {
    let snapshot = add_snapshot();
    let key = CacheKey::new(
        snapshot.id(),
        &snapshot.body().dependencies,
        NativeTier::Cranelift,
        snapshot.id(),
    );
    let cache = SharedCodeCache::new(1, 64);
    cache.insert(key, 32, 7, 0_u64);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let cache = cache.clone();
            scope.spawn(move || {
                for _ in 0..128 {
                    let lease = cache.lease(key).expect("live cache entry");
                    assert!(
                        lease
                            .with(|value| {
                                *value += 1;
                                *value
                            })
                            .is_some()
                    );
                }
            });
        }
    });
    let lease = cache.lease(key).expect("entry remains cached");
    assert_eq!(lease.with(|value| *value), Some(512));
}

#[test]
#[cfg(feature = "inkwell")]
fn task5_matching_surface_driver_metrics() {
    let snapshot = add_snapshot();
    let mut compile_samples = Vec::new();
    let mut cold_samples = Vec::new();
    let mut warm_samples = Vec::new();
    let mut cold = None;
    let mut warm = None;
    for _ in 0..7 {
        let compile_started = std::time::Instant::now();
        let tier1 = NativeCompiler::new()
            .compile_tier1(&snapshot)
            .expect("driver tier1 compile");
        compile_samples.push(compile_started.elapsed().as_nanos());
        let cold_started = std::time::Instant::now();
        cold = Some(
            tier1
                .execute(&[NativeValue::Integer(20), NativeValue::Integer(21)])
                .expect("driver cold"),
        );
        cold_samples.push(cold_started.elapsed().as_nanos());
        let warm_started = std::time::Instant::now();
        for _ in 0..1_000 {
            warm = Some(
                tier1
                    .execute(&[NativeValue::Integer(20), NativeValue::Integer(21)])
                    .expect("driver warm"),
            );
        }
        warm_samples.push(warm_started.elapsed().as_nanos());
    }
    compile_samples.sort_unstable();
    cold_samples.sort_unstable();
    warm_samples.sort_unstable();
    let cold = cold.expect("cold outcome");
    let warm = warm.expect("warm outcome");

    let guard = guard_snapshot();
    let guard_code = NativeCompiler::new()
        .compile_tier1(&guard)
        .expect("driver guard compile");
    let deopt = guard_code
        .execute(&[NativeValue::Boolean(false)])
        .expect("driver deopt");
    let mut bridges = BridgeRuntime::new(4, 4_096);
    for _ in 0..32 {
        let _ = bridges.observe_and_link(&guard, 7, 1, FailureOrigin::Live, snapshot.body());
    }
    let bridge = bridges
        .execute_guard_target(
            guard.id(),
            7,
            1,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("driver bridge")
        .expect("driver linked bridge");
    let optimized = OptimizerPipeline
        .run(&snapshot, 6)
        .expect("driver optimizer");
    let mut executor = CachedNativeExecutor::new(2, 4_096);
    let selected_t1 = executor
        .execute_tier1(
            optimized.verified(),
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("driver selected tier1");
    let selected_t2 = executor
        .execute_tier2(
            optimized.verified(),
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("driver selected tier2");
    executor.invalidate(optimized.selected_id());

    assert_eq!(cold.values, vec![NativeValue::Integer(42)]);
    assert_eq!(warm.values, cold.values);
    assert_eq!(cold.counters.generic_dispatch_calls, 0);
    assert_eq!((deopt.guard_id, deopt.deopt_id), (7, 7));
    assert_eq!(bridge.values, cold.values);
    assert_eq!(selected_t1.values, selected_t2.values);
    assert_eq!(executor.cached_tiers(optimized.verified()), (false, false));
    eprintln!(
        "task5_driver result=42 compile_median_us={} cold_median_ns={} warm_1000_median_us={} machine_entries={} helpers={} generic={} deopts={}",
        compile_samples[3] / 1_000,
        cold_samples[3],
        warm_samples[3] / 1_000,
        warm.counters.machine_entries,
        warm.counters.helper_calls,
        warm.counters.generic_dispatch_calls,
        deopt.counters.deopts,
    );
}

#[test]
#[cfg(feature = "inkwell")]
fn bounded_cache_executes_both_tiers_from_exact_snapshot() {
    let snapshot = add_snapshot();
    let mut executor = CachedNativeExecutor::new(2, 2_048);
    let tier1 = executor
        .execute_tier1(
            &snapshot,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("cached tier1");
    let tier2 = executor
        .execute_tier2(
            &snapshot,
            &[NativeValue::Integer(20), NativeValue::Integer(21)],
        )
        .expect("cached tier2");
    assert_eq!(tier1.values, tier2.values);
    assert_eq!(executor.cached_tiers(&snapshot), (true, true));
}

#[test]
#[cfg(feature = "inkwell")]
fn llvm_o3_requires_observed_tier1_executes_same_snapshot() {
    // Given: one verified snapshot that has not yet executed in Tier 1.
    let snapshot = add_snapshot();
    let mut compiler = NativeCompiler::new();
    assert!(compiler.compile_tier2(&snapshot).is_err());
    let tier1 = compiler.compile_tier1(&snapshot).expect("Tier 1 compile");

    // When: Tier 1 executes and the exact snapshot ID is marked observed.
    let tier1_outcome = tier1
        .execute(&[NativeValue::Integer(20), NativeValue::Integer(21)])
        .expect("Tier 1 execution");
    compiler
        .observe_tier1(&tier1_outcome)
        .expect("observed Tier 1 receipt");
    let tier2 = compiler.compile_tier2(&snapshot).expect("LLVM O3 compile");
    let tier2_outcome = tier2
        .execute(&[NativeValue::Integer(20), NativeValue::Integer(21)])
        .expect("LLVM execution");

    // Then: both native tiers consume the exact ID and return identical results and counters.
    assert_eq!(tier1.snapshot_id(), tier2.snapshot_id());
    assert_eq!(tier1_outcome.values, tier2_outcome.values);
    assert_eq!(tier2_outcome.values, vec![NativeValue::Integer(42)]);
    assert_eq!(tier2_outcome.counters.machine_entries, 1);
    assert_eq!(tier2_outcome.counters.generic_dispatch_calls, 0);
}

#[test]
#[cfg(feature = "inkwell")]
fn llvm_cranelift_execute_identical_float_power_snapshot() {
    let snapshot = power_snapshot();
    let mut compiler = NativeCompiler::new();
    let tier1 = compiler.compile_tier1(&snapshot).expect("Tier 1 compile");
    let inputs = [
        NativeValue::FloatBits(4.0_f64.to_bits()),
        NativeValue::FloatBits((-1.5_f64).to_bits()),
    ];
    let tier1_outcome = tier1.execute(&inputs).expect("Tier 1 power");
    compiler
        .observe_tier1(&tier1_outcome)
        .expect("observed Tier 1 receipt");
    let tier2 = compiler
        .compile_tier2(&snapshot)
        .expect("LLVM power compile");
    let tier2_outcome = tier2.execute(&inputs).expect("LLVM power");

    assert_eq!(tier1.snapshot_id(), tier2.snapshot_id());
    assert_eq!(tier1_outcome.values, tier2_outcome.values);
    assert_eq!(
        tier2_outcome.values,
        vec![NativeValue::FloatBits(0.125_f64.to_bits())]
    );
    assert_eq!(tier1_outcome.counters.machine_entries, 1);
    assert_eq!(tier2_outcome.counters.machine_entries, 1);
    assert_eq!(tier1_outcome.counters.helper_calls, 0);
    assert_eq!(tier2_outcome.counters.helper_calls, 0);
}

#[test]
#[cfg(feature = "inkwell")]
fn llvm_cranelift_publish_identical_guard_deopt() {
    // Given: a verified guard snapshot and false condition.
    let snapshot = guard_snapshot();
    let mut compiler = NativeCompiler::new();
    let tier1 = compiler.compile_tier1(&snapshot).expect("Tier 1 compile");

    // When: both tiers take the same guard failure.
    let tier1 = tier1
        .execute(&[NativeValue::Boolean(false)])
        .expect("Tier 1 guard");
    compiler.observe_tier1(&tier1).expect("Tier 1 receipt");
    let tier2 = compiler.compile_tier2(&snapshot).expect("LLVM compile");
    let tier2 = tier2
        .execute(&[NativeValue::Boolean(false)])
        .expect("LLVM guard");

    // Then: guard, deopt, root-map, counters, and snapshot behavior agree.
    assert_eq!(tier1.guard_id, tier2.guard_id);
    assert_eq!(tier1.deopt_id, tier2.deopt_id);
    assert_eq!(tier1.safepoint_id, tier2.safepoint_id);
    assert_eq!(tier1.counters.deopts, tier2.counters.deopts);
    assert_eq!(tier2.counters.generic_dispatch_calls, 0);
}

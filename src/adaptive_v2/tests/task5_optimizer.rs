use super::super::native::optimizer::{OptimizationPass, OptimizerPipeline, VerifiedCallee};
use super::super::native::{NativeCompiler, NativeRuntime, NativeValue};
use super::super::trace::EntryKind;
use super::super::wxir_v2::VerifiedSnapshot;
use super::super::wxir_v2::deopt::{DeoptRecipe, FrameRecipe, ResumeMode};
use super::super::wxir_v2::dependency::{Dependency, DependencyKind};
use super::super::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, RootMap, SafepointId,
    SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
};
use super::super::wxir_v2::replay::{ReplayHeap, ReplayOutcome, ReplayValue, replay};
use super::task4_support::{compile_permit, dependencies, identity};

fn foldable_snapshot() -> VerifiedSnapshot {
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![],
            vec![
                Instruction::new(
                    InstructionKind::Constant(Constant::Integer(20)),
                    vec![],
                    Some(ValueDef::new(ValueId::new(0), ValueType::I64)),
                    Effect::Pure,
                ),
                Instruction::new(
                    InstructionKind::Constant(Constant::Integer(22)),
                    vec![],
                    Some(ValueDef::new(ValueId::new(1), ValueType::I64)),
                    Effect::Pure,
                ),
                Instruction::new(
                    InstructionKind::IntegerAdd,
                    vec![ValueId::new(0), ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(2), ValueType::I64)),
                    Effect::Pure,
                ),
            ],
            Terminator::Return {
                values: vec![ValueId::new(2)],
            },
        )],
        vec![],
        vec![],
        dependencies(7),
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("foldable snapshot")
}

#[test]
fn propagation_pass_creates_reverified_snapshot_keeps_semantics() {
    // Given: an immutable snapshot with two constant operands and an add.
    let original = foldable_snapshot();

    // When: propagation/folding is enabled and the derived snapshot executes.
    let optimized = OptimizerPipeline
        .run(&original, 1)
        .expect("optimized snapshot");
    let original_replay = replay(&original, &[], &mut ReplayHeap::default());
    let optimized_replay = replay(optimized.verified(), &[], &mut ReplayHeap::default());
    let code = NativeCompiler::new()
        .compile_tier1(optimized.verified())
        .expect("derived snapshot compiles");
    let native = code.execute(&[]).expect("derived snapshot executes");

    // Then: the body and ID changed, both semantic models agree, and native returns 42.
    assert_ne!(optimized.selected_id(), original.id());
    assert_eq!(
        optimized.reports()[0].pass,
        OptimizationPass::PropagateAndFold
    );
    assert!(optimized.reports()[0].changed);
    assert_eq!(
        original_replay,
        ReplayOutcome::Return(vec![ReplayValue::Integer(42)])
    );
    assert_eq!(optimized_replay, original_replay);
    assert_eq!(native.values, vec![NativeValue::Integer(42)]);
    assert_eq!(code.snapshot_id(), optimized.selected_id());
}

#[test]
fn inapplicable_passes_report_noop() {
    // Given: a scalar-only snapshot with no barriers or heap operations.
    let original = foldable_snapshot();

    // When: all six ordered passes run.
    let optimized = OptimizerPipeline.run(&original, 6).expect("full pipeline");

    // Then: every later inapplicable pass reports a verified no-op instead of a fake transform.
    assert_eq!(optimized.reports().len(), 6);
    for report in &optimized.reports()[1..] {
        assert!(!report.changed);
        assert!(!report.blocked_by_barrier);
    }
}

fn optimizer_dependencies() -> Vec<Dependency> {
    let mut result = dependencies(7);
    result.extend([
        Dependency::current(DependencyKind::Shape, 1, 1),
        Dependency::current(DependencyKind::Class, 1, 1),
        Dependency::current(DependencyKind::Callee, 1, 1),
    ]);
    result
}

fn seal(block: Block, roots: Vec<RootMap>, deopts: Vec<DeoptRecipe>) -> VerifiedSnapshot {
    VerifiedSnapshot::seal(
        SnapshotDraft::new(
            identity(),
            EntryKind::FunctionEntry,
            BlockId::new(0),
            vec![block],
            roots,
            deopts,
            optimizer_dependencies(),
        )
        .with_schema_epoch(7),
        compile_permit(7),
    )
    .expect("optimizer fixture")
}

fn empty_deopt(id: u32, point: SafepointId) -> DeoptRecipe {
    DeoptRecipe::new(
        id,
        identity(),
        id,
        ResumeMode::ReplayBeforePc,
        vec![FrameRecipe::new(9, id, vec![])],
        point,
    )
    .with_dependencies(optimizer_dependencies())
}

fn add_callee_snapshot(epoch: u64) -> VerifiedSnapshot {
    let executable = super::super::trace::ExecutableIdentity::new(1, epoch);
    let deps = vec![
        Dependency::current(DependencyKind::Executable, 1, epoch),
        Dependency::current(DependencyKind::Schema, 7, 7),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
    ];
    let draft = SnapshotDraft::new(
        executable,
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::I64),
                ValueDef::new(ValueId::new(1), ValueType::I64),
            ],
            vec![Instruction::new(
                InstructionKind::IntegerAdd,
                vec![ValueId::new(0), ValueId::new(1)],
                Some(ValueDef::new(ValueId::new(2), ValueType::I64)),
                Effect::Pure,
            )],
            Terminator::Return {
                values: vec![ValueId::new(2)],
            },
        )],
        vec![],
        vec![],
        deps,
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("verified add callee")
}

fn multiply(left: i64, right: i64) -> i64 {
    left * right
}

#[test]
fn every_pipeline_stage_performs_real_bounded_transform() {
    let direct = seal(
        Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::Handle),
                ValueDef::new(ValueId::new(1), ValueType::I64),
            ],
            vec![Instruction::new(
                InstructionKind::ObjectGet.at_pc(7),
                vec![ValueId::new(0), ValueId::new(1)],
                Some(ValueDef::new(ValueId::new(2), ValueType::I64)),
                Effect::Read,
            )],
            Terminator::Return {
                values: vec![ValueId::new(2)],
            },
        ),
        vec![],
        vec![],
    );
    let direct_result = OptimizerPipeline.run(&direct, 2).expect("direct lowering");
    assert!(direct_result.reports()[1].changed);

    let point = SafepointId::new(3);
    let inline = seal(
        Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::Bool),
                ValueDef::new(ValueId::new(1), ValueType::I64),
                ValueDef::new(ValueId::new(2), ValueType::I64),
            ],
            vec![
                Instruction::new(
                    InstructionKind::Guard { guard: 3 },
                    vec![ValueId::new(0)],
                    None,
                    Effect::Pure,
                ),
                Instruction::safepoint(
                    InstructionKind::Call { callee: 1 },
                    vec![ValueId::new(1), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(3), ValueType::I64)),
                    Effect::Call,
                    point,
                )
                .ordered(0),
            ],
            Terminator::Return {
                values: vec![ValueId::new(3)],
            },
        ),
        vec![RootMap::new(point, Default::default())],
        vec![empty_deopt(3, point)],
    );
    let unproved = OptimizerPipeline.run(&inline, 3).expect("no proof");
    assert!(!unproved.reports()[2].changed);
    let mut runtime = NativeRuntime::default();
    runtime.insert_call(1, multiply);
    let unproved_code = NativeCompiler::new()
        .compile_tier1(unproved.verified())
        .expect("unproved call");
    assert_eq!(
        unproved_code
            .execute_with_heap(
                &[
                    NativeValue::Boolean(true),
                    NativeValue::Integer(20),
                    NativeValue::Integer(22)
                ],
                &mut runtime,
            )
            .expect("arbitrary callee remains a call")
            .values,
        vec![NativeValue::Integer(440)],
    );
    let wrong_epoch =
        VerifiedCallee::prove_add(1, &add_callee_snapshot(2)).expect("different epoch proof");
    assert!(
        !OptimizerPipeline
            .run_with_callees(&inline, 3, &[wrong_epoch])
            .expect("epoch mismatch")
            .reports()[2]
            .changed
    );
    let proof =
        VerifiedCallee::prove_add(1, &add_callee_snapshot(1)).expect("verified callee proof");
    let inline_result = OptimizerPipeline
        .run_with_callees(&inline, 3, &[proof])
        .expect("guarded inline");
    assert!(inline_result.reports()[2].changed);
    let inline_code = NativeCompiler::new()
        .compile_tier1(inline_result.verified())
        .expect("inline code");
    assert_eq!(
        inline_code
            .execute(&[
                NativeValue::Boolean(true),
                NativeValue::Integer(20),
                NativeValue::Integer(22)
            ])
            .expect("inline execute")
            .values,
        vec![NativeValue::Integer(42)]
    );
    let deopt = inline_code
        .execute(&[
            NativeValue::Boolean(false),
            NativeValue::Integer(20),
            NativeValue::Integer(22),
        ])
        .expect("forced deopt");
    assert_eq!(
        (deopt.guard_id, deopt.safepoint_id, deopt.deopt_id),
        (3, 3, 3)
    );

    let allocation_point = SafepointId::new(4);
    let scalar = seal(
        Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::I64),
                ValueDef::new(ValueId::new(1), ValueType::I64),
            ],
            vec![
                Instruction::safepoint(
                    InstructionKind::Allocate,
                    vec![],
                    Some(ValueDef::new(ValueId::new(2), ValueType::Handle)),
                    Effect::Allocation,
                    allocation_point,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ObjectSet,
                    vec![ValueId::new(2), ValueId::new(0), ValueId::new(1)],
                    None,
                    Effect::Write,
                )
                .ordered(1),
                Instruction::new(
                    InstructionKind::ObjectGet,
                    vec![ValueId::new(2), ValueId::new(0)],
                    Some(ValueDef::new(ValueId::new(3), ValueType::I64)),
                    Effect::Read,
                ),
            ],
            Terminator::Return {
                values: vec![ValueId::new(3)],
            },
        ),
        vec![RootMap::new(allocation_point, Default::default())],
        vec![empty_deopt(4, allocation_point)],
    );
    assert_eq!(
        replay(
            &scalar,
            &[ReplayValue::Integer(7), ReplayValue::Integer(42)],
            &mut ReplayHeap::default()
        ),
        ReplayOutcome::Return(vec![ReplayValue::Integer(42)])
    );
    let scalar_result = OptimizerPipeline
        .run(&scalar, 4)
        .expect("scalar replacement");
    assert!(scalar_result.reports()[3].changed);
    assert_eq!(
        replay(
            scalar_result.verified(),
            &[ReplayValue::Integer(7), ReplayValue::Integer(42)],
            &mut ReplayHeap::default()
        ),
        ReplayOutcome::Return(vec![ReplayValue::Integer(42)])
    );

    let forward = object_set_get(false);
    let forward_result = OptimizerPipeline.run(&forward, 5).expect("heap forwarding");
    assert!(forward_result.reports()[4].changed);
    let gvn = duplicate_adds();
    let gvn_result = OptimizerPipeline.run(&gvn, 6).expect("gvn");
    assert!(gvn_result.reports()[5].changed);
    assert_eq!(
        replay(
            gvn_result.verified(),
            &[ReplayValue::Integer(20), ReplayValue::Integer(22)],
            &mut ReplayHeap::default()
        ),
        ReplayOutcome::Return(vec![ReplayValue::Integer(42)])
    );
}

fn object_set_get(with_barrier: bool) -> VerifiedSnapshot {
    let point = SafepointId::new(8);
    let mut instructions = vec![
        Instruction::new(
            InstructionKind::ObjectSet,
            vec![ValueId::new(0), ValueId::new(1), ValueId::new(2)],
            None,
            Effect::Write,
        )
        .ordered(0),
    ];
    if with_barrier {
        instructions.push(
            Instruction::safepoint(
                InstructionKind::Helper { helper: 9 },
                vec![],
                None,
                Effect::Helper,
                point,
            )
            .ordered(1),
        );
    }
    instructions.push(Instruction::new(
        InstructionKind::ObjectGet,
        vec![ValueId::new(0), ValueId::new(1)],
        Some(ValueDef::new(ValueId::new(3), ValueType::I64)),
        Effect::Read,
    ));
    seal(
        Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::Handle),
                ValueDef::new(ValueId::new(1), ValueType::I64),
                ValueDef::new(ValueId::new(2), ValueType::I64),
            ],
            instructions,
            Terminator::Return {
                values: vec![ValueId::new(3)],
            },
        ),
        if with_barrier {
            vec![RootMap::new(point, Default::default())]
        } else {
            vec![]
        },
        if with_barrier {
            vec![empty_deopt(8, point)]
        } else {
            vec![]
        },
    )
}

#[test]
fn scalar_replacement_forwards_nonescaping_fields() {
    let point = SafepointId::new(12);
    let snapshot = seal(
        Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::I64),
                ValueDef::new(ValueId::new(1), ValueType::I64),
                ValueDef::new(ValueId::new(2), ValueType::I64),
                ValueDef::new(ValueId::new(3), ValueType::I64),
            ],
            vec![
                Instruction::safepoint(
                    InstructionKind::Allocate,
                    vec![],
                    Some(ValueDef::new(ValueId::new(4), ValueType::Handle)),
                    Effect::Allocation,
                    point,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ObjectSet,
                    vec![ValueId::new(4), ValueId::new(0), ValueId::new(1)],
                    None,
                    Effect::Write,
                )
                .ordered(1),
                Instruction::new(
                    InstructionKind::ObjectSet,
                    vec![ValueId::new(4), ValueId::new(2), ValueId::new(3)],
                    None,
                    Effect::Write,
                )
                .ordered(2),
                Instruction::new(
                    InstructionKind::ObjectGet,
                    vec![ValueId::new(4), ValueId::new(0)],
                    Some(ValueDef::new(ValueId::new(5), ValueType::I64)),
                    Effect::Read,
                ),
                Instruction::new(
                    InstructionKind::ObjectGet,
                    vec![ValueId::new(4), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(6), ValueType::I64)),
                    Effect::Read,
                ),
                Instruction::new(
                    InstructionKind::IntegerAdd,
                    vec![ValueId::new(5), ValueId::new(6)],
                    Some(ValueDef::new(ValueId::new(7), ValueType::I64)),
                    Effect::Pure,
                ),
            ],
            Terminator::Return {
                values: vec![ValueId::new(7)],
            },
        ),
        vec![RootMap::new(point, Default::default())],
        vec![empty_deopt(12, point)],
    );

    let optimized = OptimizerPipeline
        .run(&snapshot, 4)
        .expect("multi-field scalar replacement");
    assert!(optimized.reports()[3].changed);
    assert_eq!(
        replay(
            optimized.verified(),
            &[
                ReplayValue::Integer(10),
                ReplayValue::Integer(20),
                ReplayValue::Integer(11),
                ReplayValue::Integer(22),
            ],
            &mut ReplayHeap::default(),
        ),
        ReplayOutcome::Return(vec![ReplayValue::Integer(42)])
    );
}

fn duplicate_adds() -> VerifiedSnapshot {
    seal(
        Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::I64),
                ValueDef::new(ValueId::new(1), ValueType::I64),
            ],
            vec![
                Instruction::new(
                    InstructionKind::IntegerAdd,
                    vec![ValueId::new(0), ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(2), ValueType::I64)),
                    Effect::Pure,
                ),
                Instruction::new(
                    InstructionKind::IntegerAdd,
                    vec![ValueId::new(1), ValueId::new(0)],
                    Some(ValueDef::new(ValueId::new(3), ValueType::I64)),
                    Effect::Pure,
                ),
            ],
            Terminator::Return {
                values: vec![ValueId::new(3)],
            },
        ),
        vec![],
        vec![],
    )
}

#[test]
fn tier1_selects_licm_gvn_without_input_mutation() {
    let original = duplicate_adds();
    let original_id = original.id();
    let original_body = original.body().clone();
    let mut compiler = NativeCompiler::new();
    let tier1 = compiler
        .compile_tier1(&original)
        .expect("ordinary tier1 compile");
    let tier1_outcome = tier1
        .execute(&[NativeValue::Integer(20), NativeValue::Integer(22)])
        .expect("ordinary tier1 execute");

    assert_ne!(tier1.snapshot_id(), original_id);
    assert_eq!(original.id(), original_id);
    assert_eq!(original.body(), &original_body);
    assert_eq!(tier1_outcome.values, vec![NativeValue::Integer(42)]);
    compiler
        .observe_tier1(&tier1_outcome)
        .expect("observe selected tier1");

    #[cfg(feature = "inkwell")]
    {
        let tier2 = compiler
            .compile_tier2(&original)
            .expect("tier2 consumes selected snapshot");
        let tier2_outcome = tier2
            .execute(&[NativeValue::Integer(20), NativeValue::Integer(22)])
            .expect("selected tier2 execute");
        assert_eq!(tier2.snapshot_id(), tier1.snapshot_id());
        assert_eq!(tier2_outcome.values, tier1_outcome.values);

        let mut cached = super::super::native::CachedNativeExecutor::new(4, 8_192);
        let cached_tier1 = cached
            .execute_tier1(
                &original,
                &[NativeValue::Integer(20), NativeValue::Integer(22)],
            )
            .expect("cached selected tier1");
        let cached_tier2 = cached
            .execute_tier2(
                &original,
                &[NativeValue::Integer(20), NativeValue::Integer(22)],
            )
            .expect("cached selected tier2");
        assert_eq!(cached_tier2.values, cached_tier1.values);
        assert_eq!(cached.cached_tiers(&original), (true, true));

        let unrelated = foldable_snapshot();
        cached
            .execute_tier1(&unrelated, &[])
            .expect("unrelated cached tier1");
        assert_eq!(cached.cached_tiers(&unrelated), (true, false));

        cached.invalidate(cached_tier1.snapshot_id());
        assert_eq!(cached.cached_tiers(&original), (false, false));
        assert_eq!(cached.cached_tiers(&unrelated), (true, false));
        cached.invalidate(cached_tier1.snapshot_id());
        assert_eq!(cached.cached_tiers(&original), (false, false));

        cached
            .execute_tier1(
                &original,
                &[NativeValue::Integer(20), NativeValue::Integer(22)],
            )
            .expect("repopulated selected tier1");
        cached
            .execute_tier2(
                &original,
                &[NativeValue::Integer(20), NativeValue::Integer(22)],
            )
            .expect("repopulated selected tier2");
        cached.invalidate(original_id);
        assert_eq!(cached.cached_tiers(&original), (false, false));
        assert_eq!(cached.cached_tiers(&unrelated), (true, false));
        cached.invalidate(original_id);
        assert_eq!(cached.cached_tiers(&original), (false, false));
    }
}

#[test]
fn separate_compilers_keep_distinct_tier1_dumps() {
    struct ClifDumpGuard {
        directory: std::path::PathBuf,
    }

    impl Drop for ClifDumpGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn snapshot_hex(snapshot: super::super::wxir_v2::SnapshotId) -> String {
        snapshot
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    const CHILD: &str = "WUSTITE_ADAPTIVE_V2_SYMBOL_TEST_CHILD";
    const TEST_NAME: &str =
        "adaptive_v2::tests::task5_optimizer::separate_compilers_keep_distinct_tier1_dumps";
    if std::env::var_os(CHILD).is_none() {
        let directory = std::env::temp_dir().join(format!(
            "wustite-adaptive-v2-symbol-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create isolated CLIF dump directory");
        let _guard = ClifDumpGuard {
            directory: directory.clone(),
        };
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD, "1")
            .env("WUSTITE_ADAPTIVE_V2_CLIF_DIR", &directory)
            .output()
            .expect("run isolated compiler test process");
        assert!(
            output.status.success(),
            "child stdout:\n{}\nchild stderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let directory = std::path::PathBuf::from(
        std::env::var_os("WUSTITE_ADAPTIVE_V2_CLIF_DIR").expect("child CLIF dump directory"),
    );

    let snapshots = [foldable_snapshot(), duplicate_adds()];
    let compiled = std::thread::scope(|scope| {
        snapshots
            .into_iter()
            .map(|snapshot| {
                scope.spawn(move || {
                    let code = NativeCompiler::new()
                        .compile_tier1(&snapshot)
                        .expect("compile distinct snapshot in fresh compiler");
                    code.snapshot_id()
                })
            })
            .map(|thread| thread.join().expect("compiler thread"))
            .collect::<Vec<_>>()
    });
    assert_ne!(compiled[0], compiled[1]);

    let paths = compiled
        .iter()
        .map(|id| directory.join(format!("adaptive_v2_t1_{}.clif", snapshot_hex(*id))))
        .collect::<Vec<_>>();
    assert_ne!(paths[0], paths[1]);
    assert!(paths.iter().all(|path| path.is_file()), "{paths:?}");
    assert_ne!(
        std::fs::read(&paths[0]).expect("first CLIF"),
        std::fs::read(&paths[1]).expect("second CLIF")
    );
}

#[test]
fn helper_barrier_blocks_heap_forwarding() {
    let snapshot = object_set_get(true);
    let optimized = OptimizerPipeline
        .run(&snapshot, 5)
        .expect("barrier analysis");
    assert!(!optimized.reports()[4].changed);
    assert!(optimized.reports()[4].blocked_by_barrier);
}

fn counted_loop(blocked: bool) -> VerifiedSnapshot {
    let mut latch = vec![Instruction::new(
        InstructionKind::IntegerAdd,
        vec![ValueId::new(0), ValueId::new(1)],
        Some(ValueDef::new(ValueId::new(6), ValueType::I64)),
        Effect::Pure,
    )];
    if blocked {
        latch.insert(
            0,
            Instruction::new(InstructionKind::LiveProbe, vec![], None, Effect::Read),
        );
    }
    latch.push(Instruction::new(
        InstructionKind::Constant(Constant::Boolean(false)),
        vec![],
        Some(ValueDef::new(ValueId::new(7), ValueType::Bool)),
        Effect::Pure,
    ));
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::LoopHeader {
            header_pc: 1,
            backedge_pc: 2,
            preheader: None,
        },
        BlockId::new(0),
        vec![
            Block::new(
                BlockId::new(0),
                vec![
                    ValueDef::new(ValueId::new(0), ValueType::I64),
                    ValueDef::new(ValueId::new(1), ValueType::I64),
                ],
                vec![
                    Instruction::new(
                        InstructionKind::Constant(Constant::Boolean(true)),
                        vec![],
                        Some(ValueDef::new(ValueId::new(2), ValueType::Bool)),
                        Effect::Pure,
                    ),
                    Instruction::new(
                        InstructionKind::Constant(Constant::Integer(0)),
                        vec![],
                        Some(ValueDef::new(ValueId::new(3), ValueType::I64)),
                        Effect::Pure,
                    ),
                ],
                Terminator::Jump {
                    target: BlockId::new(1),
                    arguments: vec![ValueId::new(2), ValueId::new(3)],
                },
            ),
            Block::new(
                BlockId::new(1),
                vec![
                    ValueDef::new(ValueId::new(4), ValueType::Bool),
                    ValueDef::new(ValueId::new(5), ValueType::I64),
                ],
                vec![],
                Terminator::Branch {
                    condition: ValueId::new(4),
                    yes: BlockId::new(2),
                    no: BlockId::new(3),
                },
            ),
            Block::new(
                BlockId::new(2),
                vec![],
                latch,
                Terminator::Jump {
                    target: BlockId::new(1),
                    arguments: vec![ValueId::new(7), ValueId::new(6)],
                },
            ),
            Block::new(
                BlockId::new(3),
                vec![],
                vec![],
                Terminator::Return {
                    values: vec![ValueId::new(5)],
                },
            ),
        ],
        vec![],
        vec![],
        dependencies(7),
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("counted loop")
}

#[test]
fn licm_hoists_effect_free_latches() {
    let original = counted_loop(false);
    let before = replay(
        &original,
        &[ReplayValue::Integer(20), ReplayValue::Integer(22)],
        &mut ReplayHeap::default(),
    );
    let optimized = OptimizerPipeline.run(&original, 6).expect("licm");
    let after = replay(
        optimized.verified(),
        &[ReplayValue::Integer(20), ReplayValue::Integer(22)],
        &mut ReplayHeap::default(),
    );
    assert!(optimized.reports()[5].changed);
    assert_eq!(
        before,
        ReplayOutcome::Return(vec![ReplayValue::Integer(42)])
    );
    assert_eq!(after, before);
    let native = NativeCompiler::new()
        .compile_tier1(optimized.verified())
        .expect("native loop");
    assert_eq!(
        native
            .execute(&[NativeValue::Integer(20), NativeValue::Integer(22)])
            .expect("execute loop")
            .values,
        vec![NativeValue::Integer(42)]
    );

    let blocked = OptimizerPipeline
        .run(&counted_loop(true), 6)
        .expect("blocked licm");
    assert!(!blocked.reports()[5].changed);
}

#[test]
#[cfg(feature = "inkwell")]
fn licm_selected_multiblock_snapshot_exact_tier_parity() {
    let original = counted_loop(false);
    let original_id = original.id();
    let mut compiler = NativeCompiler::new();
    let tier1 = compiler.compile_tier1(&original).expect("loop tier1");
    let tier1_outcome = tier1
        .execute(&[NativeValue::Integer(20), NativeValue::Integer(22)])
        .expect("loop tier1 execute");
    compiler
        .observe_tier1(&tier1_outcome)
        .expect("observed loop tier1");
    let tier2 = compiler.compile_tier2(&original).expect("loop tier2");
    let tier2_outcome = tier2
        .execute(&[NativeValue::Integer(20), NativeValue::Integer(22)])
        .expect("loop tier2 execute");
    assert_ne!(tier1.snapshot_id(), original_id);
    assert_eq!(tier2.snapshot_id(), tier1.snapshot_id());
    assert_eq!(original.id(), original_id);
    assert_eq!(tier1_outcome.values, tier2_outcome.values);
    assert_eq!(tier2_outcome.values, vec![NativeValue::Integer(42)]);
}

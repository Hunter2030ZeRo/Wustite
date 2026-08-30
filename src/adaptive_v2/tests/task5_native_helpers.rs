use super::super::heap::GcConfig;
use super::super::integration::SharedTier1Code;
use super::super::native::{AdaptiveNativeContext, NativeCompiler, NativeRuntime, NativeValue};
use super::super::trace::EntryKind;
use super::super::wxir_v2::VerifiedSnapshot;
use super::super::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
};
use super::super::wxir_v2::dependency::{Dependency, DependencyKind};
use super::super::wxir_v2::ir::{
    Block, BlockId, Effect, Instruction, InstructionKind, RootLocation, RootMap, SafepointId,
    SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
};
use super::task4_support::{compile_permit, dependencies, identity};

fn object_list_call_snapshot() -> VerifiedSnapshot {
    let point = SafepointId::new(1);
    let mut deps = dependencies(7);
    deps.extend([
        Dependency::current(DependencyKind::Shape, 20, 1),
        Dependency::current(DependencyKind::Class, 21, 1),
        Dependency::current(DependencyKind::ListLayout, 22, 1),
        Dependency::current(DependencyKind::Callee, 1, 1),
    ]);
    let parameters = [
        ValueType::Handle,
        ValueType::Handle,
        ValueType::I64,
        ValueType::I64,
        ValueType::I64,
    ]
    .into_iter()
    .enumerate()
    .map(|(id, ty)| ValueDef::new(ValueId::new(u32::try_from(id).expect("fixture id")), ty))
    .collect::<Vec<_>>();
    let instructions = vec![
        Instruction::new(
            InstructionKind::ObjectSet,
            vec![ValueId::new(0), ValueId::new(2), ValueId::new(4)],
            None,
            Effect::Write,
        )
        .ordered(0),
        Instruction::new(
            InstructionKind::ListAppend,
            vec![ValueId::new(1), ValueId::new(4)],
            None,
            Effect::Write,
        )
        .ordered(1),
        Instruction::new(
            InstructionKind::ObjectGet,
            vec![ValueId::new(0), ValueId::new(2)],
            Some(ValueDef::new(ValueId::new(6), ValueType::I64)),
            Effect::Read,
        ),
        Instruction::new(
            InstructionKind::ListGet,
            vec![ValueId::new(1), ValueId::new(3)],
            Some(ValueDef::new(ValueId::new(7), ValueType::I64)),
            Effect::Read,
        ),
        Instruction::safepoint(
            InstructionKind::Call { callee: 1 },
            vec![ValueId::new(6), ValueId::new(7)],
            Some(ValueDef::new(ValueId::new(8), ValueType::I64)),
            Effect::Call,
            point,
        )
        .ordered(2),
    ];
    let frame = FrameRecipe::new(
        9,
        12,
        parameters
            .iter()
            .enumerate()
            .map(|(register, value)| {
                RegisterRecipe::new(
                    u16::try_from(register).expect("fixture register"),
                    RegisterSource::Ssa(value.id),
                    value.ty,
                )
            })
            .collect(),
    );
    let recipe = DeoptRecipe::new(
        1,
        identity(),
        12,
        ResumeMode::ReplayBeforePc,
        vec![frame],
        point,
    )
    .with_dependencies(deps.clone());
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            parameters,
            instructions,
            Terminator::Return {
                values: vec![ValueId::new(8)],
            },
        )],
        vec![RootMap::new(
            point,
            [
                RootLocation::Ssa(ValueId::new(0)),
                RootLocation::Ssa(ValueId::new(1)),
            ]
            .into_iter()
            .collect(),
        )],
        vec![recipe],
        deps,
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("verified helper snapshot")
}

fn add(left: i64, right: i64) -> i64 {
    left + right
}

#[cfg(feature = "inkwell")]
fn multiblock_object_list_call_snapshot() -> VerifiedSnapshot {
    let source = object_list_call_snapshot();
    let mut body = source.body().clone();
    let tail = body.blocks[0].instructions.split_off(2);
    body.blocks[0].terminator = Terminator::Jump {
        target: BlockId::new(1),
        arguments: vec![],
    };
    body.blocks.push(Block::new(
        BlockId::new(1),
        vec![],
        tail,
        Terminator::Return {
            values: vec![ValueId::new(8)],
        },
    ));
    source
        .derive_optimized(body)
        .expect("verified multi-block helper snapshot")
}

#[test]
fn cranelift_executes_object_list_direct_call_op_helpers() {
    // Given: a verified object/list/call trace and private helper runtime.
    let snapshot = object_list_call_snapshot();
    let mut runtime = NativeRuntime::default();
    runtime.insert_object(1, []);
    runtime.insert_list(2, vec![10]);
    runtime.insert_call(1, add);
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compile");

    // When: the trace mutates, reads, and calls through operation-specific helpers.
    let outcome = code
        .execute_with_heap(
            &[
                NativeValue::Handle(1),
                NativeValue::Handle(2),
                NativeValue::Integer(7),
                NativeValue::Integer(0),
                NativeValue::Integer(5),
            ],
            &mut runtime,
        )
        .expect("native helper execution");

    // Then: actual machine code returns the model result with no generic dispatch.
    assert_eq!(outcome.values, vec![NativeValue::Integer(15)]);
    assert_eq!(outcome.counters.machine_entries, 1);
    assert_eq!(outcome.counters.helper_calls, 5);
    assert_eq!(outcome.counters.generic_dispatch_calls, 0);
}

#[test]
fn cranelift_helpers_mutate_shared_adaptive_heap_adapter() {
    // Given: one verified trace and object/list/call payloads in the production heap adapter.
    let snapshot = object_list_call_snapshot();
    let mut runtime = AdaptiveNativeContext::new(GcConfig {
        collect_every_allocation: true,
        ..GcConfig::default()
    });
    let object = runtime.allocate_object().expect("object");
    let list = runtime.allocate_list().expect("list");
    runtime.append_integer(list, 10).expect("seed list");
    let callable = runtime.register_binary_callable(add).expect("callable");
    runtime.bind_callable(1, callable).expect("bind callable");
    runtime.bind_field(7, "answer");
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compile");

    // When: generated machine code executes every operation-specific helper.
    let outcome = code
        .execute_with_adaptive_heap(
            &[
                object,
                list,
                NativeValue::Integer(7),
                NativeValue::Integer(0),
                NativeValue::Integer(5),
            ],
            &mut runtime,
        )
        .expect("adaptive helper execution");
    runtime.collect_minor().expect("collect after helper calls");

    // Then: the shared adapter produces the same result without generic dispatch.
    assert_eq!(callable, NativeValue::Handle(3));
    assert_eq!(outcome.values, vec![NativeValue::Integer(15)]);
    assert_eq!(outcome.counters.machine_entries, 1);
    assert_eq!(outcome.counters.helper_calls, 5);
    assert_eq!(outcome.counters.generic_dispatch_calls, 0);
}

#[test]
fn shared_cranelift_helpers_overlap_and_keep_gc_roots() {
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::time::Duration;

    // Given: one compiled trace shared by two independent mutators and helper contexts.
    let snapshot = object_list_call_snapshot();
    let code = Arc::new(
        SharedTier1Code::new(
            NativeCompiler::new()
                .compile_tier1(&snapshot)
                .expect("compile once"),
        )
        .expect("Cranelift shared code"),
    );
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let workers = (0..2)
        .map(|worker| {
            let code = Arc::clone(&code);
            let gate = Arc::clone(&gate);
            let entered_tx = entered_tx.clone();
            std::thread::spawn(move || {
                let mut runtime = AdaptiveNativeContext::new(GcConfig {
                    collect_every_allocation: true,
                    ..GcConfig::default()
                });
                let object = runtime.allocate_object().expect("object");
                let list = runtime.allocate_list().expect("list");
                runtime.append_integer(list, 10).expect("seed list");
                let callable = runtime
                    .register_binary_callable(move |left, right| {
                        entered_tx.send(worker).expect("announce helper entry");
                        let (open, changed) = &*gate;
                        let mut open = open.lock().expect("gate lock");
                        while !*open {
                            open = changed.wait(open).expect("gate wait");
                        }
                        left + right
                    })
                    .expect("callable");
                runtime.bind_callable(1, callable).expect("bind callable");
                runtime.bind_field(7, "answer");
                let outcome = code
                    .execute_with_adaptive_heap(
                        &[
                            object,
                            list,
                            NativeValue::Integer(7),
                            NativeValue::Integer(0),
                            NativeValue::Integer(5),
                        ],
                        &mut runtime,
                    )
                    .expect("concurrent native helper execution");
                runtime.collect_minor().expect("minor collection");
                assert_eq!(runtime.get_integer_field(object, 7, "answer"), Ok(5));
                assert_eq!(runtime.integer_at(list, 1), Ok(5));
                outcome
            })
        })
        .collect::<Vec<_>>();
    drop(entered_tx);

    // When: both native calls reach the user-call helper before either is allowed to return.
    let first = entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("first mutator entered native helper");
    let second = entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("second mutator overlapped native helper");
    assert_ne!(first, second);
    let (open, changed) = &*gate;
    *open.lock().expect("gate lock") = true;
    changed.notify_all();

    // Then: both executions complete through machine code without generic dispatch or lost roots.
    for worker in workers {
        let outcome = worker.join().expect("mutator join");
        assert_eq!(outcome.values, vec![NativeValue::Integer(15)]);
        assert_eq!(outcome.counters.machine_entries, 1);
        assert_eq!(outcome.counters.helper_calls, 5);
        assert_eq!(outcome.counters.generic_dispatch_calls, 0);
    }
}

#[test]
fn helper_error_blocks_native_success() {
    // Given: compiled helper code and a runtime missing the object handle.
    let snapshot = object_list_call_snapshot();
    let mut runtime = NativeRuntime::default();
    runtime.insert_list(2, vec![10]);
    runtime.insert_call(1, add);
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compile");

    // When: the object helper rejects the stale handle.
    let result = code.execute_with_heap(
        &[
            NativeValue::Handle(99),
            NativeValue::Handle(2),
            NativeValue::Integer(7),
            NativeValue::Integer(0),
            NativeValue::Integer(5),
        ],
        &mut runtime,
    );

    // Then: the native boundary returns a typed helper failure.
    assert!(result.is_err());
}

#[test]
#[cfg(feature = "inkwell")]
fn llvm_cranelift_share_object_list_call_helper_abi_snapshot() {
    // Given: one verified helper snapshot and equivalent fresh heaps for both tiers.
    let snapshot = object_list_call_snapshot();
    let mut compiler = NativeCompiler::new();
    let mut tier1_runtime = NativeRuntime::default();
    tier1_runtime.insert_object(1, []);
    tier1_runtime.insert_list(2, vec![10]);
    tier1_runtime.insert_call(1, add);
    let arguments = [
        NativeValue::Handle(1),
        NativeValue::Handle(2),
        NativeValue::Integer(7),
        NativeValue::Integer(0),
        NativeValue::Integer(5),
    ];

    // When: Tier 1 executes, then LLVM O3 executes the exact same snapshot on a fresh heap.
    let tier1 = compiler.compile_tier1(&snapshot).expect("Tier 1 compile");
    let tier1_outcome = tier1
        .execute_with_heap(&arguments, &mut tier1_runtime)
        .expect("Tier 1");
    compiler
        .observe_tier1(&tier1_outcome)
        .expect("Tier 1 receipt");
    let mut tier2_runtime = NativeRuntime::default();
    tier2_runtime.insert_object(1, []);
    tier2_runtime.insert_list(2, vec![10]);
    tier2_runtime.insert_call(1, add);
    let tier2 = compiler.compile_tier2(&snapshot).expect("LLVM O3 compile");
    let tier2_outcome = tier2
        .execute_with_heap(&arguments, &mut tier2_runtime)
        .expect("LLVM O3");

    // Then: IDs, result, helper ABI counters, and dispatch count are identical.
    assert_eq!(tier1.snapshot_id(), tier2.snapshot_id());
    assert_eq!(tier1_outcome.values, tier2_outcome.values);
    assert_eq!(tier2_outcome.counters.helper_calls, 5);
    assert_eq!(tier2_outcome.counters.generic_dispatch_calls, 0);
}

#[test]
#[cfg(feature = "inkwell")]
fn llvm_cfg_matches_cranelift_multiblock_object_list_call() {
    let snapshot = multiblock_object_list_call_snapshot();
    let arguments = [
        NativeValue::Handle(1),
        NativeValue::Handle(2),
        NativeValue::Integer(7),
        NativeValue::Integer(0),
        NativeValue::Integer(5),
    ];
    let mut compiler = NativeCompiler::new();
    let mut tier1_runtime = NativeRuntime::default();
    tier1_runtime.insert_object(1, []);
    tier1_runtime.insert_list(2, vec![10]);
    tier1_runtime.insert_call(1, add);
    let tier1 = compiler
        .compile_tier1(&snapshot)
        .expect("multi-block tier1");
    let tier1_outcome = tier1
        .execute_with_heap(&arguments, &mut tier1_runtime)
        .expect("tier1 execute");
    compiler
        .observe_tier1(&tier1_outcome)
        .expect("observed tier1");
    let mut tier2_runtime = NativeRuntime::default();
    tier2_runtime.insert_object(1, []);
    tier2_runtime.insert_list(2, vec![10]);
    tier2_runtime.insert_call(1, add);
    let tier2 = compiler
        .compile_tier2(&snapshot)
        .expect("multi-block tier2");
    let tier2_outcome = tier2
        .execute_with_heap(&arguments, &mut tier2_runtime)
        .expect("tier2 execute");
    assert_eq!(tier1.snapshot_id(), tier2.snapshot_id());
    assert_eq!(tier1_outcome.values, tier2_outcome.values);
    assert_eq!(tier2_outcome.values, vec![NativeValue::Integer(15)]);
    assert_eq!(tier2_outcome.counters.helper_calls, 5);
    assert_eq!(tier2_outcome.counters.generic_dispatch_calls, 0);
}

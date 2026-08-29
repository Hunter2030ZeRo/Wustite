use super::super::native::{NativeCompiler, NativeValue};
use super::super::trace::EntryKind;
use super::super::wxir_v2::VerifiedSnapshot;
use super::super::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
};
use super::super::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, RootLocation, RootMap,
    SafepointId, SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
};
use super::task4_support::{compile_permit, dependencies, identity};

fn constant_snapshot(constant: Constant, ty: ValueType) -> VerifiedSnapshot {
    let draft = SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![],
            vec![Instruction::new(
                InstructionKind::Constant(constant),
                vec![],
                Some(ValueDef::new(ValueId::new(0), ty)),
                Effect::Pure,
            )],
            Terminator::Return {
                values: vec![ValueId::new(0)],
            },
        )],
        vec![],
        vec![],
        dependencies(7),
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("constant snapshot")
}

fn side_exit_snapshot() -> VerifiedSnapshot {
    let point = SafepointId::new(7);
    let deps = dependencies(7);
    let recipe = DeoptRecipe::new(
        7,
        identity(),
        18,
        ResumeMode::ResumeAfterPc,
        vec![FrameRecipe::new(
            9,
            18,
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
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![ValueDef::new(ValueId::new(0), ValueType::I64)],
            vec![],
            Terminator::SideExit {
                id: 7,
                values: vec![ValueId::new(0)],
            },
        )],
        vec![RootMap::new(point, Default::default())],
        vec![recipe],
        deps,
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("side-exit snapshot")
}

fn handle_side_exit_snapshot() -> VerifiedSnapshot {
    let point = SafepointId::new(8);
    let deps = dependencies(7);
    let recipe = DeoptRecipe::new(
        8,
        identity(),
        19,
        ResumeMode::ResumeAfterPc,
        vec![FrameRecipe::new(
            9,
            19,
            vec![RegisterRecipe::new(
                0,
                RegisterSource::Ssa(ValueId::new(0)),
                ValueType::Handle,
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
            vec![ValueDef::new(ValueId::new(0), ValueType::Handle)],
            vec![],
            Terminator::SideExit {
                id: 8,
                values: vec![ValueId::new(0)],
            },
        )],
        vec![RootMap::new(
            point,
            [RootLocation::Ssa(ValueId::new(0))].into_iter().collect(),
        )],
        vec![recipe],
        deps,
    )
    .with_schema_epoch(7);
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("Handle side-exit snapshot")
}

#[test]
fn native_slots_round_trip_boolean_and_handle_tags() {
    // Given: verified Boolean and Handle constant snapshots.
    let boolean = constant_snapshot(Constant::Boolean(true), ValueType::Bool);
    let handle = constant_snapshot(Constant::HandleBits(41), ValueType::Handle);

    // When: both snapshots execute through actual Cranelift machine code.
    let mut compiler = NativeCompiler::new();
    let boolean = compiler
        .compile_tier1(&boolean)
        .expect("Boolean compile")
        .execute(&[]);
    let handle = compiler
        .compile_tier1(&handle)
        .expect("Handle compile")
        .execute(&[]);

    // Then: output tags decode to their exact semantic variants.
    assert_eq!(
        boolean.expect("Boolean execute").values,
        vec![NativeValue::Boolean(true)]
    );
    assert_eq!(
        handle.expect("Handle execute").values,
        vec![NativeValue::Handle(41)]
    );
}

#[test]
fn native_handle_side_exit_preserves_packed_generation() {
    // Given: a verified Handle side exit and a local stable token with nonzero generation.
    let snapshot = handle_side_exit_snapshot();
    let token = (7_u64 << 32) | 41;
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("Handle compile");

    // When: actual Cranelift code copies the Handle through its side-exit ABI.
    let outcome = code
        .execute(&[NativeValue::Handle(token)])
        .expect("Handle side exit");

    // Then: the complete packed local identity, including generation, is retained.
    assert_eq!(outcome.values, vec![NativeValue::Handle(token)]);
    assert_eq!(outcome.exit_id, 8);
}

#[test]
fn float_bits_round_trip_through_native_codegen() {
    // Given: a verified F64 snapshot inside the native scalar capability matrix.
    let snapshot = constant_snapshot(Constant::FloatBits(1.5_f64.to_bits()), ValueType::F64);

    // When: Tier 1 compiles and executes the snapshot.
    let result = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("F64 compile")
        .execute(&[])
        .expect("F64 execute");

    // Then: the backend preserves the exact IEEE-754 bit pattern.
    assert_eq!(
        result.values,
        vec![NativeValue::FloatBits(1.5_f64.to_bits())]
    );
}

#[test]
fn undefined_borrowed_view_is_rejected_before_native_codegen() {
    // Given: a verified borrowed-view placeholder with no stable native ABI.
    let snapshot = constant_snapshot(Constant::UndefinedDead, ValueType::BorrowedView);

    // When: Tier 1 compilation is requested.
    let result = NativeCompiler::new().compile_tier1(&snapshot);

    // Then: the backend rejects the unsupported ABI before execution.
    assert!(result.is_err());
}

#[test]
fn side_exit_publishes_value_and_complete_resume_identifiers() {
    // Given: a verified ResumeAfter side exit with deopt and root-map metadata.
    let snapshot = side_exit_snapshot();
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compile");

    // When: native code takes side exit 7.
    let outcome = code
        .execute(&[NativeValue::Integer(91)])
        .expect("side exit");

    // Then: typed state and every reconstruction identifier are available before fallback.
    assert_eq!(outcome.values, vec![NativeValue::Integer(91)]);
    assert_eq!(outcome.exit_id, 7);
    assert_eq!(outcome.deopt_id, 7);
    assert_eq!(outcome.safepoint_id, 7);
    assert_eq!(outcome.counters.generic_dispatch_calls, 0);
}

#[test]
#[cfg(feature = "inkwell")]
fn llvm_and_cranelift_publish_identical_side_exit_state() {
    // Given: one verified Handle side-exit snapshot and a nonzero-generation stable token.
    let snapshot = handle_side_exit_snapshot();
    let token = (13_u64 << 32) | 91;
    let mut compiler = NativeCompiler::new();
    let tier1 = compiler.compile_tier1(&snapshot).expect("Tier 1 compile");

    // When: Tier 1 executes and LLVM O3 executes the exact same snapshot.
    let tier1 = tier1
        .execute(&[NativeValue::Handle(token)])
        .expect("Tier 1 exit");
    compiler.observe_tier1(&tier1).expect("Tier 1 receipt");
    let tier2 = compiler.compile_tier2(&snapshot).expect("LLVM compile");
    let tier2 = tier2
        .execute(&[NativeValue::Handle(token)])
        .expect("LLVM exit");

    // Then: both tiers preserve the full token and every exit/deopt/root identifier.
    assert_eq!(tier1.values, vec![NativeValue::Handle(token)]);
    assert_eq!(tier1.values, tier2.values);
    assert_eq!(tier1.exit_id, tier2.exit_id);
    assert_eq!(tier1.deopt_id, tier2.deopt_id);
    assert_eq!(tier1.safepoint_id, tier2.safepoint_id);
}

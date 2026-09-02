use std::collections::{BTreeMap, BTreeSet};

use super::super::profile::{AdaptiveProfile, FactClass, LiveObservation, ProfileCase};
use super::super::trace::{
    EntryKind, ExecutableIdentity, RecordedTrace, TraceError, TraceEvent, TraceOp, TraceRecorder,
    TraceStart,
};
use super::super::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
};
use super::super::wxir_v2::dependency::{Dependency, DependencyKind};
use super::super::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, RootLocation, RootMap,
    SafepointId, SnapshotDraft, Terminator, ValueDef, ValueId, ValueType, WxIrAbi,
};
use super::super::wxir_v2::{SnapshotError, VerifiedSnapshot};

fn observe(profile: &mut AdaptiveProfile, count: usize) {
    for _ in 0..count {
        profile.observe_live(LiveObservation::new(
            ProfileCase::new(1),
            FactClass::UnknownClassified,
        ));
    }
}

fn recording_profile() -> AdaptiveProfile {
    let mut profile = AdaptiveProfile::new(7);
    observe(&mut profile, 32);
    profile
}

fn entry_trace(profile: &mut AdaptiveProfile) -> RecordedTrace {
    let permit = profile
        .take_record_permit()
        .expect("32 live observations create a record permit");
    let mut recorder = TraceRecorder::try_start(
        permit,
        TraceStart {
            executable: ExecutableIdentity::new(9, 3),
            entry: EntryKind::FunctionEntry,
            start_pc: 0,
        },
    )
    .expect("valid function entry");
    recorder
        .record(TraceEvent::new(
            0,
            TraceOp::Constant(Constant::Integer(1)),
            &[],
            Some((0, ValueType::I64)),
            Effect::Pure,
        ))
        .expect("constant records");
    recorder
        .finish(Terminator::Return {
            values: vec![ValueId::new(0)],
        })
        .expect("entry trace finishes")
}

fn dependencies(epoch: u64) -> Vec<Dependency> {
    vec![
        Dependency::current(DependencyKind::Executable, 9, 3),
        Dependency::current(DependencyKind::Schema, 7, epoch),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
    ]
}

fn recipe(point: SafepointId) -> DeoptRecipe {
    DeoptRecipe::new(
        0,
        ExecutableIdentity::new(9, 3),
        4,
        ResumeMode::ReplayBeforePc,
        vec![FrameRecipe::new(
            9,
            4,
            vec![RegisterRecipe::new(
                0,
                RegisterSource::Ssa(ValueId::new(0)),
                ValueType::Handle,
            )],
        )],
        point,
    )
}

#[test]
fn recorder_snapshot_require_both_live_profile_windows() {
    let mut profile = AdaptiveProfile::new(7);
    profile.seed_static_hint(ProfileCase::new(1), 1_000_000);
    observe(&mut profile, 31);
    assert!(profile.take_record_permit().is_none());
    observe(&mut profile, 1);
    let trace = entry_trace(&mut profile);
    assert!(profile.finish_recording());
    observe(&mut profile, 31);
    assert!(profile.take_compile_permit().is_none());
    observe(&mut profile, 1);
    let permit = profile
        .take_compile_permit()
        .expect("post-record live window creates compile permit");
    let snapshot = VerifiedSnapshot::seal(
        trace.into_draft(dependencies(permit.schema_epoch())),
        permit,
    )
    .expect("verified immutable snapshot");
    assert_eq!(snapshot.abi(), WxIrAbi::V2);
}

#[test]
fn recorder_enforces_osr_backedge_nested_loop_trace_limit_rules() {
    let mut profile = recording_profile();
    let permit = profile.take_record_permit().expect("record permit");
    assert!(matches!(
        TraceRecorder::try_start(
            permit,
            TraceStart {
                executable: ExecutableIdentity::new(9, 3),
                entry: EntryKind::LoopHeader {
                    header_pc: 10,
                    backedge_pc: 20,
                    preheader: None,
                },
                start_pc: 11,
            },
        ),
        Err(TraceError::ArbitraryPc { pc: 11 })
    ));

    let mut profile = recording_profile();
    let permit = profile.take_record_permit().expect("record permit");
    let mut recorder = TraceRecorder::with_limit(
        permit,
        TraceStart {
            executable: ExecutableIdentity::new(9, 3),
            entry: EntryKind::FunctionEntry,
            start_pc: 0,
        },
        1,
    )
    .expect("valid entry");
    recorder
        .record(TraceEvent::effect_only(
            0,
            TraceOp::NestedLoopHeader { header_pc: 12 },
        ))
        .expect("nested loop becomes a side exit");
    assert_eq!(
        recorder.record(TraceEvent::effect_only(1, TraceOp::Helper { helper: 1 })),
        Err(TraceError::Terminated)
    );
}

#[test]
fn verifier_rejects_bad_defs_roots_deps_and_borrows() {
    let point = SafepointId::new(1);
    let block = Block::new(
        BlockId::new(0),
        vec![ValueDef::new(ValueId::new(0), ValueType::Handle)],
        vec![
            Instruction::new(
                InstructionKind::BorrowView,
                vec![ValueId::new(0)],
                Some(ValueDef::new(ValueId::new(1), ValueType::BorrowedView)),
                Effect::Pure,
            ),
            Instruction::safepoint(
                InstructionKind::Helper { helper: 1 },
                vec![ValueId::new(1)],
                None,
                Effect::Helper,
                point,
            ),
        ],
        Terminator::Return { values: vec![] },
    );
    let mut roots = BTreeSet::new();
    roots.insert(RootLocation::Ssa(ValueId::new(0)));
    let draft = SnapshotDraft::new(
        ExecutableIdentity::new(9, 3),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![block],
        vec![RootMap::new(point, roots)],
        vec![recipe(point)],
        vec![Dependency::observed(DependencyKind::Executable, 9, 3, 4)],
    );
    assert!(matches!(
        draft.verify(),
        Err(SnapshotError::StaleDependency { .. })
            | Err(SnapshotError::BorrowAcrossSafepoint { .. })
    ));
}

#[test]
fn snapshot_ids_deterministic_cover_roots_deopt_deps() {
    let mut profile = recording_profile();
    let first = entry_trace(&mut profile);
    assert!(profile.finish_recording());
    observe(&mut profile, 32);
    let permit = profile.take_compile_permit().expect("compile permit");
    let a = VerifiedSnapshot::seal(first.clone().into_draft(dependencies(7)), permit)
        .expect("first snapshot");

    let mut other_profile = recording_profile();
    let second = entry_trace(&mut other_profile);
    assert!(other_profile.finish_recording());
    observe(&mut other_profile, 32);
    let permit = other_profile.take_compile_permit().expect("compile permit");
    let b = VerifiedSnapshot::seal(second.into_draft(dependencies(7)), permit)
        .expect("identical snapshot");
    assert_eq!(a.id(), b.id());
}

#[test]
fn root_map_uses_semantic_order() {
    let mut first = BTreeMap::new();
    first.insert(2_u32, RootLocation::Ssa(ValueId::new(2)));
    first.insert(1_u32, RootLocation::Ssa(ValueId::new(1)));
    assert_eq!(
        first.values().copied().collect::<Vec<_>>(),
        vec![
            RootLocation::Ssa(ValueId::new(1)),
            RootLocation::Ssa(ValueId::new(2)),
        ]
    );
}

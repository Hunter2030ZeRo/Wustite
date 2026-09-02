use super::super::profile::{
    AdaptiveProfile, CompilePermit, FactClass, LiveObservation, ProfileCase,
};
use super::super::trace::{EntryKind, ExecutableIdentity};
use super::super::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
};
use super::super::wxir_v2::dependency::{Dependency, DependencyKind};
use super::super::wxir_v2::ir::{
    Block, BlockId, Effect, Instruction, InstructionKind, RootLocation, RootMap, SafepointId,
    SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
};

pub(super) const fn identity() -> ExecutableIdentity {
    ExecutableIdentity::new(9, 3)
}

pub(super) fn dependencies(schema_epoch: u64) -> Vec<Dependency> {
    vec![
        Dependency::current(DependencyKind::Executable, 9, 3),
        Dependency::current(DependencyKind::Schema, 7, schema_epoch),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
    ]
}

pub(super) fn compile_permit(schema_epoch: u64) -> CompilePermit {
    let mut profile = AdaptiveProfile::new(schema_epoch, 10);
    for _ in 0..64 {
        observe(&mut profile);
    }
    let _record = profile.take_record_permit().expect("record permit");
    assert!(profile.finish_recording());
    for _ in 0..32 {
        observe(&mut profile);
    }
    profile.take_compile_permit().expect("compile permit")
}

fn observe(profile: &mut AdaptiveProfile) {
    profile.observe_live(LiveObservation::new(
        ProfileCase::new(1),
        FactClass::UnknownClassified,
    ));
}

pub(super) fn rooted_helper_draft() -> SnapshotDraft {
    let point = SafepointId::new(1);
    let deps = dependencies(7);
    let instruction = Instruction::safepoint(
        InstructionKind::Helper { helper: 1 },
        vec![ValueId::new(0)],
        None,
        Effect::Helper,
        point,
    )
    .ordered(0);
    let recipe = DeoptRecipe::new(
        1,
        identity(),
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
    .with_dependencies(deps.clone());
    SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![ValueDef::new(ValueId::new(0), ValueType::Handle)],
            vec![instruction],
            Terminator::Return { values: vec![] },
        )],
        vec![RootMap::new(
            point,
            [RootLocation::Ssa(ValueId::new(0))].into_iter().collect(),
        )],
        vec![recipe],
        deps,
    )
    .with_schema_epoch(7)
}

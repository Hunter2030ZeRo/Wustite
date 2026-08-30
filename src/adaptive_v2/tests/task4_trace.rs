use super::super::profile::{AdaptiveProfile, FactClass, LiveObservation, ProfileCase};
use super::super::trace::{EntryKind, TraceError, TraceEvent, TraceOp, TraceRecorder, TraceStart};
use super::super::wxir_v2::ir::{
    Constant, Effect, FactLowering, SafepointId, Terminator, ValueId, ValueType,
};
use super::task4_support::identity;

fn permit() -> super::super::profile::RecordPermit {
    let mut profile = AdaptiveProfile::new(7);
    for _ in 0..64 {
        profile.observe_live(LiveObservation::new(
            ProfileCase::new(1),
            FactClass::UnknownClassified,
        ));
    }
    profile.take_record_permit().expect("record permit")
}

#[test]
fn loop_trace_validates_backedges_and_reducibility() {
    let start = TraceStart {
        executable: identity(),
        entry: EntryKind::LoopHeader {
            header_pc: 10,
            backedge_pc: 20,
            preheader: None,
        },
        start_pc: 10,
    };
    let recorder = TraceRecorder::try_start(permit(), start).expect("loop header");
    assert!(
        recorder
            .finish(Terminator::Backedge {
                target_pc: 10,
                safepoint: super::super::wxir_v2::ir::SafepointId::new(1),
            })
            .is_ok()
    );

    let recorder = TraceRecorder::try_start(permit(), start).expect("loop header");
    assert_eq!(
        recorder.finish(Terminator::Backedge {
            target_pc: 11,
            safepoint: super::super::wxir_v2::ir::SafepointId::new(1),
        }),
        Err(TraceError::MismatchedBackedge {
            expected: 10,
            actual: 11
        })
    );

    let recorder = TraceRecorder::try_start(permit(), start).expect("loop header");
    assert_eq!(
        recorder.finish(Terminator::IrreducibleBackedge),
        Err(TraceError::IrreducibleBackedge)
    );
}

#[test]
fn trace_limit_undefined_reg_missing_safepoint_typed_failures() {
    let start = TraceStart {
        executable: identity(),
        entry: EntryKind::FunctionEntry,
        start_pc: 0,
    };
    let mut recorder = TraceRecorder::with_limit(permit(), start, 1).expect("entry");
    recorder
        .record(TraceEvent::new(
            0,
            TraceOp::Constant(Constant::Integer(1)),
            &[],
            Some((0, ValueType::I64)),
            Effect::Pure,
        ))
        .expect("first event");
    assert_eq!(
        recorder.record(TraceEvent::effect_only(1, TraceOp::IntegerAdd)),
        Err(TraceError::TraceLimit { limit: 1 })
    );

    let mut recorder = TraceRecorder::try_start(permit(), start).expect("entry");
    assert_eq!(
        recorder.record(TraceEvent::new(
            0,
            TraceOp::IntegerAdd,
            &[9, 9],
            Some((0, ValueType::I64)),
            Effect::Pure
        )),
        Err(TraceError::UndefinedRegister { register: 9 })
    );

    let mut recorder = TraceRecorder::try_start(permit(), start).expect("entry");
    assert_eq!(
        recorder.record(TraceEvent::new(
            0,
            TraceOp::Call { callee: 9 },
            &[],
            None,
            Effect::Call
        )),
        Err(TraceError::MissingSafepoint { pc: 0 })
    );
}

#[test]
fn trace_ssa_renames_regs_models_calls_branches_loop_control() {
    let start = TraceStart {
        executable: identity(),
        entry: EntryKind::FunctionEntry,
        start_pc: 0,
    };
    let mut recorder = TraceRecorder::try_start(permit(), start).expect("entry");
    recorder
        .record(TraceEvent::new(
            0,
            TraceOp::Constant(Constant::Integer(1)),
            &[],
            Some((0, ValueType::I64)),
            Effect::Pure,
        ))
        .expect("first write");
    recorder
        .record(TraceEvent::new(
            1,
            TraceOp::Constant(Constant::Integer(2)),
            &[],
            Some((0, ValueType::I64)),
            Effect::Pure,
        ))
        .expect("register rewrite gets new SSA id");
    recorder
        .record(
            TraceEvent::new(
                2,
                TraceOp::Call { callee: 9 },
                &[0],
                Some((1, ValueType::I64)),
                Effect::Call,
            )
            .at_safepoint(SafepointId::new(1)),
        )
        .expect("recursive call");
    recorder
        .record(TraceEvent::new(
            3,
            TraceOp::Branch {
                taken: true,
                side_exit: 7,
            },
            &[1],
            None,
            Effect::Pure,
        ))
        .expect("conditional hot branch");
    let trace = recorder
        .finish(Terminator::Return {
            values: vec![ValueId::new(2)],
        })
        .expect("return path");
    let draft = trace.into_draft(super::task4_support::dependencies(7));
    assert_eq!(
        draft.body.blocks[0].instructions[0]
            .output
            .expect("first output")
            .id,
        ValueId::new(0)
    );
    assert_eq!(
        draft.body.blocks[0].instructions[1]
            .output
            .expect("second output")
            .id,
        ValueId::new(1)
    );
    assert_eq!(
        draft.body.blocks[0].instructions[2]
            .output
            .expect("call output")
            .id,
        ValueId::new(2)
    );
}

#[test]
fn nested_loops_split_structure_facts_never_replace_live_recording() {
    let start = TraceStart {
        executable: identity(),
        entry: EntryKind::FunctionEntry,
        start_pc: 0,
    };
    let mut recorder = TraceRecorder::try_start(permit(), start).expect("entry");
    recorder
        .record(TraceEvent::effect_only(
            0,
            TraceOp::Fact {
                lowering: FactLowering::ElidedProven,
            },
        ))
        .expect("proven probe elides");
    recorder
        .record(TraceEvent::effect_only(
            1,
            TraceOp::Fact {
                lowering: FactLowering::GuardedStatic { guard: 4 },
            },
        ))
        .expect("static fact emits guard");
    recorder
        .record(TraceEvent::effect_only(
            2,
            TraceOp::Fact {
                lowering: FactLowering::LiveProbe,
            },
        ))
        .expect("unknown fact probes");
    recorder
        .record(TraceEvent::effect_only(
            3,
            TraceOp::NestedLoopHeader { header_pc: 20 },
        ))
        .expect("nested header side exits");
    assert_eq!(
        recorder.record(TraceEvent::effect_only(4, TraceOp::Helper { helper: 1 })),
        Err(TraceError::Terminated)
    );
    let trace = recorder
        .finish(Terminator::SideExit {
            id: 20,
            values: vec![],
        })
        .expect("outer trace seals separately");
    assert_eq!(
        trace
            .into_draft(super::task4_support::dependencies(7))
            .body
            .blocks[0]
            .instructions
            .len(),
        3
    );
}

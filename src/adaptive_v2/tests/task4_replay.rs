use std::collections::BTreeMap;

use super::super::trace::EntryKind;
use super::super::wxir_v2::VerifiedSnapshot;
use super::super::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
};
use super::super::wxir_v2::dependency::{Dependency, DependencyKind};
use super::super::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, RootLocation, RootMap,
    SafepointId, SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
};
use super::super::wxir_v2::replay::{ReplayHeap, ReplayOutcome, ReplayValue, replay};
use super::task4_support::{compile_permit, dependencies, identity};

fn differential_snapshot() -> VerifiedSnapshot {
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
        ValueType::Bool,
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
        vec![
            Block::new(
                BlockId::new(0),
                parameters,
                instructions,
                Terminator::Branch {
                    condition: ValueId::new(5),
                    yes: BlockId::new(1),
                    no: BlockId::new(2),
                },
            ),
            Block::new(
                BlockId::new(1),
                vec![],
                vec![],
                Terminator::Return {
                    values: vec![ValueId::new(8)],
                },
            ),
            Block::new(
                BlockId::new(2),
                vec![],
                vec![Instruction::new(
                    InstructionKind::Constant(Constant::Integer(-1)),
                    vec![],
                    Some(ValueDef::new(ValueId::new(9), ValueType::I64)),
                    Effect::Pure,
                )],
                Terminator::Return {
                    values: vec![ValueId::new(9)],
                },
            ),
        ],
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
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("verified replay snapshot")
}

fn reference(arguments: &[ReplayValue], heap: &mut ReplayHeap) -> ReplayOutcome {
    let [
        ReplayValue::Handle(object),
        ReplayValue::Handle(list),
        ReplayValue::Integer(key),
        ReplayValue::Integer(index),
        payload @ ReplayValue::Integer(_),
        ReplayValue::Boolean(condition),
    ] = arguments
    else {
        return ReplayOutcome::Invalid;
    };
    heap.objects
        .get_mut(object)
        .expect("object fixture")
        .insert(*key, *payload);
    heap.lists
        .get_mut(list)
        .expect("list fixture")
        .push(*payload);
    let ReplayValue::Integer(left) = heap.objects[object][key] else {
        return ReplayOutcome::Invalid;
    };
    let Ok(index) = usize::try_from(*index) else {
        return ReplayOutcome::Invalid;
    };
    let Some(ReplayValue::Integer(right)) = heap.lists[list].get(index) else {
        return ReplayOutcome::Invalid;
    };
    ReplayOutcome::Return(vec![ReplayValue::Integer(if *condition {
        left + right
    } else {
        -1
    })])
}

#[test]
fn trace_replay_matches_object_call_branch_model() {
    let snapshot = differential_snapshot();
    for condition in [true, false] {
        let arguments = [
            ReplayValue::Handle(1),
            ReplayValue::Handle(2),
            ReplayValue::Integer(4),
            ReplayValue::Integer(0),
            ReplayValue::Integer(7),
            ReplayValue::Boolean(condition),
        ];
        let base = ReplayHeap {
            objects: BTreeMap::from([(1, BTreeMap::new())]),
            lists: BTreeMap::from([(2, vec![ReplayValue::Integer(5)])]),
        };
        let mut actual_heap = base.clone();
        let mut expected_heap = base;
        let actual = replay(&snapshot, &arguments, &mut actual_heap);
        let expected = reference(&arguments, &mut expected_heap);
        assert_eq!(actual, expected);
        assert_eq!(actual_heap, expected_heap);
    }
}

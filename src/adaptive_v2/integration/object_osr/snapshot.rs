use crate::adaptive_v2::profile::RecordPermit;
use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity};
use crate::adaptive_v2::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
};
use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId, Effect, Instruction, InstructionKind, RootMap, SafepointId, SnapshotDraft,
    Terminator, ValueDef, ValueId, ValueType,
};

use super::SiteOperation;

pub(super) fn draft(
    executable: u64,
    pc: u32,
    operation: SiteOperation,
    permit: RecordPermit,
) -> SnapshotDraft {
    if operation == SiteOperation::DirectCall {
        return direct_call_draft(executable, pc, permit);
    }
    let (parameters, instruction, returned) = operation_parts(operation, pc);
    let epoch = executable;
    SnapshotDraft::new(
        ExecutableIdentity::new(executable, epoch),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            parameters,
            vec![instruction],
            Terminator::Return { values: returned },
        )],
        Vec::new(),
        Vec::new(),
        dependencies(executable, epoch, permit.schema_epoch(), operation),
    )
    .with_schema_epoch(permit.schema_epoch())
}

fn operation_parts(
    operation: SiteOperation,
    pc: u32,
) -> (Vec<ValueDef>, Instruction, Vec<ValueId>) {
    let handle = ValueDef::new(ValueId::new(0), ValueType::Handle);
    let index = ValueDef::new(ValueId::new(1), ValueType::I64);
    let result = ValueDef::new(ValueId::new(3), ValueType::I64);
    match operation {
        SiteOperation::ObjectGet => (
            vec![handle, index],
            Instruction::new(
                InstructionKind::ObjectGet.at_pc(pc),
                vec![handle.id, index.id],
                Some(result),
                Effect::Read,
            ),
            vec![result.id],
        ),
        SiteOperation::ListGet => (
            vec![handle, index],
            Instruction::new(
                InstructionKind::ListGet.at_pc(pc),
                vec![handle.id, index.id],
                Some(result),
                Effect::Read,
            ),
            vec![result.id],
        ),
        SiteOperation::DirectCall => unreachable!("direct call has a safepoint-specific draft"),
    }
}

fn dependencies(
    executable: u64,
    epoch: u64,
    schema: u64,
    operation: SiteOperation,
) -> Vec<Dependency> {
    let mut dependencies = vec![
        Dependency::current(DependencyKind::Executable, executable, epoch),
        Dependency::current(DependencyKind::Schema, executable, schema),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
    ];
    match operation {
        SiteOperation::ObjectGet => dependencies.extend([
            Dependency::current(DependencyKind::Shape, executable, 1),
            Dependency::current(DependencyKind::Class, executable, 1),
        ]),
        SiteOperation::ListGet => dependencies.push(Dependency::current(
            DependencyKind::ListLayout,
            executable,
            1,
        )),
        SiteOperation::DirectCall => {
            dependencies.push(Dependency::current(DependencyKind::Callee, executable, 1))
        }
    }
    dependencies
}

fn direct_call_draft(executable: u64, pc: u32, permit: RecordPermit) -> SnapshotDraft {
    let point = SafepointId::new(1);
    let parameters = vec![
        ValueDef::new(ValueId::new(0), ValueType::I64),
        ValueDef::new(ValueId::new(1), ValueType::I64),
    ];
    let mut dependencies = vec![
        Dependency::current(DependencyKind::Executable, executable, executable),
        Dependency::current(DependencyKind::Schema, executable, permit.schema_epoch()),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
        Dependency::current(DependencyKind::Callee, u64::from(pc), 1),
    ];
    let identity = ExecutableIdentity::new(executable, executable);
    let recipe = DeoptRecipe::new(
        1,
        identity,
        pc,
        ResumeMode::ReplayBeforePc,
        vec![FrameRecipe::new(
            executable,
            pc,
            parameters
                .iter()
                .enumerate()
                .map(|(register, value)| {
                    RegisterRecipe::new(
                        u16::try_from(register).unwrap_or(u16::MAX),
                        RegisterSource::Ssa(value.id),
                        value.ty,
                    )
                })
                .collect(),
        )],
        point,
    )
    .with_dependencies(dependencies.clone());
    SnapshotDraft::new(
        identity,
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            parameters,
            vec![
                Instruction::safepoint(
                    InstructionKind::Call {
                        callee: u64::from(pc),
                    },
                    vec![ValueId::new(0), ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(2), ValueType::I64)),
                    Effect::Call,
                    point,
                )
                .ordered(0),
            ],
            Terminator::Return {
                values: vec![ValueId::new(2)],
            },
        )],
        vec![RootMap::new(point, Default::default())],
        vec![recipe],
        std::mem::take(&mut dependencies),
    )
    .with_schema_epoch(permit.schema_epoch())
}

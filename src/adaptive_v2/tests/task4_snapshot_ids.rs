use super::super::trace::EntryKind;
use super::super::wxir_v2::VerifiedSnapshot;
use super::super::wxir_v2::dependency::{Dependency, DependencyKind};
use super::super::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, SnapshotDraft, Terminator,
    ValueDef, ValueId, ValueType,
};
use super::task4_support::{compile_permit, dependencies, identity, rooted_helper_draft};

fn scalar_draft(value: i64, mut dependencies: Vec<Dependency>) -> SnapshotDraft {
    dependencies.reverse();
    SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        vec![Block::new(
            BlockId::new(0),
            vec![],
            vec![Instruction::new(
                InstructionKind::Constant(Constant::Integer(value)),
                vec![],
                Some(ValueDef::new(ValueId::new(0), ValueType::I64)),
                Effect::Pure,
            )],
            Terminator::Return {
                values: vec![ValueId::new(0)],
            },
        )],
        vec![],
        vec![],
        dependencies,
    )
    .with_schema_epoch(7)
}

fn seal(draft: SnapshotDraft) -> VerifiedSnapshot {
    VerifiedSnapshot::seal(draft, compile_permit(7)).expect("valid snapshot")
}

#[test]
fn snapshot_id_ignores_insertion_order_and_threads() {
    let first = seal(scalar_draft(1, dependencies(7)));
    let second = seal(scalar_draft(1, dependencies(7)));
    assert_eq!(first.id(), second.id());
    let draft = scalar_draft(1, dependencies(7));
    let joined = std::thread::spawn(move || seal(draft).id())
        .join()
        .expect("reader thread");
    assert_eq!(first.id(), joined);
}

#[test]
fn instruction_dep_parent_root_deopt_resume_changes_perturb_id() {
    let base = seal(scalar_draft(1, dependencies(7)));
    assert_ne!(base.id(), seal(scalar_draft(2, dependencies(7))).id());

    let mut changed_dependency = dependencies(7);
    let helper = changed_dependency
        .iter_mut()
        .find(|dependency| dependency.kind == DependencyKind::HelperAbi)
        .expect("helper dependency");
    *helper = Dependency::current(DependencyKind::HelperAbi, 0, 2);
    assert_ne!(base.id(), seal(scalar_draft(1, changed_dependency)).id());

    let mut parent = scalar_draft(1, dependencies(7));
    parent.body.parent = Some((base.id(), 3, 1));
    assert_ne!(base.id(), seal(parent).id());

    let rooted = seal(rooted_helper_draft());
    let mut resume = rooted_helper_draft();
    resume.body.deopts[0].resume_pc = 5;
    assert_ne!(rooted.id(), seal(resume).id());

    let mut mode = rooted_helper_draft();
    mode.body.deopts[0].mode = super::super::wxir_v2::deopt::ResumeMode::ResumeAfterPc;
    assert_ne!(rooted.id(), seal(mode).id());

    let mut roots = rooted_helper_draft();
    let root = super::super::wxir_v2::ir::RootLocation::Cache(7);
    roots.body.root_maps[0].roots.insert(root);
    roots.body.deopts[0].explicit_roots.push(root);
    assert_ne!(rooted.id(), seal(roots).id());
}

#[test]
fn snapshot_supports_readers_without_mutation() {
    let snapshot = seal(scalar_draft(1, dependencies(7)));
    let readers = (0..8)
        .map(|_| {
            let snapshot = snapshot.clone();
            std::thread::spawn(move || (snapshot.id(), snapshot.body().blocks.len()))
        })
        .collect::<Vec<_>>();
    for reader in readers {
        assert_eq!(reader.join().expect("snapshot reader"), (snapshot.id(), 1));
    }
}

#[test]
fn frame_reg_explicit_root_insertion_order_canonical() {
    let mut first = rooted_helper_draft();
    first.body.deopts[0].frames[0].registers.extend([
        super::super::wxir_v2::deopt::RegisterRecipe::new(
            1,
            super::super::wxir_v2::deopt::RegisterSource::Constant(Constant::Integer(5)),
            ValueType::I64,
        ),
        super::super::wxir_v2::deopt::RegisterRecipe::new(
            2,
            super::super::wxir_v2::deopt::RegisterSource::Constant(Constant::Boolean(true)),
            ValueType::Bool,
        ),
    ]);
    let roots = [
        super::super::wxir_v2::ir::RootLocation::Cache(1),
        super::super::wxir_v2::ir::RootLocation::HostPin(2),
    ];
    first.body.deopts[0].explicit_roots.extend(roots);
    first.body.root_maps[0].roots.extend(roots);

    let mut reordered = first.clone();
    reordered.body.deopts[0].frames[0].registers.reverse();
    reordered.body.deopts[0].explicit_roots.reverse();
    let canonical_id = seal(first.clone()).id();
    assert_eq!(canonical_id, seal(reordered).id());

    let mut changed = first;
    changed.body.deopts[0].frames[0].registers[1].source =
        super::super::wxir_v2::deopt::RegisterSource::Constant(Constant::Integer(6));
    assert_ne!(canonical_id, seal(changed).id());

    let mut typed_spill = rooted_helper_draft();
    typed_spill.body.deopts[0].virtuals = vec![super::super::wxir_v2::deopt::VirtualRecipe {
        id: 7,
        kind: super::super::wxir_v2::deopt::VirtualKind::List {
            items: vec![super::super::wxir_v2::deopt::RegisterSource::Spill {
                slot: 9,
                ty: ValueType::I64,
            }],
        },
    }];
    typed_spill.body.root_maps[0].roots.extend([
        super::super::wxir_v2::ir::RootLocation::Virtual(7),
        super::super::wxir_v2::ir::RootLocation::DeoptWorklist,
    ]);
    let mut changed_type = typed_spill.clone();
    let super::super::wxir_v2::deopt::VirtualKind::List { items } =
        &mut changed_type.body.deopts[0].virtuals[0].kind
    else {
        unreachable!("fixture is a list")
    };
    items[0] = super::super::wxir_v2::deopt::RegisterSource::Spill {
        slot: 9,
        ty: ValueType::F64,
    };
    assert_ne!(seal(typed_spill).id(), seal(changed_type).id());
}

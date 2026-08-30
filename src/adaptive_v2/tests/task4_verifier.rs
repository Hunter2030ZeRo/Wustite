use super::super::trace::EntryKind;
use super::super::wxir_v2::SnapshotError;
use super::super::wxir_v2::dependency::{Dependency, DependencyKind};
use super::super::wxir_v2::ir::{
    Block, BlockId, Constant, Effect, Instruction, InstructionKind, RootLocation, RootMap,
    SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
};
use super::task4_support::{dependencies, identity, rooted_helper_draft};

fn draft(blocks: Vec<Block>) -> SnapshotDraft {
    SnapshotDraft::new(
        identity(),
        EntryKind::FunctionEntry,
        BlockId::new(0),
        blocks,
        vec![],
        vec![],
        dependencies(7),
    )
    .with_schema_epoch(7)
}

fn constant(id: u32, ty: ValueType) -> Instruction {
    Instruction::new(
        InstructionKind::Constant(Constant::Integer(1)),
        vec![],
        Some(ValueDef::new(ValueId::new(id), ty)),
        Effect::Pure,
    )
}

#[test]
fn verifier_rejects_duplicate_undefined_use_pre_definitions() {
    let duplicate = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![constant(0, ValueType::I64), constant(0, ValueType::I64)],
        Terminator::Return { values: vec![] },
    )]);
    assert_eq!(
        duplicate.verify(),
        Err(SnapshotError::DuplicateDefinition { value: 0 })
    );

    let undefined = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![],
        Terminator::Return {
            values: vec![ValueId::new(9)],
        },
    )]);
    assert_eq!(
        undefined.verify(),
        Err(SnapshotError::UndefinedValue { value: 9 })
    );

    let before = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![
            Instruction::new(
                InstructionKind::IntegerAdd,
                vec![ValueId::new(1), ValueId::new(1)],
                Some(ValueDef::new(ValueId::new(0), ValueType::I64)),
                Effect::Pure,
            ),
            constant(1, ValueType::I64),
        ],
        Terminator::Return { values: vec![] },
    )]);
    assert_eq!(
        before.verify(),
        Err(SnapshotError::UseBeforeDefinition { value: 1 })
    );
}

#[test]
fn verifier_rejects_non_dominating_uses_bad_types_cfg_phi_edges() {
    let blocks = vec![
        Block::new(
            BlockId::new(0),
            vec![ValueDef::new(ValueId::new(0), ValueType::Bool)],
            vec![],
            Terminator::Branch {
                condition: ValueId::new(0),
                yes: BlockId::new(1),
                no: BlockId::new(2),
            },
        ),
        Block::new(
            BlockId::new(1),
            vec![],
            vec![constant(1, ValueType::I64)],
            Terminator::Jump {
                target: BlockId::new(3),
                arguments: vec![],
            },
        ),
        Block::new(
            BlockId::new(2),
            vec![],
            vec![],
            Terminator::Jump {
                target: BlockId::new(3),
                arguments: vec![],
            },
        ),
        Block::new(
            BlockId::new(3),
            vec![],
            vec![],
            Terminator::Return {
                values: vec![ValueId::new(1)],
            },
        ),
    ];
    assert_eq!(
        draft(blocks).verify(),
        Err(SnapshotError::NonDominatingUse { value: 1, block: 3 })
    );

    let bad_type = draft(vec![Block::new(
        BlockId::new(0),
        vec![ValueDef::new(ValueId::new(0), ValueType::Bool)],
        vec![Instruction::new(
            InstructionKind::IntegerAdd,
            vec![ValueId::new(0), ValueId::new(0)],
            Some(ValueDef::new(ValueId::new(1), ValueType::I64)),
            Effect::Pure,
        )],
        Terminator::Return { values: vec![] },
    )]);
    assert_eq!(
        bad_type.verify(),
        Err(SnapshotError::TypeMismatch { value: 1 })
    );

    let bad_cfg = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![],
        Terminator::Jump {
            target: BlockId::new(99),
            arguments: vec![],
        },
    )]);
    assert_eq!(
        bad_cfg.verify(),
        Err(SnapshotError::InvalidCfg { block: 0 })
    );

    let bad_phi = draft(vec![
        Block::new(
            BlockId::new(0),
            vec![],
            vec![constant(0, ValueType::I64)],
            Terminator::Jump {
                target: BlockId::new(1),
                arguments: vec![],
            },
        ),
        Block::new(
            BlockId::new(1),
            vec![ValueDef::new(ValueId::new(1), ValueType::I64)],
            vec![],
            Terminator::Return { values: vec![] },
        ),
    ]);
    assert_eq!(
        bad_phi.verify(),
        Err(SnapshotError::InvalidPhi { block: 1 })
    );
}

#[test]
fn verifier_rejects_bad_effect_guard_exit_op_deps() {
    let point = super::super::wxir_v2::ir::SafepointId::new(1);
    let bad_effect = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![Instruction::safepoint(
            InstructionKind::Helper { helper: 1 },
            vec![],
            None,
            Effect::Helper,
            point,
        )],
        Terminator::Return { values: vec![] },
    )]);
    assert_eq!(
        bad_effect.verify(),
        Err(SnapshotError::BadEffectOrdering { block: 0 })
    );

    let guard = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![Instruction::new(
            InstructionKind::Guard { guard: 5 },
            vec![],
            None,
            Effect::Pure,
        )],
        Terminator::Return { values: vec![] },
    )]);
    assert_eq!(guard.verify(), Err(SnapshotError::MissingDeopt { id: 5 }));

    let exit = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![],
        Terminator::SideExit {
            id: 6,
            values: vec![],
        },
    )]);
    assert_eq!(exit.verify(), Err(SnapshotError::MissingDeopt { id: 6 }));

    let mut object = draft(vec![Block::new(
        BlockId::new(0),
        vec![ValueDef::new(ValueId::new(0), ValueType::Handle)],
        vec![Instruction::new(
            InstructionKind::ObjectGet,
            vec![ValueId::new(0)],
            Some(ValueDef::new(ValueId::new(1), ValueType::Handle)),
            Effect::Read,
        )],
        Terminator::Return { values: vec![] },
    )]);
    assert_eq!(
        object.verify(),
        Err(SnapshotError::MissingDependency {
            kind: DependencyKind::Shape
        })
    );
    object
        .body
        .dependencies
        .push(Dependency::current(DependencyKind::Shape, 1, 1));
    assert_eq!(
        object.verify(),
        Err(SnapshotError::MissingDependency {
            kind: DependencyKind::Class
        })
    );
}

#[test]
fn verifier_rejects_missing_surplus_roots_stale_deps() {
    let mut missing = rooted_helper_draft();
    missing.body.root_maps[0].roots.clear();
    assert_eq!(
        missing.verify(),
        Err(SnapshotError::MissingRoot { point: 1 })
    );

    let mut surplus = rooted_helper_draft();
    surplus.body.root_maps[0]
        .roots
        .insert(RootLocation::Cache(9));
    assert_eq!(
        surplus.verify(),
        Err(SnapshotError::SurplusRoot { point: 1 })
    );

    let mut stale = rooted_helper_draft();
    stale.body.dependencies[0] = Dependency::observed(DependencyKind::Executable, 9, 3, 4);
    assert_eq!(
        stale.verify(),
        Err(SnapshotError::StaleDependency {
            kind: DependencyKind::Executable
        })
    );
}

#[test]
fn verifier_rejects_borrows_at_barriers() {
    for (index, effect) in [
        Effect::Allocation,
        Effect::Helper,
        Effect::Call,
        Effect::Backedge,
    ]
    .into_iter()
    .enumerate()
    {
        let point = super::super::wxir_v2::ir::SafepointId::new(
            u32::try_from(index + 1).expect("small point"),
        );
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
                    effect,
                    point,
                )
                .ordered(0),
            ],
            Terminator::Return { values: vec![] },
        );
        assert_eq!(
            draft(vec![block]).verify(),
            Err(SnapshotError::BorrowAcrossSafepoint { value: 1 })
        );
    }
}

#[test]
fn verifier_checks_dep_identity_allows_distinct_same_kind_deps() {
    let mut wrong_executable = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![],
        Terminator::Return { values: vec![] },
    )]);
    wrong_executable.body.dependencies[0] = Dependency::current(DependencyKind::Executable, 10, 3);
    assert_eq!(
        wrong_executable.verify(),
        Err(SnapshotError::DanglingDependency)
    );

    let mut wrong_schema = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![],
        Terminator::Return { values: vec![] },
    )]);
    wrong_schema.body.dependencies[1] = Dependency::current(DependencyKind::Schema, 7, 8);
    assert_eq!(
        wrong_schema.verify(),
        Err(SnapshotError::DanglingDependency)
    );

    let mut distinct_shapes = draft(vec![Block::new(
        BlockId::new(0),
        vec![ValueDef::new(ValueId::new(0), ValueType::Handle)],
        vec![Instruction::new(
            InstructionKind::ObjectGet,
            vec![ValueId::new(0)],
            Some(ValueDef::new(ValueId::new(1), ValueType::I64)),
            Effect::Read,
        )],
        Terminator::Return { values: vec![] },
    )]);
    distinct_shapes.body.dependencies.extend([
        Dependency::current(DependencyKind::Shape, 11, 1),
        Dependency::current(DependencyKind::Shape, 12, 1),
        Dependency::current(DependencyKind::Class, 13, 1),
    ]);
    assert_eq!(distinct_shapes.verify(), Ok(()));
}

#[test]
fn verifier_tracks_only_live_borrows_precise_spill_virtual_roots() {
    let point = super::super::wxir_v2::ir::SafepointId::new(1);
    let mut dead_before_barrier = rooted_helper_draft();
    dead_before_barrier.body.blocks[0].instructions.insert(
        0,
        Instruction::new(
            InstructionKind::BorrowView,
            vec![ValueId::new(0)],
            Some(ValueDef::new(ValueId::new(1), ValueType::BorrowedView)),
            Effect::Pure,
        ),
    );
    assert_eq!(dead_before_barrier.verify(), Ok(()));

    let mut roots = rooted_helper_draft();
    roots.body.deopts[0].frames[0].registers = vec![
        super::super::wxir_v2::deopt::RegisterRecipe::new(
            0,
            super::super::wxir_v2::deopt::RegisterSource::Spill {
                slot: 4,
                ty: ValueType::Handle,
            },
            ValueType::Handle,
        ),
        super::super::wxir_v2::deopt::RegisterRecipe::new(
            1,
            super::super::wxir_v2::deopt::RegisterSource::Virtual(7),
            ValueType::Handle,
        ),
    ];
    roots.body.deopts[0].virtuals = vec![super::super::wxir_v2::deopt::VirtualRecipe {
        id: 7,
        kind: super::super::wxir_v2::deopt::VirtualKind::List {
            items: vec![
                super::super::wxir_v2::deopt::RegisterSource::Spill {
                    slot: 9,
                    ty: ValueType::Handle,
                },
                super::super::wxir_v2::deopt::RegisterSource::Spill {
                    slot: 10,
                    ty: ValueType::I64,
                },
            ],
        },
    }];
    roots.body.root_maps = vec![RootMap::new(
        point,
        [
            RootLocation::Spill(4),
            RootLocation::Spill(9),
            RootLocation::Virtual(7),
            RootLocation::DeoptWorklist,
        ]
        .into_iter()
        .collect(),
    )];
    assert_eq!(roots.verify(), Ok(()));
    roots.body.root_maps[0]
        .roots
        .remove(&RootLocation::Spill(9));
    assert_eq!(roots.verify(), Err(SnapshotError::MissingRoot { point: 1 }));
    roots.body.root_maps[0].roots.insert(RootLocation::Spill(9));
    roots.body.root_maps[0]
        .roots
        .insert(RootLocation::Spill(10));
    assert_eq!(roots.verify(), Err(SnapshotError::SurplusRoot { point: 1 }));
    roots.body.root_maps[0]
        .roots
        .remove(&RootLocation::Spill(10));
    roots.body.root_maps[0]
        .roots
        .remove(&RootLocation::DeoptWorklist);
    assert_eq!(roots.verify(), Err(SnapshotError::MissingRoot { point: 1 }));

    let mut bad_constant = rooted_helper_draft();
    bad_constant.body.deopts[0].frames[0].registers[0].source =
        super::super::wxir_v2::deopt::RegisterSource::Constant(Constant::HandleBits(1));
    bad_constant.body.root_maps[0].roots.clear();
    assert_eq!(
        bad_constant.verify(),
        Err(SnapshotError::InvalidDeopt { id: 1 })
    );
}

#[test]
fn verifier_rejects_unbound_branch_params_missing_backedge_recipe() {
    for parameterized in [BlockId::new(1), BlockId::new(2)] {
        let blocks = vec![
            Block::new(
                BlockId::new(0),
                vec![ValueDef::new(ValueId::new(0), ValueType::Bool)],
                vec![],
                Terminator::Branch {
                    condition: ValueId::new(0),
                    yes: BlockId::new(1),
                    no: BlockId::new(2),
                },
            ),
            Block::new(
                BlockId::new(1),
                if parameterized == BlockId::new(1) {
                    vec![ValueDef::new(ValueId::new(1), ValueType::I64)]
                } else {
                    vec![]
                },
                vec![],
                Terminator::Return { values: vec![] },
            ),
            Block::new(
                BlockId::new(2),
                if parameterized == BlockId::new(2) {
                    vec![ValueDef::new(ValueId::new(2), ValueType::I64)]
                } else {
                    vec![]
                },
                vec![],
                Terminator::Return { values: vec![] },
            ),
        ];
        assert_eq!(
            draft(blocks).verify(),
            Err(SnapshotError::InvalidPhi {
                block: parameterized.get()
            })
        );
    }

    let backedge = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![],
        Terminator::Backedge {
            target_pc: 0,
            safepoint: super::super::wxir_v2::ir::SafepointId::new(8),
        },
    )]);
    assert_eq!(
        backedge.verify(),
        Err(SnapshotError::MissingDeopt { id: 8 })
    );

    let no_point = draft(vec![Block::new(
        BlockId::new(0),
        vec![],
        vec![
            Instruction::new(
                InstructionKind::Helper { helper: 1 },
                vec![],
                None,
                Effect::Helper,
            )
            .ordered(0),
        ],
        Terminator::Return { values: vec![] },
    )]);
    assert_eq!(
        no_point.verify(),
        Err(SnapshotError::MissingSafepoint { block: 0 })
    );
}

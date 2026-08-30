use super::*;
use crate::adaptive_v2::heap::{GcConfig, GcHeap};
use crate::adaptive_v2::native::{NativeCompiler, NativeValue};
use crate::adaptive_v2::profile::{AdaptiveProfile, FactClass, LiveObservation, ProfileCase};
use crate::adaptive_v2::shapes::ShapeTable;
use crate::adaptive_v2::symbols::SymbolTable;
use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity};
use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
use crate::adaptive_v2::wxir_v2::deopt::{
    DeoptRecipe, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode, VirtualKind,
};
use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId, Constant, Instruction, RootLocation, RootMap, SnapshotDraft, Terminator,
    ValueDef, ValueId, ValueType, WxIrAbi,
};
use crate::adaptive_v2::wxir_v2::materialize::{DeoptEngine, MaterializedKind, RuntimeAtom};

fn dependencies(shape: (u64, u64, u64)) -> Vec<Dependency> {
    vec![
        Dependency::current(DependencyKind::Executable, 9, 9),
        Dependency::current(DependencyKind::Schema, 9, 9),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
        Dependency::current(DependencyKind::Shape, shape.0, shape.1),
    ]
}

fn deopt(
    id: u32,
    point: SafepointId,
    registers: Vec<RegisterRecipe>,
    shape: (u64, u64, u64),
) -> DeoptRecipe {
    DeoptRecipe::new(
        id,
        ExecutableIdentity::new(9, 9),
        id,
        ResumeMode::ReplayBeforePc,
        vec![FrameRecipe::new(9, id, registers)],
        point,
    )
    .with_dependencies(dependencies(shape))
}

fn shapes() -> (ShapeTable, (u64, u64, u64)) {
    let mut symbols = SymbolTable::new();
    let field = symbols.intern("field").expect("field symbol");
    let mut shapes = ShapeTable::new(symbols.namespace());
    let class = shapes.new_class();
    shapes.invalidate_class(class).expect("fresh shape epoch");
    let root = shapes.root_shape(class).expect("root shape");
    let child = shapes.transition(root, field).expect("field shape");
    let key = shapes.key(child).expect("shape key").serialized_parts();
    (shapes, key)
}

fn body(shape: (u64, u64, u64)) -> SnapshotBody {
    let allocation = SafepointId::new(1);
    let guard = SafepointId::new(2);
    SnapshotBody {
        abi: WxIrAbi::V2,
        executable: ExecutableIdentity::new(9, 9),
        schema_epoch: 9,
        entry_kind: EntryKind::FunctionEntry,
        entry: BlockId::new(0),
        parent: None,
        blocks: vec![Block::new(
            BlockId::new(0),
            vec![
                ValueDef::new(ValueId::new(0), ValueType::I64),
                ValueDef::new(ValueId::new(1), ValueType::Bool),
            ],
            vec![
                Instruction::new(
                    InstructionKind::Constant(Constant::Integer(7)),
                    Vec::new(),
                    Some(ValueDef::new(ValueId::new(2), ValueType::I64)),
                    Effect::Pure,
                ),
                Instruction::safepoint(
                    InstructionKind::Allocate,
                    Vec::new(),
                    Some(ValueDef::new(ValueId::new(3), ValueType::Handle)),
                    Effect::Allocation,
                    allocation,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ObjectSet,
                    vec![ValueId::new(3), ValueId::new(2), ValueId::new(0)],
                    None,
                    Effect::Write,
                )
                .ordered(1),
                Instruction::safepoint(
                    InstructionKind::Guard { guard: 2 },
                    vec![ValueId::new(1)],
                    None,
                    Effect::Pure,
                    guard,
                ),
                Instruction::new(
                    InstructionKind::ObjectGet,
                    vec![ValueId::new(3), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::I64)),
                    Effect::Read,
                ),
            ],
            Terminator::Return {
                values: vec![ValueId::new(4)],
            },
        )],
        root_maps: vec![
            RootMap::new(allocation, BTreeSet::new()),
            RootMap::new(guard, BTreeSet::from([RootLocation::Ssa(ValueId::new(3))])),
        ],
        deopts: vec![
            deopt(1, allocation, Vec::new(), shape),
            deopt(
                2,
                guard,
                vec![
                    RegisterRecipe::new(0, RegisterSource::Ssa(ValueId::new(3)), ValueType::Handle),
                    RegisterRecipe::new(1, RegisterSource::Ssa(ValueId::new(3)), ValueType::Handle),
                ],
                shape,
            ),
        ],
        dependencies: dependencies(shape),
    }
}

#[test]
fn forced_deopt_keeps_one_virtual_all_aliases_fields() {
    // Given: a nonescaping object live at a guard through two aliasing registers.
    let (shapes, shape) = shapes();
    let mut body = body(shape);

    // When: scalar replacement runs across the allocation-to-guard interval.
    let changed = run(&mut body);

    // Then: allocation traffic disappears while the guard recipe reconstructs identity and data.
    assert!(changed);
    let guard = body
        .deopts
        .iter()
        .find(|recipe| recipe.id == 2)
        .expect("guard recipe");
    assert_eq!(guard.virtuals.len(), 1);
    let virtual_id = guard.virtuals[0].id;
    assert!(
        guard.frames[0]
            .registers
            .iter()
            .all(|register| register.source == RegisterSource::Virtual(virtual_id))
    );
    assert!(matches!(
        &guard.virtuals[0].kind,
        VirtualKind::Object { fields, .. }
            if fields == &vec![(7, RegisterSource::Ssa(ValueId::new(0)))]
    ));
    assert_eq!(
        body.root_maps
            .iter()
            .find(|map| map.point == SafepointId::new(2))
            .expect("guard roots")
            .roots,
        BTreeSet::from([
            RootLocation::Virtual(virtual_id),
            RootLocation::DeoptWorklist,
        ])
    );
    assert!(body.blocks[0].instructions.iter().all(|instruction| {
        !matches!(
            instruction.kind.semantic(),
            InstructionKind::Allocate | InstructionKind::ObjectSet | InstructionKind::ObjectGet
        )
    }));
    let heap = GcHeap::new(GcConfig::default());
    let values = BTreeMap::from([(ValueId::new(0), RuntimeAtom::Integer(81))]);
    let state = DeoptEngine::new(&heap, &values, &BTreeMap::new())
        .with_shapes(&shapes)
        .reconstruct(guard)
        .expect("transactional deopt materialization");
    let materialized = state.virtuals.first().expect("materialized object");
    assert_eq!(state.frames[0].registers[0], state.frames[0].registers[1]);
    assert_eq!(
        state.frames[0].registers[0],
        RuntimeAtom::Handle(materialized.handle)
    );
    assert!(matches!(
        &materialized.kind,
        MaterializedKind::Object { fields, .. }
            if fields == &vec![(7, RuntimeAtom::Integer(81))]
    ));
}

#[test]
fn scalar_replacement_crosses_jump_phi_identical_field_state() {
    // Given: the only incoming phi value aliases an object initialized in its dominator.
    let (_shapes, shape) = shapes();
    let mut body = body(shape);
    let original = body.blocks.remove(0);
    let mut instructions = original.instructions.into_iter();
    let constant = instructions.next().expect("field constant");
    let allocate = instructions.next().expect("allocation");
    let set = instructions.next().expect("field store");
    let guard = instructions.next().expect("guard");
    let mut get = instructions.next().expect("field load");
    get.inputs[0] = ValueId::new(5);
    body.blocks = vec![
        Block::new(
            BlockId::new(0),
            original.parameters,
            vec![constant, allocate, set],
            Terminator::Jump {
                target: BlockId::new(1),
                arguments: vec![ValueId::new(3)],
            },
        ),
        Block::new(
            BlockId::new(1),
            vec![ValueDef::new(ValueId::new(5), ValueType::Handle)],
            vec![guard, get],
            original.terminator,
        ),
    ];
    for recipe in &mut body.deopts {
        for register in &mut recipe.frames[0].registers {
            if register.source == RegisterSource::Ssa(ValueId::new(3)) {
                register.source = RegisterSource::Ssa(ValueId::new(5));
            }
        }
    }
    for map in &mut body.root_maps {
        if map.roots.remove(&RootLocation::Ssa(ValueId::new(3))) {
            map.roots.insert(RootLocation::Ssa(ValueId::new(5)));
        }
    }

    // When: whole-body scalar replacement follows the alias through the jump argument.
    assert!(run(&mut body));

    // Then: both sides of the phi are removed and the load uses the dominating field SSA value.
    assert!(body.blocks[1].parameters.is_empty());
    assert!(matches!(
        body.blocks[0].terminator,
        Terminator::Jump {
            ref arguments,
            ..
        } if arguments.is_empty()
    ));
    assert!(matches!(
        body.blocks[1].instructions[1].kind.semantic(),
        InstructionKind::Copy
    ));
    assert_eq!(body.blocks[1].instructions[1].inputs, vec![ValueId::new(0)]);
    assert_eq!(body.deopts[0].virtuals.len(), 1);
}

#[test]
fn scalar_replacement_rejects_backedge_region() {
    // Given: an otherwise replaceable allocation whose region ends in a loop backedge.
    let (_shapes, shape) = shapes();
    let mut body = body(shape);
    body.blocks[0].terminator = Terminator::Backedge {
        target_pc: 0,
        safepoint: SafepointId::new(3),
    };
    let original = body.clone();

    // When: scalar replacement examines the cyclic region.
    let changed = run(&mut body);

    // Then: it preserves the allocation because loop-carried virtual state is not proven.
    assert!(!changed);
    assert_eq!(body, original);
}

fn diamond_body(shape: (u64, u64, u64), divergent: bool) -> SnapshotBody {
    let mut body = body(shape);
    let original = body.blocks.remove(0);
    let mut instructions = original.instructions.into_iter();
    let constant = instructions.next().expect("field constant");
    let allocate = instructions.next().expect("allocation");
    let set = instructions.next().expect("field store");
    let guard = instructions.next().expect("guard");
    let mut get = instructions.next().expect("field load");
    get.inputs[0] = ValueId::new(5);
    let divergent_set = divergent.then(|| {
        Instruction::new(
            InstructionKind::ObjectSet,
            vec![ValueId::new(3), ValueId::new(2), ValueId::new(0)],
            None,
            Effect::Write,
        )
    });
    body.blocks = vec![
        Block::new(
            BlockId::new(0),
            original.parameters,
            vec![constant, allocate, set],
            Terminator::Branch {
                condition: ValueId::new(1),
                yes: BlockId::new(1),
                no: BlockId::new(2),
            },
        ),
        Block::new(
            BlockId::new(1),
            Vec::new(),
            divergent_set.into_iter().collect(),
            Terminator::Jump {
                target: BlockId::new(3),
                arguments: vec![ValueId::new(3)],
            },
        ),
        Block::new(
            BlockId::new(2),
            Vec::new(),
            Vec::new(),
            Terminator::Jump {
                target: BlockId::new(3),
                arguments: vec![ValueId::new(3)],
            },
        ),
        Block::new(
            BlockId::new(3),
            vec![ValueDef::new(ValueId::new(5), ValueType::Handle)],
            vec![guard, get],
            original.terminator,
        ),
    ];
    for recipe in &mut body.deopts {
        for register in &mut recipe.frames[0].registers {
            if register.source == RegisterSource::Ssa(ValueId::new(3)) {
                register.source = RegisterSource::Ssa(ValueId::new(5));
            }
        }
    }
    for map in &mut body.root_maps {
        if map.roots.remove(&RootLocation::Ssa(ValueId::new(3))) {
            map.roots.insert(RootLocation::Ssa(ValueId::new(5)));
        }
    }
    body
}

#[test]
fn branch_merge_needs_same_virtual_fields() {
    // Given: one diamond with a shared state and one with a path-local mutation barrier.
    let (_shapes, shape) = shapes();
    let mut identical = diamond_body(shape, false);
    let mut divergent = diamond_body(shape, true);
    let divergent_original = divergent.clone();

    // When: scalar replacement evaluates both merge states.
    let identical_changed = run(&mut identical);
    let divergent_changed = run(&mut divergent);

    // Then: the identical merge is virtualized and the divergent merge remains authoritative.
    assert!(identical_changed);
    assert!(identical.blocks[3].parameters.is_empty());
    assert!(!divergent_changed);
    assert_eq!(divergent, divergent_original);
}

fn compile_permit(schema_epoch: u64) -> crate::adaptive_v2::profile::CompilePermit {
    let mut profile = AdaptiveProfile::new(9);
    let observation = LiveObservation::new(ProfileCase::new(1), FactClass::UnknownClassified);
    for _ in 0..64 {
        profile.observe_live(observation);
    }
    profile.take_record_permit().expect("record permit");
    assert!(profile.finish_recording());
    for _ in 0..32 {
        profile.observe_live(observation);
    }
    let permit = profile.take_compile_permit().expect("compile permit");
    assert_eq!(permit.schema_epoch(), schema_epoch);
    permit
}

#[test]
fn nonescaping_object_runs_native_helper_free() {
    // Given: a state-dependent object store/get with a reconstructible guard exit.
    let (_shapes, shape) = shapes();
    let mut optimized = body(shape);
    assert!(run(&mut optimized));
    let draft = SnapshotDraft::new(
        optimized.executable,
        optimized.entry_kind,
        optimized.entry,
        optimized.blocks,
        optimized.root_maps,
        optimized.deopts,
        optimized.dependencies,
    )
    .with_schema_epoch(optimized.schema_epoch);
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit(9)).expect("optimized snapshot");
    let code = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("native object scalar replacement");

    // When: the same machine code receives two distinct stored values.
    let first = code
        .execute(&[NativeValue::Integer(17), NativeValue::Boolean(true)])
        .expect("first object execution");
    let second = code
        .execute(&[NativeValue::Integer(93), NativeValue::Boolean(true)])
        .expect("second object execution");

    // Then: results remain input-dependent and no object helper is emitted or called.
    assert_eq!(first.values, vec![NativeValue::Integer(17)]);
    assert_eq!(second.values, vec![NativeValue::Integer(93)]);
    assert_eq!(first.counters.helper_calls, 0);
    assert_eq!(second.counters.helper_calls, 0);
    eprintln!(
        "FUSION_OBJECT values={:?},{:?} helper_calls={},{}",
        first.values, second.values, first.counters.helper_calls, second.counters.helper_calls
    );
    assert!(snapshot.body().blocks.iter().all(|block| {
        block.instructions.iter().all(|instruction| {
            !matches!(
                instruction.kind.semantic(),
                InstructionKind::Allocate
                    | InstructionKind::ObjectSet
                    | InstructionKind::ObjectGet
                    | InstructionKind::Helper { .. }
            )
        })
    }));
}

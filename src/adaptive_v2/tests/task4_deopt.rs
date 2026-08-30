use std::collections::BTreeMap;

use super::super::heap::{GcConfig, GcError, GcHeap, GcObject};
use super::super::lists::ListStrategy;
use super::super::shapes::ShapeTable;
use super::super::symbols::SymbolTable;
use super::super::wxir_v2::deopt::{
    DeoptRecipe, ExceptionState, FrameRecipe, RegisterRecipe, RegisterSource, ResumeMode,
    VirtualKind, VirtualRecipe,
};
use super::super::wxir_v2::dependency::{Dependency, DependencyKind};
use super::super::wxir_v2::ir::{Constant, SafepointId, ValueId, ValueType};
use super::super::wxir_v2::materialize::{DeoptEngine, DeoptError, RuntimeAtom};
use super::task4_support::{dependencies, identity};

fn shapes() -> (ShapeTable, super::super::shapes::ClassId, (u64, u64, u64)) {
    let mut symbols = SymbolTable::new();
    let field = symbols.intern("field").expect("field symbol");
    let mut shapes = ShapeTable::new(symbols.namespace());
    let class = shapes.new_class();
    let root = shapes.root_shape(class).expect("root shape");
    let child = shapes.transition(root, field).expect("field shape");
    let key = shapes.key(child).expect("shape key").serialized_parts();
    (shapes, class, key)
}

fn cyclic_recipe(shape: (u64, u64, u64)) -> DeoptRecipe {
    let object = VirtualRecipe {
        id: 0,
        kind: VirtualKind::Object {
            shape_identity: shape.0,
            shape_dependency_epoch: shape.1,
            shape_layout_epoch: shape.2,
            fields: vec![(0, RegisterSource::Virtual(1))],
        },
    };
    let list = VirtualRecipe {
        id: 1,
        kind: VirtualKind::List {
            items: vec![
                RegisterSource::Virtual(0),
                RegisterSource::Ssa(ValueId::new(0)),
                RegisterSource::Constant(Constant::Integer(7)),
            ],
        },
    };
    DeoptRecipe::new(
        1,
        identity(),
        40,
        ResumeMode::ReplayBeforePc,
        vec![
            FrameRecipe::new(
                10,
                20,
                vec![
                    RegisterRecipe::new(0, RegisterSource::Virtual(0), ValueType::Handle),
                    RegisterRecipe::new(1, RegisterSource::Ssa(ValueId::new(1)), ValueType::I64),
                ],
            ),
            FrameRecipe::new(
                11,
                30,
                vec![RegisterRecipe::new(
                    0,
                    RegisterSource::Virtual(1),
                    ValueType::Handle,
                )],
            )
            .with_exception(ExceptionState::Pending {
                class: 99,
                message: "boom".into(),
            }),
        ],
        SafepointId::new(1),
    )
    .with_virtuals(vec![object, list])
    .with_dependencies(dependencies(7))
}

#[test]
fn forced_deopt_restores_frames_aliases_fields_and_lists() {
    let heap = GcHeap::new(GcConfig {
        collect_every_allocation: true,
        promotion_age: 1,
        allocation_limit: Some(16),
    });
    let external = heap.allocate(GcObject::new()).expect("external handle");
    let values = BTreeMap::from([
        (ValueId::new(0), RuntimeAtom::Handle(external)),
        (ValueId::new(1), RuntimeAtom::Integer(42)),
    ]);
    let spills = BTreeMap::new();
    let (shapes, _class, key) = shapes();
    let state = DeoptEngine::new(&heap, &values, &spills)
        .with_shapes(&shapes)
        .reconstruct(&cyclic_recipe(key))
        .expect("atomic deopt reconstruction");
    assert_eq!(state.mode, ResumeMode::ReplayBeforePc);
    assert_eq!(state.resume_pc, 40);
    assert_eq!(state.frames.len(), 2);
    assert_eq!(state.frames[0].registers[1], RuntimeAtom::Integer(42));
    assert!(matches!(
        state.frames[1].exception,
        ExceptionState::Pending { class: 99, .. }
    ));
    let object = state.virtuals[0].handle;
    let list = state.virtuals[1].handle;
    assert_eq!(state.frames[0].registers[0], RuntimeAtom::Handle(object));
    assert_eq!(state.frames[1].registers[0], RuntimeAtom::Handle(list));
    assert_eq!(heap.resolve(object).expect("object").references(), &[list]);
    assert_eq!(
        heap.resolve(list).expect("list").references(),
        &[object, external]
    );
    assert!(
        matches!(&state.virtuals[1].kind, super::super::wxir_v2::materialize::MaterializedKind::List { strategy: ListStrategy::Generic, items } if items == &vec![RuntimeAtom::Handle(object), RuntimeAtom::Handle(external), RuntimeAtom::Integer(7)])
    );
}

#[test]
fn alloc_helper_malformed_frame_failures_publish_no_partial_graph() {
    let heap = GcHeap::new(GcConfig {
        allocation_limit: Some(2),
        ..GcConfig::default()
    });
    let external = heap.allocate(GcObject::new()).expect("one live slot");
    let values = BTreeMap::from([
        (ValueId::new(0), RuntimeAtom::Handle(external)),
        (ValueId::new(1), RuntimeAtom::Integer(1)),
    ]);
    let spills = BTreeMap::new();
    let (shapes, _class, key) = shapes();
    let recipe = cyclic_recipe(key);
    assert_eq!(
        DeoptEngine::new(&heap, &values, &spills)
            .with_shapes(&shapes)
            .reconstruct(&recipe),
        Err(DeoptError::InvalidHandle(GcError::AllocationLimit))
    );
    heap.allocate(GcObject::new())
        .expect("failed graph consumed no slot");

    let heap = GcHeap::new(GcConfig {
        allocation_limit: Some(2),
        ..GcConfig::default()
    });
    let mut malformed = recipe.clone();
    malformed.frames[0].registers[0].register = 3;
    assert!(matches!(
        DeoptEngine::new(&heap, &values, &spills)
            .with_shapes(&shapes)
            .reconstruct(&malformed),
        Err(DeoptError::NonContiguousRegisters { .. })
    ));
    heap.allocate(GcObject::new())
        .expect("malformed frame allocated nothing");

    assert_eq!(
        DeoptEngine::new(&heap, &values, &spills)
            .with_shapes(&shapes)
            .with_forced_helper_failure(77)
            .reconstruct(&recipe),
        Err(DeoptError::HelperFailure { helper: 77 })
    );
    heap.allocate(GcObject::new())
        .expect("helper failure allocated nothing");
}

#[test]
fn deopt_rejects_stale_shapes_and_foreign_handles() {
    let heap = GcHeap::new(GcConfig::default());
    let foreign_heap = GcHeap::new(GcConfig::default());
    let foreign = foreign_heap.allocate(GcObject::new()).expect("foreign");
    let values = BTreeMap::from([
        (ValueId::new(0), RuntimeAtom::Handle(foreign)),
        (ValueId::new(1), RuntimeAtom::Integer(1)),
    ]);
    let spills = BTreeMap::new();
    let (mut shapes, class, key) = shapes();
    let recipe = cyclic_recipe(key);
    assert!(matches!(
        DeoptEngine::new(&heap, &values, &spills)
            .with_shapes(&shapes)
            .reconstruct(&recipe),
        Err(DeoptError::InvalidHandle(_))
    ));

    shapes.invalidate_class(class).expect("invalidate shape");
    let local = heap.allocate(GcObject::new()).expect("local");
    let values = BTreeMap::from([
        (ValueId::new(0), RuntimeAtom::Handle(local)),
        (ValueId::new(1), RuntimeAtom::Integer(1)),
    ]);
    assert_eq!(
        DeoptEngine::new(&heap, &values, &spills)
            .with_shapes(&shapes)
            .reconstruct(&recipe),
        Err(DeoptError::StaleShape { shape: key.0 })
    );

    let mut stale = recipe;
    stale
        .dependencies
        .push(Dependency::observed(DependencyKind::Callee, 1, 1, 2));
    assert_eq!(
        DeoptEngine::new(&heap, &values, &spills)
            .with_shapes(&shapes)
            .reconstruct(&stale),
        Err(DeoptError::StaleDependency)
    );
}

#[test]
fn deopt_resume_handles_spills_and_repeats() {
    let heap = GcHeap::new(GcConfig::default());
    let values = BTreeMap::new();
    let spills = BTreeMap::from([(4, RuntimeAtom::FloatBits(0x7ff8_1234_5678_9abc))]);
    let recipe = DeoptRecipe::new(
        2,
        identity(),
        12,
        ResumeMode::ResumeAfterPc,
        vec![
            FrameRecipe::new(
                1,
                12,
                vec![
                    RegisterRecipe::new(
                        0,
                        RegisterSource::Spill {
                            slot: 4,
                            ty: ValueType::F64,
                        },
                        ValueType::F64,
                    ),
                    RegisterRecipe::new(1, RegisterSource::UndefinedDead, ValueType::Handle),
                ],
            )
            .with_dead_registers([1]),
        ],
        SafepointId::new(2),
    )
    .with_dependencies(dependencies(7));
    let engine = DeoptEngine::new(&heap, &values, &spills);
    let mut bad_constant = recipe.clone();
    bad_constant.frames[0].registers[1].source = RegisterSource::Constant(Constant::HandleBits(1));
    bad_constant.frames[0].dead_registers.clear();
    assert_eq!(
        engine.reconstruct(&bad_constant),
        Err(DeoptError::InvalidConstant)
    );
    let mut mismatched_virtual_spill = recipe.clone();
    mismatched_virtual_spill.virtuals = vec![VirtualRecipe {
        id: 9,
        kind: VirtualKind::List {
            items: vec![RegisterSource::Spill {
                slot: 4,
                ty: ValueType::I64,
            }],
        },
    }];
    assert_eq!(
        engine.reconstruct(&mismatched_virtual_spill),
        Err(DeoptError::TypeMismatch { register: u16::MAX })
    );
    let first = engine.reconstruct(&recipe).expect("first deopt");
    let second = engine.reconstruct(&recipe).expect("repeated deopt");
    assert_eq!(first.mode, ResumeMode::ResumeAfterPc);
    assert_eq!(
        first.frames[0].registers,
        vec![
            RuntimeAtom::FloatBits(0x7ff8_1234_5678_9abc),
            RuntimeAtom::UndefinedDead
        ]
    );
    assert_eq!(first, second);
}

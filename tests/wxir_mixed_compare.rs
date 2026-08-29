use wustite::bytecode::{CompareOperator, Function, Instruction};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::planner::JitPlan;
use wustite::structure_map::{
    Fact, OperationSiteId, RegionExit, RegionId, RegionKind, SlotType, StateSlot,
    StructureMapBuilder,
};
use wustite::wxir::{WxInstKind, build_region};

#[test]
fn mixed_integer_float_comparison_preserves_exact_runtime_semantics() {
    // Given: a proven mixed comparison that cannot safely round the integer to f64.
    let function = Function {
        register_count: 3,
        code: vec![
            Instruction::CompareOp {
                dst: 2,
                op: CompareOperator::Eq,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::Branch {
                cond: 2,
                yes: 2,
                no: 3,
            },
            Instruction::Jump { target: 0 },
            Instruction::Return { src: 2 },
        ],
    };
    let slots = vec![
        StateSlot {
            register: 0,
            ty: SlotType::SmallInt,
        },
        StateSlot {
            register: 1,
            ty: SlotType::Float,
        },
    ];
    let mut builder = StructureMapBuilder::new();
    builder
        .record_operation(
            0,
            Fact::Proven(SlotType::SmallInt),
            Fact::Proven(SlotType::Float),
            Fact::Proven(SlotType::Bool),
        )
        .unwrap();
    let region_id = builder.begin_region(0, slots.clone());
    builder
        .finish_region(
            region_id,
            RegionKind::Loop { backedge: 2 },
            vec![RegionExit { target: 3 }],
        )
        .unwrap();
    let structure_map = builder
        .finish(&function.code, function.register_count)
        .unwrap();
    let executable = ExecutableFunction::new_with_parameters(
        function,
        structure_map,
        vec![
            ExecutableParameter {
                name: "integer".to_owned(),
                register: 0,
                ty: SlotType::SmallInt,
            },
            ExecutableParameter {
                name: "float".to_owned(),
                register: 1,
                ty: SlotType::Float,
            },
        ],
    );
    let region = executable.structure_map().region(RegionId(0)).unwrap();

    // When: the comparison is lowered with complete proven facts.
    let wxir = build_region(
        &executable,
        &JitPlan {
            region_id: RegionId(0),
            header: 0,
            backedge: 2,
            exits: vec![RegionExit { target: 3 }],
            live_slots: slots,
            blocks: region.blocks.clone(),
            summary: region.summary,
        },
    )
    .unwrap();

    // Then: exact mixed comparison remains a runtime operation instead of an f64 cast.
    assert!(
        wxir.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(instruction.kind, WxInstKind::RuntimeCall { pc: 0, .. }))
    );
}

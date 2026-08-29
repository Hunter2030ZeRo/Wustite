use wustite::bytecode::{BinaryOperator, Function, Instruction};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::planner::JitPlan;
use wustite::structure_map::{
    Fact, OperationSiteId, RegionExit, RegionId, RegionKind, SlotType, StateSlot,
    StructureMapBuilder, TypeFact,
};
use wustite::wxir::{WxInstKind, build_region};

fn lower(fact: TypeFact) -> wustite::wxir::WxFunction {
    let function = Function {
        register_count: 4,
        code: vec![
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Add,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::ConstBool {
                dst: 3,
                value: true,
            },
            Instruction::Branch {
                cond: 3,
                yes: 3,
                no: 4,
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
            ty: SlotType::SmallInt,
        },
    ];
    let mut builder = StructureMapBuilder::new();
    builder.record_operation(0, fact, fact, fact).unwrap();
    let region_id = builder.begin_region(0, slots.clone());
    builder
        .finish_region(
            region_id,
            RegionKind::Loop { backedge: 3 },
            vec![RegionExit { target: 4 }],
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
                name: "lhs".to_string(),
                register: 0,
                ty: SlotType::SmallInt,
            },
            ExecutableParameter {
                name: "rhs".to_string(),
                register: 1,
                ty: SlotType::SmallInt,
            },
        ],
    );
    let region = executable.structure_map().region(RegionId(0)).unwrap();
    build_region(
        &executable,
        &JitPlan {
            region_id: RegionId(0),
            header: 0,
            backedge: 3,
            exits: vec![RegionExit { target: 4 }],
            live_slots: slots,
            blocks: region.blocks.clone(),
            summary: region.summary,
        },
    )
    .unwrap()
}

#[test]
fn typed_ssa_inputs_allow_guarded_and_unknown_operation_facts_to_lower_directly() {
    // Given: the same runtime-typed entry state with Guardable and Unknown operation facts.
    let guarded = lower(Fact::Guardable(SlotType::SmallInt));
    let unknown = lower(Fact::Unknown);

    // When: the operation at bytecode pc zero is inspected in both WXIR functions.
    let guarded_runtime = guarded
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| matches!(instruction.kind, WxInstKind::RuntimeCall { pc: 0, .. }));
    let unknown_runtime = unknown
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| matches!(instruction.kind, WxInstKind::RuntimeCall { pc: 0, .. }));

    // Then: entry type guards justify both specializations without a redundant runtime call.
    assert!(!guarded_runtime);
    assert!(!unknown_runtime);
}

use wustite::bytecode::{Function, Instruction};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::planner::JitPlan;
use wustite::structure_map::{
    RegionExit, RegionId, RegionKind, SlotType, StateSlot, StructureMapBuilder,
};
use wustite::wxir::{WxConstant, WxInstKind, build_region, verify};

#[test]
fn local_tuple_length_eliminates_allocation_and_runtime_dispatch() {
    // Given: a tuple allocation used only by the immediately following length operation.
    let function = Function {
        register_count: 5,
        code: vec![
            Instruction::BuildTuple {
                dst: 2,
                items: vec![0, 1],
            },
            Instruction::Length { dst: 3, object: 2 },
            Instruction::Move { dst: 0, src: 3 },
            Instruction::ConstBool {
                dst: 4,
                value: true,
            },
            Instruction::Branch {
                cond: 4,
                yes: 5,
                no: 6,
            },
            Instruction::Jump { target: 0 },
            Instruction::Return { src: 0 },
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
    let region_id = builder.begin_region(0, slots.clone());
    builder
        .finish_region(
            region_id,
            RegionKind::Loop { backedge: 5 },
            vec![RegionExit { target: 6 }],
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
    let plan = JitPlan {
        region_id: RegionId(0),
        header: 0,
        backedge: 5,
        exits: vec![RegionExit { target: 6 }],
        live_slots: slots,
        blocks: region.blocks.clone(),
        summary: region.summary,
    };

    // When: the StructureMap-backed region is lowered and optimized.
    let function = build_region(&executable, &plan).unwrap();

    // Then: the tuple is virtual and its length is an ordinary scalar constant.
    assert!(
        function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(
                instruction.kind,
                WxInstKind::Constant(WxConstant::Int(2))
            ))
    );
    assert!(
        !function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(
                instruction.kind,
                WxInstKind::RuntimeCall { pc: 0 | 1, .. }
            ))
    );
    verify(&function).unwrap();
}

#[test]
fn local_tuple_constant_projection_reuses_the_member_ssa_value() {
    // Given: a local tuple whose sole consumer projects a statically known member.
    let function = Function {
        register_count: 6,
        code: vec![
            Instruction::ConstSmallInt { dst: 2, value: 1 },
            Instruction::BuildTuple {
                dst: 3,
                items: vec![0, 1],
            },
            Instruction::GetItem {
                dst: 4,
                object: 3,
                key: 2,
            },
            Instruction::ConstBool {
                dst: 5,
                value: true,
            },
            Instruction::Branch {
                cond: 5,
                yes: 5,
                no: 6,
            },
            Instruction::Jump { target: 0 },
            Instruction::Return { src: 4 },
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
    let region_id = builder.begin_region(0, slots.clone());
    builder
        .finish_region(
            region_id,
            RegionKind::Loop { backedge: 5 },
            vec![RegionExit { target: 6 }],
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
    let plan = JitPlan {
        region_id: RegionId(0),
        header: 0,
        backedge: 5,
        exits: vec![RegionExit { target: 6 }],
        live_slots: slots,
        blocks: region.blocks.clone(),
        summary: region.summary,
    };

    // When: tuple construction and indexing are lowered together.
    let function = build_region(&executable, &plan).unwrap();

    // Then: neither bytecode operation dispatches and the projected value remains scalar.
    assert!(
        !function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(
                instruction.kind,
                WxInstKind::RuntimeCall { pc: 1 | 2, .. }
            ))
    );
    verify(&function).unwrap();
}

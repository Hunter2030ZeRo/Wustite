use wustite::bytecode::{BinaryOperator, BooleanOperator, Function, Instruction, UnaryOperator};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::planner::JitPlan;
use wustite::structure_map::{
    Fact, OperationSiteId, RegionExit, RegionId, RegionKind, SlotType, StateSlot,
    StructureMapBuilder,
};
use wustite::wxir::{
    WxBinaryOp, WxCastOp, WxFunction, WxInstKind, WxIntBinaryOp, WxIntOverflowOp, build_region,
};

fn build_loop(
    code: Vec<Instruction>,
    slots: Vec<StateSlot>,
    operation: Option<(SlotType, SlotType, SlotType)>,
    backedge: usize,
    exit: usize,
) -> WxFunction {
    let function = Function {
        register_count: 12,
        code,
    };
    let mut builder = StructureMapBuilder::new();
    if let Some((lhs, rhs, result)) = operation {
        builder
            .record_operation(
                0,
                Fact::Proven(lhs),
                Fact::Proven(rhs),
                Fact::Proven(result),
            )
            .unwrap();
    }
    let parameters = slots
        .iter()
        .enumerate()
        .map(|(index, slot)| ExecutableParameter {
            name: format!("arg{index}"),
            register: slot.register,
            ty: slot.ty,
        })
        .collect();
    let region = builder.begin_region(0, slots);
    builder
        .finish_region(
            region,
            RegionKind::Loop { backedge },
            vec![RegionExit { target: exit }],
        )
        .unwrap();
    let structure_map = builder
        .finish(&function.code, function.register_count)
        .unwrap();
    let executable = ExecutableFunction::new_with_parameters(function, structure_map, parameters);
    let region = executable.structure_map().region(RegionId(0)).unwrap();
    let plan = JitPlan {
        region_id: RegionId(0),
        header: 0,
        backedge,
        exits: vec![RegionExit { target: exit }],
        live_slots: region.entry_summary.clone(),
        blocks: region.blocks.clone(),
        summary: region.summary,
    };
    build_region(&executable, &plan).unwrap()
}

#[test]
fn boolean_and_unary_scalar_operations_lower_natively() {
    // Given: guarded loop inputs with Bool, SmallInt, and Float representations.
    let function = build_loop(
        vec![
            Instruction::BooleanOp {
                dst: 4,
                op: BooleanOperator::And,
                lhs: 0,
                rhs: 1,
            },
            Instruction::BooleanOp {
                dst: 5,
                op: BooleanOperator::Or,
                lhs: 4,
                rhs: 1,
            },
            Instruction::UnaryOp {
                dst: 6,
                op: UnaryOperator::Not,
                src: 5,
            },
            Instruction::UnaryOp {
                dst: 7,
                op: UnaryOperator::Negate,
                src: 2,
            },
            Instruction::Move { dst: 2, src: 7 },
            Instruction::UnaryOp {
                dst: 8,
                op: UnaryOperator::Negate,
                src: 3,
            },
            Instruction::Move { dst: 3, src: 8 },
            Instruction::Branch {
                cond: 6,
                yes: 8,
                no: 9,
            },
            Instruction::Jump { target: 0 },
            Instruction::Return { src: 2 },
        ],
        vec![
            StateSlot {
                register: 0,
                ty: SlotType::Bool,
            },
            StateSlot {
                register: 1,
                ty: SlotType::Bool,
            },
            StateSlot {
                register: 2,
                ty: SlotType::SmallInt,
            },
            StateSlot {
                register: 3,
                ty: SlotType::Float,
            },
        ],
        None,
        8,
        9,
    );

    // When: scalar boolean and unary operations are lowered.
    let instructions = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();

    // Then: each operation is direct and integer negation retains a replay exit.
    assert!(instructions.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::Binary {
            op: WxBinaryOp::Integer(WxIntBinaryOp::And),
            ..
        }
    )));
    assert!(instructions.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::Binary {
            op: WxBinaryOp::Integer(WxIntBinaryOp::Or),
            ..
        }
    )));
    assert!(instructions.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::IntegerBinaryWithOverflow {
            op: WxIntOverflowOp::Sub,
            ..
        }
    )));
    assert!(
        !instructions.iter().any(|instruction| matches!(
            instruction.kind,
            WxInstKind::RuntimeCall { pc: 0..=4, .. }
        ))
    );
    assert!(
        function
            .side_exits
            .iter()
            .any(|side_exit| side_exit.resume_pc == 3)
    );
}

#[test]
fn mixed_smallint_float_arithmetic_inserts_a_signed_cast() {
    // Given: a proven SmallInt plus Float operation followed by Float negation.
    let function = build_loop(
        vec![
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Add,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::UnaryOp {
                dst: 3,
                op: UnaryOperator::Negate,
                src: 2,
            },
            Instruction::Move { dst: 1, src: 3 },
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
            Instruction::Return { src: 1 },
        ],
        vec![
            StateSlot {
                register: 0,
                ty: SlotType::SmallInt,
            },
            StateSlot {
                register: 1,
                ty: SlotType::Float,
            },
        ],
        Some((SlotType::SmallInt, SlotType::Float, SlotType::Float)),
        5,
        6,
    );

    // When: the mixed operation is lowered.
    let instructions = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();

    // Then: signed conversion and Float operations replace both runtime calls.
    assert!(instructions.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::Cast {
            op: WxCastOp::IntToFloat { signed: true },
            ..
        }
    )));
    assert!(
        !instructions.iter().any(|instruction| matches!(
            instruction.kind,
            WxInstKind::RuntimeCall { pc: 0 | 1, .. }
        ))
    );
}

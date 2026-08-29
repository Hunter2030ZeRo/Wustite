use wustite::bytecode::{BinaryOperator, CompareOperator, Function, Instruction};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::planner::JitPlan;
use wustite::structure_map::{
    Fact, OperationSiteId, RegionExit, RegionId, RegionKind, SlotType, StateSlot,
    StructureMapBuilder, TypeFact,
};
use wustite::wxir::{
    WxBinaryOp, WxCompareOp, WxFunction, WxInstKind, WxIntBinaryOp, WxIntCompareOp,
    WxIntOverflowOp, build_region,
};

fn build_loop(
    code: Vec<Instruction>,
    slots: Vec<StateSlot>,
    sites: Vec<(usize, TypeFact, TypeFact, TypeFact)>,
    backedge: usize,
    exit: usize,
) -> WxFunction {
    let function = Function {
        register_count: 16,
        code,
    };
    let mut builder = StructureMapBuilder::new();
    for (pc, lhs, rhs, result) in sites {
        builder.record_operation(pc, lhs, rhs, result).unwrap();
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

fn proven(ty: SlotType) -> TypeFact {
    Fact::Proven(ty)
}

#[test]
fn checked_integer_arithmetic_and_floor_divide_lower_natively() {
    // Given: a loop whose generic arithmetic sites are proven SmallInt operations.
    let small = proven(SlotType::SmallInt);
    let boolean = proven(SlotType::Bool);
    let function = build_loop(
        vec![
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Subtract,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::BinaryOp {
                dst: 3,
                op: BinaryOperator::Multiply,
                lhs: 2,
                rhs: 1,
                site: OperationSiteId(1),
            },
            Instruction::BinaryOp {
                dst: 4,
                op: BinaryOperator::FloorDivide,
                lhs: 3,
                rhs: 1,
                site: OperationSiteId(2),
            },
            Instruction::CompareOp {
                dst: 5,
                op: CompareOperator::Lt,
                lhs: 4,
                rhs: 0,
                site: OperationSiteId(3),
            },
            Instruction::Branch {
                cond: 5,
                yes: 5,
                no: 6,
            },
            Instruction::Jump { target: 0 },
            Instruction::Return { src: 4 },
        ],
        vec![
            StateSlot {
                register: 0,
                ty: SlotType::SmallInt,
            },
            StateSlot {
                register: 1,
                ty: SlotType::SmallInt,
            },
        ],
        vec![
            (0, small, small, small),
            (1, small, small, small),
            (2, small, small, small),
            (3, small, small, boolean),
        ],
        5,
        6,
    );

    // When: the region is lowered to WXIR.
    let instructions = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .collect::<Vec<_>>();

    // Then: checked Sub/Mul and guarded floor division avoid generic runtime calls.
    assert!(instructions.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::IntegerBinaryWithOverflow {
            op: WxIntOverflowOp::Sub,
            ..
        }
    )));
    assert!(instructions.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::IntegerBinaryWithOverflow {
            op: WxIntOverflowOp::Mul,
            ..
        }
    )));
    assert!(instructions.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::Binary {
            op: WxBinaryOp::Integer(WxIntBinaryOp::FloorDiv),
            ..
        }
    )));
    assert!(
        !instructions.iter().any(|instruction| matches!(
            instruction.kind,
            WxInstKind::RuntimeCall { pc: 0..=2, .. }
        ))
    );
    assert!([0, 1, 2].into_iter().all(|pc| {
        function
            .side_exits
            .iter()
            .any(|side_exit| side_exit.resume_pc == pc)
    }));
}

#[test]
fn integer_comparisons_lower_every_source_operator() {
    // Given: proven SmallInt comparison sites for all source operators.
    let small = proven(SlotType::SmallInt);
    let boolean = proven(SlotType::Bool);
    let operators = [
        CompareOperator::Eq,
        CompareOperator::NotEq,
        CompareOperator::Lt,
        CompareOperator::Le,
        CompareOperator::Gt,
        CompareOperator::Ge,
    ];
    let mut code = operators
        .iter()
        .enumerate()
        .map(|(pc, op)| Instruction::CompareOp {
            dst: u16::try_from(pc + 2).unwrap(),
            op: *op,
            lhs: 0,
            rhs: 1,
            site: OperationSiteId(u32::try_from(pc).unwrap()),
        })
        .collect::<Vec<_>>();
    for (dst, lhs, rhs) in [(8, 2, 3), (9, 8, 4), (10, 9, 5), (11, 10, 6), (12, 11, 7)] {
        code.push(Instruction::BooleanOp {
            dst,
            op: wustite::bytecode::BooleanOperator::And,
            lhs,
            rhs,
        });
    }
    code.push(Instruction::Branch {
        cond: 12,
        yes: 12,
        no: 13,
    });
    code.push(Instruction::Jump { target: 0 });
    code.push(Instruction::Return { src: 12 });
    let function = build_loop(
        code,
        vec![
            StateSlot {
                register: 0,
                ty: SlotType::SmallInt,
            },
            StateSlot {
                register: 1,
                ty: SlotType::SmallInt,
            },
        ],
        (0..operators.len())
            .map(|pc| (pc, small, small, boolean))
            .collect(),
        12,
        13,
    );

    // When: all comparison instructions are inspected.
    let comparisons = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.kind, WxInstKind::Compare { .. }))
        .collect::<Vec<_>>();

    // Then: no proven integer comparison requires a runtime call.
    assert_eq!(comparisons.len(), operators.len());
    assert!(comparisons.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::Compare {
            op: WxCompareOp::Integer(WxIntCompareOp::Eq),
            ..
        }
    )));
    assert!(comparisons.iter().any(|instruction| matches!(
        instruction.kind,
        WxInstKind::Compare {
            op: WxCompareOp::Integer(WxIntCompareOp::Ne),
            ..
        }
    )));
    assert!(
        !function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(
                instruction.kind,
                WxInstKind::RuntimeCall { pc: 0..=5, .. }
            ))
    );
}

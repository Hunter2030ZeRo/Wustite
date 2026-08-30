use crate::bytecode::{BinaryOperator, CompareOperator, Function, Instruction};
use crate::executable::ExecutableFunction;
use crate::structure_map::{OperationSiteId, SlotType, StructureMapBuilder, TypeFact};

use super::super::QuickCode;

fn exact(ty: SlotType) -> TypeFact {
    TypeFact::Proven(ty)
}

fn executable(
    code: Vec<Instruction>,
    sites: Vec<(usize, TypeFact, TypeFact, TypeFact)>,
) -> ExecutableFunction {
    let function = Function {
        register_count: 4,
        code,
    };
    let mut builder = StructureMapBuilder::new();
    for (pc, lhs, rhs, result) in sites {
        builder
            .record_operation(pc, lhs, rhs, result)
            .expect("operation site fixture should be representable");
    }
    let structure_map = builder
        .finish(&function.code, function.register_count)
        .expect("structure map fixture should be representable");
    ExecutableFunction::new(function, structure_map)
}

fn site(
    pc: usize,
    lhs: TypeFact,
    rhs: TypeFact,
    result: TypeFact,
) -> (usize, TypeFact, TypeFact, TypeFact) {
    (pc, lhs, rhs, result)
}

#[test]
fn quick_code_builds_only_exact_add_lt() {
    let small = exact(SlotType::SmallInt);
    let boolean = exact(SlotType::Bool);
    let executable = executable(
        vec![
            Instruction::ConstSmallInt { dst: 0, value: 1 },
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Add,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::CompareOp {
                dst: 3,
                op: CompareOperator::Lt,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(1),
            },
            Instruction::Return { src: 2 },
        ],
        vec![site(1, small, small, small), site(2, small, small, boolean)],
    );

    let quick = QuickCode::new(&executable);

    assert_eq!(quick.len(), executable.bytecode().code.len());
    assert_eq!(quick.get(0), None);
    assert_eq!(
        quick.get(1),
        Some(super::super::QuickInstruction::Add {
            dst: 2,
            lhs: 0,
            rhs: 1
        })
    );
    assert_eq!(
        quick.get(2),
        Some(super::super::QuickInstruction::Compare {
            dst: 3,
            lhs: 0,
            rhs: 1,
            op: CompareOperator::Lt,
        })
    );
    assert_eq!(quick.get(3), None);
}

#[test]
fn quick_code_keeps_unknown_mismatched_unsupported_sites() {
    let small = exact(SlotType::SmallInt);
    let float = exact(SlotType::Float);
    let cases = vec![
        (BinaryOperator::Add, TypeFact::Unknown, small, small, 0),
        (BinaryOperator::Add, small, TypeFact::Unknown, small, 0),
        (BinaryOperator::Add, small, small, TypeFact::Unknown, 0),
        (BinaryOperator::Add, small, small, float, 0),
        (BinaryOperator::Divide, small, small, small, 0),
        (BinaryOperator::FloorDivide, small, small, small, 0),
        (BinaryOperator::Power, small, small, small, 0),
        (BinaryOperator::Add, small, small, small, 1),
    ];
    for (op, lhs_fact, rhs_fact, result_fact, fact_pc) in cases {
        let executable = executable(
            vec![Instruction::BinaryOp {
                dst: 2,
                op,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            }],
            vec![site(fact_pc, lhs_fact, rhs_fact, result_fact)],
        );
        assert_eq!(QuickCode::new(&executable).get(0), None);
    }

    for instruction in [
        Instruction::AddI64 {
            dst: 2,
            lhs: 0,
            rhs: 1,
        },
        Instruction::LtI64 {
            dst: 2,
            lhs: 0,
            rhs: 1,
        },
    ] {
        let executable = executable(vec![instruction], Vec::new());
        assert_eq!(QuickCode::new(&executable).get(0), None);
    }
}

#[test]
fn quick_code_guards_without_structure_facts() {
    let small = exact(SlotType::SmallInt);
    let boolean = exact(SlotType::Bool);
    let executable = executable(
        vec![
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Subtract,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Multiply,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(1),
            },
            Instruction::CompareOp {
                dst: 3,
                op: CompareOperator::Ge,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(2),
            },
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Divide,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(3),
            },
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::FloorDivide,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(4),
            },
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Power,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(5),
            },
        ],
        vec![
            site(99, TypeFact::Unknown, small, small),
            site(99, small, TypeFact::Unknown, small),
            site(99, small, small, boolean),
        ],
    );

    let quick = QuickCode::new_interpreter(executable.bytecode());

    assert_eq!(
        quick.get(0),
        Some(super::super::QuickInstruction::Subtract {
            dst: 2,
            lhs: 0,
            rhs: 1,
        })
    );
    assert_eq!(
        quick.get(1),
        Some(super::super::QuickInstruction::Multiply {
            dst: 2,
            lhs: 0,
            rhs: 1,
        })
    );
    assert_eq!(
        quick.get(2),
        Some(super::super::QuickInstruction::Compare {
            dst: 3,
            lhs: 0,
            rhs: 1,
            op: CompareOperator::Ge,
        })
    );
    assert_eq!(
        quick.get(3),
        Some(super::super::QuickInstruction::Divide {
            dst: 2,
            lhs: 0,
            rhs: 1,
        })
    );
    assert_eq!(
        quick.get(4),
        Some(super::super::QuickInstruction::FloorDivide {
            dst: 2,
            lhs: 0,
            rhs: 1,
        })
    );
    assert_eq!(
        quick.get(5),
        Some(super::super::QuickInstruction::Power {
            dst: 2,
            lhs: 0,
            rhs: 1,
        })
    );
}

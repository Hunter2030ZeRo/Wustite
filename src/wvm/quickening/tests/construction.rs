use crate::bytecode::{BinaryOperator, CompareOperator, Function, Instruction};
use crate::executable::ExecutableFunction;
use crate::structure_map::{OperationSite, OperationSiteId, SlotType, StructureMap, TypeFact};

use super::super::QuickCode;

fn exact(ty: SlotType) -> TypeFact {
    TypeFact::Exact(ty)
}

fn executable(code: Vec<Instruction>, sites: Vec<OperationSite>) -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
            register_count: 4,
            code,
        },
        StructureMap {
            loops: Vec::new(),
            operation_sites: sites,
        },
    )
}

fn site(pc: usize, lhs: TypeFact, rhs: TypeFact, result: TypeFact) -> OperationSite {
    OperationSite {
        pc,
        lhs,
        rhs,
        result,
    }
}

#[test]
fn quick_code_builds_only_exact_add_and_lt() {
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
        Some(super::super::QuickInstruction::Lt {
            dst: 3,
            lhs: 0,
            rhs: 1
        })
    );
    assert_eq!(quick.get(3), None);
}

#[test]
fn quick_code_preserves_unknown_mismatched_and_unsupported_sites() {
    let small = exact(SlotType::SmallInt);
    let boolean = exact(SlotType::Bool);
    let float = exact(SlotType::Float);
    let cases = vec![
        (BinaryOperator::Add, TypeFact::Unknown, small, small, 0),
        (BinaryOperator::Add, small, TypeFact::Unknown, small, 0),
        (BinaryOperator::Add, small, small, TypeFact::Unknown, 0),
        (BinaryOperator::Add, small, small, float, 0),
        (BinaryOperator::Subtract, small, small, small, 0),
        (BinaryOperator::Multiply, small, small, small, 0),
        (BinaryOperator::Divide, small, small, small, 0),
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

    for op in [
        CompareOperator::Eq,
        CompareOperator::NotEq,
        CompareOperator::Le,
        CompareOperator::Gt,
        CompareOperator::Ge,
    ] {
        let executable = executable(
            vec![Instruction::CompareOp {
                dst: 2,
                op,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            }],
            vec![site(0, small, small, boolean)],
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

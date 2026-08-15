use std::sync::Arc;

use crate::bytecode::{BinaryOperator, Function, Instruction};
use crate::executable::ExecutableFunction;
use crate::structure_map::{OperationSite, OperationSiteId, SlotType, StructureMap, TypeFact};

use super::{FunctionRuntime, Vm};

fn exact_add(site_pc: usize) -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
            register_count: 3,
            code: vec![
                Instruction::ConstSmallInt { dst: 0, value: 1 },
                Instruction::ConstSmallInt { dst: 1, value: 2 },
                Instruction::BinaryOp {
                    dst: 2,
                    op: BinaryOperator::Add,
                    lhs: 0,
                    rhs: 1,
                    site: OperationSiteId(0),
                },
                Instruction::Return { src: 2 },
            ],
        },
        StructureMap {
            regions: Vec::new(),
            operation_sites: vec![OperationSite {
                pc: site_pc,
                lhs: TypeFact::Exact(SlotType::SmallInt),
                rhs: TypeFact::Exact(SlotType::SmallInt),
                result: TypeFact::Exact(SlotType::SmallInt),
            }],
        },
    )
}

#[test]
fn quick_code_runtime_identity() {
    let first = exact_add(2);
    let revision = exact_add(2);
    let active = FunctionRuntime::new(&first);
    let recursive = FunctionRuntime::recursive_placeholder(&first, Arc::clone(&active.quick_code));
    let independent = FunctionRuntime::new(&revision);

    assert!(Arc::ptr_eq(&active.quick_code, &recursive.quick_code));
    assert!(!Arc::ptr_eq(&active.quick_code, &independent.quick_code));
}

#[test]
fn invalid_executable_builds_no_quick_runtime() {
    let invalid = exact_add(1);
    let mut vm = Vm::new();

    assert!(vm.execute(&invalid).is_err());
    assert!(vm.profile_for(&invalid).is_none());
}

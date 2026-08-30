use crate::bytecode::{Function, Instruction};
use crate::executable::ExecutableFunction;
use crate::structure_map::StructureMap;

use super::{full_verification_count, reset_full_verification_count, verify};

fn valid_executable() -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
            code: vec![
                Instruction::ConstSmallInt { dst: 0, value: 42 },
                Instruction::Return { src: 0 },
            ],
            register_count: 1,
        },
        StructureMap::default(),
    )
}

#[test]
fn full_verification_shared_by_calls_and_clones() {
    let executable = valid_executable();
    let clone_before_verification = executable.clone();
    reset_full_verification_count();

    assert_eq!(verify(&executable), Ok(()));
    assert_eq!(verify(&executable), Ok(()));
    assert_eq!(verify(&clone_before_verification), Ok(()));

    assert_eq!(full_verification_count(), 1);
}

#[test]
fn failed_verification_shared_by_calls_and_clones() {
    let executable = ExecutableFunction::new(
        Function {
            code: vec![Instruction::Return { src: 0 }],
            register_count: 0,
        },
        StructureMap::default(),
    );
    let clone_before_verification = executable.clone();
    reset_full_verification_count();

    let first = verify(&executable);
    assert!(first.is_err());
    assert_eq!(verify(&executable), first);
    assert_eq!(verify(&clone_before_verification), first);

    assert_eq!(full_verification_count(), 1);
}

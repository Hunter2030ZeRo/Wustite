use wustite::bytecode::{Function, Instruction};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::structure_map::{SlotType, StructureMap};
use wustite::verifier::{self, MAX_REGISTER_COUNT};

fn executable(register_count: usize, code: Vec<Instruction>) -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
            code,
            register_count,
        },
        StructureMap::default(),
    )
}

#[test]
fn verifier_rejects_return_from_unwritten_register() {
    // Given: bytecode whose return register has never been written.
    let function = executable(1, vec![Instruction::Return { src: 0 }]);

    // When: the executable is verified.
    let error = verifier::verify(&function).unwrap_err();

    // Then: the unwritten read is rejected before execution.
    assert!(error.contains("instruction 0 Return src reads uninitialized register r0"));
}

#[test]
fn verifier_rejects_build_list_item_from_unwritten_register() {
    // Given: a list construction that reads an unwritten item register.
    let function = executable(
        2,
        vec![
            Instruction::BuildList {
                dst: 0,
                items: vec![1],
            },
            Instruction::Return { src: 0 },
        ],
    );

    // When: the executable is verified.
    let error = verifier::verify(&function).unwrap_err();

    // Then: collection inputs must be definitely assigned.
    assert!(error.contains("instruction 0 collection item reads uninitialized register r1"));
}

#[test]
fn verifier_intersects_assignments_at_branch_join() {
    // Given: r1 is written on only one path to a shared return.
    let function = ExecutableFunction::new_with_parameters(
        Function {
            register_count: 2,
            code: vec![
                Instruction::Branch {
                    cond: 0,
                    yes: 1,
                    no: 3,
                },
                Instruction::ConstSmallInt { dst: 1, value: 7 },
                Instruction::Jump { target: 4 },
                Instruction::Jump { target: 4 },
                Instruction::Return { src: 1 },
            ],
        },
        StructureMap::default(),
        vec![ExecutableParameter {
            name: "condition".to_owned(),
            register: 0,
            ty: SlotType::Bool,
        }],
    );

    // When: the executable is verified.
    let error = verifier::verify(&function).unwrap_err();

    // Then: a write on only one predecessor is not definitely assigned.
    assert!(error.contains("instruction 4 Return src reads uninitialized register r1"));
}

#[test]
fn verifier_treats_parameter_registers_as_initially_assigned() {
    // Given: a function that directly returns its sole parameter.
    let function = ExecutableFunction::new_with_parameters(
        Function {
            register_count: 1,
            code: vec![Instruction::Return { src: 0 }],
        },
        StructureMap::default(),
        vec![ExecutableParameter {
            name: "value".to_owned(),
            register: 0,
            ty: SlotType::Any,
        }],
    );

    // When: the executable is verified.
    let result = verifier::verify(&function);

    // Then: the ABI write at function entry satisfies the return read.
    assert_eq!(result, Ok(()));
}

#[test]
fn verifier_intersects_loop_entry_with_backedge_assignments() {
    // Given: r1 is written on the backedge but not on the loop's first entry.
    let function = ExecutableFunction::new_with_parameters(
        Function {
            register_count: 3,
            code: vec![
                Instruction::Jump { target: 1 },
                Instruction::Move { dst: 2, src: 1 },
                Instruction::ConstSmallInt { dst: 1, value: 1 },
                Instruction::Branch {
                    cond: 0,
                    yes: 1,
                    no: 4,
                },
                Instruction::Return { src: 2 },
            ],
        },
        StructureMap::default(),
        vec![ExecutableParameter {
            name: "repeat".to_owned(),
            register: 0,
            ty: SlotType::Bool,
        }],
    );

    // When: the executable is verified.
    let error = verifier::verify(&function).unwrap_err();

    // Then: a backedge-only write cannot initialize the first loop iteration.
    assert!(error.contains("instruction 1 Move src reads uninitialized register r1"));
}

#[test]
fn verifier_rejects_reachable_fallthrough_without_return() {
    // Given: a reachable final instruction that is not a terminator.
    let function = executable(1, vec![Instruction::ConstSmallInt { dst: 0, value: 1 }]);

    // When: the executable is verified.
    let error = verifier::verify(&function).unwrap_err();

    // Then: execution cannot fall beyond the bytecode buffer.
    assert!(error.contains("falls off"));
    assert!(error.contains("without Return"));
}

#[test]
fn verifier_rejects_excessive_register_count() {
    // Given: malformed bytecode requesting more registers than the VM permits.
    let function = executable(
        MAX_REGISTER_COUNT + 1,
        vec![
            Instruction::ConstSmallInt { dst: 0, value: 1 },
            Instruction::Return { src: 0 },
        ],
    );

    // When: the executable is verified.
    let error = verifier::verify(&function).unwrap_err();

    // Then: verification rejects it before the VM allocates the register frame.
    assert!(error.contains("register_count"));
    assert!(error.contains("exceeds maximum"));
}

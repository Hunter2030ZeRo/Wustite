use wustite::{bytecode, executable, planner, structure_map, value, verifier, wvm};

fn sum_function() -> executable::ExecutableFunction {
    executable::ExecutableFunction {
        bytecode: bytecode::Function {
            register_count: 5,
            code: vec![
                bytecode::Instruction::ConstI64 { dst: 0, value: 0 },
                bytecode::Instruction::ConstI64 { dst: 1, value: 1 },
                bytecode::Instruction::ConstI64 { dst: 2, value: 1 },
                bytecode::Instruction::ConstI64 { dst: 3, value: 101 },
                bytecode::Instruction::LtI64 {
                    dst: 4,
                    lhs: 1,
                    rhs: 3,
                },
                bytecode::Instruction::Branch {
                    cond: 4,
                    yes: 6,
                    no: 9,
                },
                bytecode::Instruction::AddI64 {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                },
                bytecode::Instruction::AddI64 {
                    dst: 1,
                    lhs: 1,
                    rhs: 2,
                },
                bytecode::Instruction::Jump { target: 4 },
                bytecode::Instruction::Return { src: 0 },
            ],
        },
        structure_map: structure_map::StructureMap {
            loops: vec![structure_map::LoopRegion {
                header: 4,
                backedge: 8,
                exits: vec![structure_map::RegionExit { target: 9 }],
                live_slots: vec![
                    structure_map::LiveSlot {
                        register: 0,
                        ty: structure_map::SlotType::I64,
                    },
                    structure_map::LiveSlot {
                        register: 1,
                        ty: structure_map::SlotType::I64,
                    },
                    structure_map::LiveSlot {
                        register: 2,
                        ty: structure_map::SlotType::I64,
                    },
                    structure_map::LiveSlot {
                        register: 3,
                        ty: structure_map::SlotType::I64,
                    },
                ],
            }],
        },
    }
}

#[test]
fn sum_one_to_one_hundred() {
    let function = sum_function();

    let mut vm = wvm::Vm::new();
    let result = vm.execute(&function).unwrap();
    let profile = vm.profile().unwrap();

    let hot_loop = function
        .structure_map
        .loops
        .iter()
        .find(|region| profile.is_hot(region.header, 50))
        .unwrap();

    let plan = planner::select_hot_loop(&function.structure_map, profile, 50).unwrap();

    assert_eq!(plan.header, 4);
    assert_eq!(plan.backedge, 8);
    assert_eq!(plan.exits, vec![structure_map::RegionExit { target: 9 }]);
    assert_eq!(plan.live_slots, function.structure_map.loops[0].live_slots);
    assert_eq!(hot_loop.header, 4);
    assert_eq!(profile.count(hot_loop.header), 101);
    assert_eq!(result.value, value::Value::I64(5050));
    assert_eq!(profile.count(4), 101);
}

#[test]
fn verifier_rejects_invalid_register() {
    let function = executable::ExecutableFunction {
        bytecode: bytecode::Function {
            register_count: 1,
            code: vec![
                bytecode::Instruction::ConstI64 { dst: 1, value: 0 },
                bytecode::Instruction::Return { src: 0 },
            ],
        },
        structure_map: structure_map::StructureMap::default(),
    };

    assert!(verifier::verify(&function).is_err());

    let mut vm = wvm::Vm::new();
    assert!(vm.execute(&function).is_err());
    assert!(vm.profile().is_none());
}

#[test]
fn verifier_rejects_invalid_jump_target() {
    let function = executable::ExecutableFunction {
        bytecode: bytecode::Function {
            register_count: 0,
            code: vec![bytecode::Instruction::Jump { target: 1 }],
        },
        structure_map: structure_map::StructureMap::default(),
    };

    assert!(verifier::verify(&function).is_err());
}

#[test]
fn verifier_rejects_invalid_loop_metadata() {
    let function = executable::ExecutableFunction {
        bytecode: bytecode::Function {
            register_count: 1,
            code: vec![
                bytecode::Instruction::Jump { target: 1 },
                bytecode::Instruction::Return { src: 0 },
            ],
        },
        structure_map: structure_map::StructureMap {
            loops: vec![structure_map::LoopRegion {
                header: 0,
                backedge: 0,
                exits: vec![structure_map::RegionExit { target: 1 }],
                live_slots: vec![],
            }],
        },
    };

    assert!(verifier::verify(&function).is_err());
}

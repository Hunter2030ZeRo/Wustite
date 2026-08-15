use wustite::{bytecode, executable, planner, structure_map, value, verifier, wvm};

fn sum_function() -> executable::ExecutableFunction {
    executable::ExecutableFunction::new(
        bytecode::Function {
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
        structure_map::StructureMap {
            regions: vec![structure_map::Region {
                kind: structure_map::RegionKind::Loop,
                entry: 4,
                backedge: Some(8),
                exits: vec![structure_map::RegionExit { target: 9 }],
                live_slots: vec![
                    structure_map::LiveSlot {
                        register: 0,
                        ty: structure_map::SlotType::SmallInt,
                    },
                    structure_map::LiveSlot {
                        register: 1,
                        ty: structure_map::SlotType::SmallInt,
                    },
                    structure_map::LiveSlot {
                        register: 2,
                        ty: structure_map::SlotType::SmallInt,
                    },
                    structure_map::LiveSlot {
                        register: 3,
                        ty: structure_map::SlotType::SmallInt,
                    },
                ],
            }],
            operation_sites: vec![],
        },
    )
}

#[test]
fn sum_one_to_one_hundred() {
    let function = sum_function();

    let mut vm = wvm::Vm::new();
    let result = vm.execute(&function).unwrap();
    let profile = vm.profile().unwrap();

    let hot_loop = function
        .structure_map()
        .regions
        .iter()
        .enumerate()
        .find(|(index, _)| profile.is_hot(structure_map::RegionId(*index), 50))
        .map(|(_, region)| region)
        .unwrap();

    let plan = planner::select_hot_loop(function.structure_map(), profile, 50).unwrap();

    assert_eq!(plan.header, 4);
    assert_eq!(plan.backedge, 8);
    assert_eq!(plan.exits, vec![structure_map::RegionExit { target: 9 }]);
    assert_eq!(
        plan.live_slots,
        function.structure_map().regions[0].live_slots
    );
    assert_eq!(plan.region_id, structure_map::RegionId(0));
    assert_eq!(hot_loop.entry, 4);
    assert_eq!(hot_loop.backedge, Some(8));
    assert_eq!(profile.entry_count(structure_map::RegionId(0)), 101);
    assert_eq!(result.value, value::Value::SmallInt(5050));
    assert_eq!(profile.entry_count(structure_map::RegionId(0)), 101);
}

#[test]
fn executable_revisions_have_independent_identity_and_runtime_state() {
    let function = sum_function();
    let mut revised_bytecode = function.bytecode().clone();
    revised_bytecode.code[6] = bytecode::Instruction::Move { dst: 0, src: 1 };
    let revised =
        executable::ExecutableFunction::new(revised_bytecode, function.structure_map().clone());
    let mut vm = wvm::Vm::with_hot_threshold(10);

    assert_ne!(function.id(), revised.id());

    assert_eq!(
        vm.execute(&function).unwrap().value,
        value::Value::SmallInt(5050)
    );
    assert_eq!(vm.jit_report().compiled_regions, 1);

    assert_eq!(
        vm.execute(&revised).unwrap().value,
        value::Value::SmallInt(100)
    );
    assert_eq!(vm.jit_report().compilation_attempts, 1);
    assert_eq!(vm.jit_report().compiled_regions, 1);
    assert_eq!(
        vm.profile_for(&function)
            .unwrap()
            .entry_count(structure_map::RegionId(0)),
        10
    );
    assert_eq!(
        vm.profile_for(&revised)
            .unwrap()
            .entry_count(structure_map::RegionId(0)),
        10
    );
}

#[test]
fn a_b_a_reuses_each_executables_compiled_runtime() {
    let a = sum_function();
    let b = sum_function();
    let mut vm = wvm::Vm::with_hot_threshold(10);

    vm.execute(&a).unwrap();
    assert_eq!(vm.jit_report().compiled_regions, 1);
    vm.execute(&b).unwrap();
    assert_eq!(vm.jit_report().compiled_regions, 1);
    assert!(vm.profile_for(&a).is_some());
    assert!(vm.profile_for(&b).is_some());

    vm.execute(&a).unwrap();
    assert_eq!(vm.jit_report().compilation_attempts, 0);
    assert_eq!(vm.jit_report().native_executions, 1);
}

#[test]
fn clone_preserves_identity_and_reuses_runtime() {
    let original = sum_function();
    let cloned = original.clone();
    let mut vm = wvm::Vm::with_hot_threshold(10);

    assert_eq!(original.id(), cloned.id());
    vm.execute(&original).unwrap();
    vm.execute(&cloned).unwrap();
    assert_eq!(vm.jit_report().compilation_attempts, 0);
    assert_eq!(vm.jit_report().native_executions, 1);
}

#[test]
fn invalid_executable_does_not_disturb_a_cached_runtime() {
    let valid = sum_function();
    let invalid = executable::ExecutableFunction::new(
        bytecode::Function {
            register_count: 0,
            code: vec![bytecode::Instruction::Return { src: 0 }],
        },
        structure_map::StructureMap::default(),
    );
    let mut vm = wvm::Vm::with_hot_threshold(10);

    vm.execute(&valid).unwrap();
    assert!(vm.execute(&invalid).is_err());
    assert!(vm.profile_for(&invalid).is_none());
    vm.execute(&valid).unwrap();
    assert_eq!(vm.jit_report().compilation_attempts, 0);
    assert_eq!(vm.jit_report().native_executions, 1);
}

#[test]
fn verifier_rejects_invalid_register() {
    let function = executable::ExecutableFunction::new(
        bytecode::Function {
            register_count: 1,
            code: vec![
                bytecode::Instruction::ConstI64 { dst: 1, value: 0 },
                bytecode::Instruction::Return { src: 0 },
            ],
        },
        structure_map::StructureMap::default(),
    );

    assert!(verifier::verify(&function).is_err());

    let mut vm = wvm::Vm::new();
    assert!(vm.execute(&function).is_err());
    assert!(vm.profile().is_none());
}

#[test]
fn verifier_rejects_invalid_jump_target() {
    let function = executable::ExecutableFunction::new(
        bytecode::Function {
            register_count: 0,
            code: vec![bytecode::Instruction::Jump { target: 1 }],
        },
        structure_map::StructureMap::default(),
    );

    assert!(verifier::verify(&function).is_err());
}

#[test]
fn verifier_rejects_invalid_loop_metadata() {
    let function = executable::ExecutableFunction::new(
        bytecode::Function {
            register_count: 1,
            code: vec![
                bytecode::Instruction::Jump { target: 1 },
                bytecode::Instruction::Return { src: 0 },
            ],
        },
        structure_map::StructureMap {
            regions: vec![structure_map::Region {
                kind: structure_map::RegionKind::Loop,
                entry: 0,
                backedge: Some(0),
                exits: vec![structure_map::RegionExit { target: 1 }],
                live_slots: vec![],
            }],
            operation_sites: vec![],
        },
    );

    assert!(verifier::verify(&function).is_err());
}

#[test]
fn verifier_rejects_duplicate_loop_headers() {
    let original = sum_function();
    let mut structure_map = original.structure_map().clone();
    structure_map.regions.push(structure_map.regions[0].clone());
    let function = executable::ExecutableFunction::new(original.bytecode().clone(), structure_map);

    assert!(
        verifier::verify(&function)
            .unwrap_err()
            .contains("duplicates loop header")
    );
}

#[test]
fn verifier_rejects_duplicate_exit_targets() {
    let original = sum_function();
    let mut structure_map = original.structure_map().clone();
    structure_map.regions[0]
        .exits
        .push(structure_map::RegionExit { target: 9 });
    let function = executable::ExecutableFunction::new(original.bytecode().clone(), structure_map);

    assert!(
        verifier::verify(&function)
            .unwrap_err()
            .contains("duplicate exit target")
    );
}

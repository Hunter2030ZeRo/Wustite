use wustite::{bytecode, planner, structure_map, value, wvm};

#[test]
fn sum_one_to_one_hundred() {
    let function = bytecode::Function {
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
    };

    let mut vm = wvm::Vm::new();
    let result = vm.execute(&function).unwrap();

    let structure_map = structure_map::StructureMap {
        loops: vec![structure_map::LoopRegion {
            header: 4,
            backedge: 8,
            exit: 9,
            live_registers: vec![0, 1, 2, 3],
        }],
    };

    let hot_loop = structure_map
        .loops
        .iter()
        .find(|region| result.profile.is_hot(region.header, 50))
        .unwrap();

    let plan = planner::select_hot_loop(&structure_map, &result.profile, 50).unwrap();

    assert_eq!(plan.header, 4);
    assert_eq!(plan.backedge, 8);
    assert_eq!(plan.exit, 9);
    assert_eq!(plan.live_registers, vec![0, 1, 2, 3]);
    assert_eq!(hot_loop.header, 4);
    assert_eq!(result.profile.count(hot_loop.header), 101);
    assert_eq!(result.value, value::Value::I64(5050));
    assert_eq!(result.profile.count(4), 101);
}

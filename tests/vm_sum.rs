use wustite::{bytecode, value, wvm};

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

    let mut vm = wvm::Vm;
    let result = vm.execute(&function).unwrap();
    assert_eq!(result, value::Value::I64(5050));
}

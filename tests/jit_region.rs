use wustite::bytecode::{Function, Instruction};
use wustite::executable::ExecutableFunction;
use wustite::jit::{CraneliftRegionCompiler, RegionCompiler};
use wustite::planner::{self, JitPlan};
use wustite::structure_map::{LiveSlot, LoopRegion, RegionExit, RegionId, SlotType, StructureMap};
use wustite::value::Value;
use wustite::wvm::Vm;
use wustite::wxir::build_region;

fn i64_slot(register: u16) -> LiveSlot {
    LiveSlot {
        register,
        ty: SlotType::I64,
    }
}

fn sum_function() -> ExecutableFunction {
    ExecutableFunction {
        bytecode: Function {
            register_count: 5,
            code: vec![
                Instruction::ConstI64 { dst: 0, value: 0 },
                Instruction::ConstI64 { dst: 1, value: 1 },
                Instruction::ConstI64 { dst: 2, value: 1 },
                Instruction::ConstI64 { dst: 3, value: 101 },
                Instruction::LtI64 {
                    dst: 4,
                    lhs: 1,
                    rhs: 3,
                },
                Instruction::Branch {
                    cond: 4,
                    yes: 6,
                    no: 9,
                },
                Instruction::AddI64 {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::AddI64 {
                    dst: 1,
                    lhs: 1,
                    rhs: 2,
                },
                Instruction::Jump { target: 4 },
                Instruction::Return { src: 0 },
            ],
        },
        structure_map: StructureMap {
            loops: vec![LoopRegion {
                header: 4,
                backedge: 8,
                exits: vec![RegionExit { target: 9 }],
                live_slots: (0..4).map(i64_slot).collect(),
            }],
        },
    }
}

#[test]
fn compiled_sum_region_restores_live_state_and_resume_pc() {
    let executable = sum_function();
    let mut vm = Vm::new();
    assert_eq!(vm.execute(&executable).unwrap().value, Value::I64(5050));
    let plan =
        planner::select_hot_loop(&executable.structure_map, vm.profile().unwrap(), 50).unwrap();
    let wxir = build_region(&executable, &plan).unwrap();
    let compiled = CraneliftRegionCompiler::new().compile(&wxir).unwrap();

    let mut registers = vec![Value::Uninitialized; 5];
    registers[0] = Value::I64(0);
    registers[1] = Value::I64(1);
    registers[2] = Value::I64(1);
    registers[3] = Value::I64(101);

    let exit = compiled.execute(&mut registers).unwrap();
    assert_eq!(exit.resume_pc, 9);
    assert_eq!(registers[0], Value::I64(5050));
    assert_eq!(registers[1], Value::I64(101));
}

#[test]
fn compiled_overflow_exits_before_updating_destination() {
    let live_slots = vec![i64_slot(0), i64_slot(1)];
    let executable = ExecutableFunction {
        bytecode: Function {
            register_count: 2,
            code: vec![
                Instruction::ConstI64 {
                    dst: 0,
                    value: i64::MAX,
                },
                Instruction::ConstI64 { dst: 1, value: 1 },
                Instruction::AddI64 {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Jump { target: 2 },
            ],
        },
        structure_map: StructureMap {
            loops: vec![LoopRegion {
                header: 2,
                backedge: 3,
                exits: vec![],
                live_slots: live_slots.clone(),
            }],
        },
    };
    let plan = JitPlan {
        region_id: RegionId(0),
        header: 2,
        backedge: 3,
        exits: vec![],
        live_slots,
    };
    let wxir = build_region(&executable, &plan).unwrap();
    let compiled = CraneliftRegionCompiler::new().compile(&wxir).unwrap();
    let mut registers = vec![Value::I64(i64::MAX), Value::I64(1)];

    let exit = compiled.execute(&mut registers).unwrap();
    assert_eq!(exit.resume_pc, 2);
    assert_eq!(registers[0], Value::I64(i64::MAX));
    assert_eq!(registers[1], Value::I64(1));

    let error = Vm::new().execute(&executable).err().unwrap();
    assert_eq!(error, "i64 addition overflow");
}

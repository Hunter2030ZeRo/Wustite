#![cfg(feature = "inkwell")]

use wustite::bytecode::{Function, Instruction};
use wustite::executable::ExecutableFunction;
use wustite::jit::{LlvmRegionCompiler, RegionCompiler};
use wustite::planner;
use wustite::structure_map::{LiveSlot, LoopRegion, RegionExit, SlotType, StructureMap};
use wustite::value::Value;
use wustite::wvm::Vm;
use wustite::wxir::{WxExitKind, build_region};

fn sum_function() -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
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
        StructureMap {
            loops: vec![LoopRegion {
                header: 4,
                backedge: 8,
                exits: vec![RegionExit { target: 9 }],
                live_slots: (0..4)
                    .map(|register| LiveSlot {
                        register,
                        ty: SlotType::SmallInt,
                    })
                    .collect(),
            }],
            operation_sites: Vec::new(),
        },
    )
}

#[test]
fn llvm_region_restores_wvm_state_when_wxir_exits() {
    // Given: verified loop WXIR and its WVM entry state.
    let executable = sum_function();
    let mut interpreter = Vm::with_hot_threshold(u64::MAX);
    interpreter.execute(&executable).unwrap();
    let plan = planner::select_hot_loop(
        executable.structure_map(),
        interpreter.profile().unwrap(),
        50,
    )
    .unwrap();
    let wxir = build_region(&executable, &plan).unwrap();
    let mut compiled = {
        let mut compiler = LlvmRegionCompiler::new(executable.id());
        compiler.compile(&wxir).unwrap()
    };
    let mut registers = vec![
        Value::SmallInt(0),
        Value::SmallInt(1),
        Value::SmallInt(1),
        Value::SmallInt(101),
        Value::Uninitialized,
    ];

    // When: LLVM native code executes through NativeRegionEntry.
    let exit = compiled.execute(&mut registers).unwrap();

    // Then: the region result and interpreter resume point match Cranelift's ABI.
    assert_eq!(exit.kind, WxExitKind::RegionExit);
    assert_eq!(exit.resume_pc, 9);
    assert_eq!(registers[0], Value::SmallInt(5050));
    assert_eq!(registers[1], Value::SmallInt(101));
}

#[test]
fn vm_promotes_cranelift_region_to_llvm_after_tier1_execution() {
    // Given: a VM configured for one Cranelift execution before Tier-2 promotion.
    let executable = sum_function();
    let mut vm = Vm::with_tier_thresholds(10, 1);
    let first = vm.execute(&executable).unwrap();

    // When: the cached region is entered again after its Tier-1 threshold.
    let second = vm.execute(&executable).unwrap();

    // Then: Cranelift ran first and LLVM replaced it for the second execution.
    assert_eq!(first.value, Value::SmallInt(5050));
    assert_eq!(second.value, Value::SmallInt(5050));
    assert_eq!(vm.jit_report().tier2_compilation_attempts, 1);
    assert_eq!(vm.jit_report().tier2_compiled_regions, 1);
    assert_eq!(vm.jit_report().tier2_native_executions, 1);
}

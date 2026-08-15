use num_bigint::BigInt;
use wustite::bytecode::{Function, Instruction};
use wustite::executable::{ExecutableFunction, ExecutableParameter};
use wustite::jit::{CompileError, CraneliftRegionCompiler, RegionCompiler};
use wustite::object::Object;
use wustite::planner::{self, JitPlan};
use wustite::structure_map::{
    LiveSlot, Region, RegionExit, RegionId, RegionKind, SlotType, StructureMap,
};
use wustite::value::Value;
use wustite::wvm::{JitFailureStage, Vm};
use wustite::wxir::{
    WxBlock, WxBlockId, WxBlockParam, WxExitKind, WxFunction, WxRegionOrigin, WxScalarType,
    WxStateValue, WxTerminator, WxType, WxValueId, build_region,
};

fn i64_slot(register: u16) -> LiveSlot {
    LiveSlot {
        register,
        ty: SlotType::SmallInt,
    }
}

fn bool_slot(register: u16) -> LiveSlot {
    LiveSlot {
        register,
        ty: SlotType::Bool,
    }
}

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
            regions: vec![Region {
                kind: RegionKind::Loop,
                entry: 4,
                backedge: Some(8),
                exits: vec![RegionExit { target: 9 }],
                live_slots: (0..4).map(i64_slot).collect(),
            }],
            operation_sites: vec![],
        },
    )
}

fn overflow_function() -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
            register_count: 4,
            code: vec![
                Instruction::ConstI64 {
                    dst: 0,
                    value: i64::MAX,
                },
                Instruction::ConstI64 { dst: 1, value: 1 },
                Instruction::LtI64 {
                    dst: 3,
                    lhs: 1,
                    rhs: 0,
                },
                Instruction::AddI64 {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Branch {
                    cond: 3,
                    yes: 6,
                    no: 5,
                },
                Instruction::Jump { target: 3 },
                Instruction::Return { src: 2 },
            ],
        },
        StructureMap {
            regions: vec![Region {
                kind: RegionKind::Loop,
                entry: 3,
                backedge: Some(5),
                exits: vec![RegionExit { target: 6 }],
                live_slots: vec![i64_slot(0), i64_slot(1), bool_slot(3)],
            }],
            operation_sites: vec![],
        },
    )
}

fn cached_entry_type_mismatch_function() -> ExecutableFunction {
    ExecutableFunction::new_with_parameters(
        Function {
            register_count: 5,
            code: vec![
                Instruction::ConstI64 { dst: 1, value: 0 },
                Instruction::ConstI64 { dst: 2, value: 1 },
                Instruction::ConstI64 { dst: 3, value: 3 },
                Instruction::LtI64 {
                    dst: 4,
                    lhs: 1,
                    rhs: 3,
                },
                Instruction::Branch {
                    cond: 4,
                    yes: 5,
                    no: 7,
                },
                Instruction::AddI64 {
                    dst: 1,
                    lhs: 1,
                    rhs: 2,
                },
                Instruction::Jump { target: 3 },
                Instruction::Return { src: 1 },
            ],
        },
        StructureMap {
            regions: vec![Region {
                kind: RegionKind::Loop,
                entry: 3,
                backedge: Some(6),
                exits: vec![RegionExit { target: 7 }],
                live_slots: vec![i64_slot(0), i64_slot(1), i64_slot(2), i64_slot(3)],
            }],
            operation_sites: vec![],
        },
        vec![ExecutableParameter {
            name: "opaque_entry".to_string(),
            register: 0,
            ty: SlotType::Any,
        }],
    )
}

fn invalid_region_metadata_function() -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
            register_count: 4,
            code: vec![
                Instruction::ConstI64 { dst: 0, value: 0 },
                Instruction::ConstI64 { dst: 1, value: 1 },
                Instruction::ConstI64 { dst: 3, value: 3 },
                Instruction::LtI64 {
                    dst: 2,
                    lhs: 0,
                    rhs: 3,
                },
                Instruction::Branch {
                    cond: 2,
                    yes: 6,
                    no: 8,
                },
                Instruction::Return { src: 0 },
                Instruction::AddI64 {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Jump { target: 3 },
                Instruction::Return { src: 0 },
            ],
        },
        StructureMap {
            regions: vec![Region {
                kind: RegionKind::Loop,
                entry: 3,
                backedge: Some(7),
                exits: vec![],
                live_slots: vec![i64_slot(0), i64_slot(1), i64_slot(3)],
            }],
            operation_sites: vec![],
        },
    )
}

fn unsupported_f64_wxir() -> WxFunction {
    let ty = WxType::Scalar(WxScalarType::F64);
    WxFunction {
        origin: WxRegionOrigin {
            region_id: RegionId(0),
            bytecode_header: 0,
            bytecode_backedge: 0,
        },
        entry: WxBlockId(0),
        entry_state: vec![WxStateValue {
            register: 0,
            value: WxValueId(0),
            ty,
        }],
        blocks: vec![WxBlock {
            id: WxBlockId(0),
            parameters: vec![WxBlockParam {
                id: WxValueId(0),
                ty,
            }],
            instructions: vec![],
            terminator: WxTerminator::Return { values: vec![] },
        }],
        returns: vec![],
        side_exits: vec![],
    }
}

#[test]
fn compiled_sum_region_restores_live_state_and_resume_pc() {
    let executable = sum_function();
    let mut vm = Vm::new();
    assert_eq!(
        vm.execute(&executable).unwrap().value,
        Value::SmallInt(5050)
    );
    let plan =
        planner::select_hot_loop(executable.structure_map(), vm.profile().unwrap(), 50).unwrap();
    let wxir = build_region(&executable, &plan).unwrap();
    let mut compiler = CraneliftRegionCompiler::new(executable.id());
    let mut compiled = compiler.compile(&wxir).unwrap();

    let mut registers = vec![Value::Uninitialized; 5];
    registers[0] = Value::SmallInt(0);
    registers[1] = Value::SmallInt(1);
    registers[2] = Value::SmallInt(1);
    registers[3] = Value::SmallInt(101);

    let exit = compiled.execute(&mut registers).unwrap();
    assert_eq!(exit.kind, WxExitKind::RegionExit);
    assert_eq!(exit.resume_pc, 9);
    assert_eq!(registers[0], Value::SmallInt(5050));
    assert_eq!(registers[1], Value::SmallInt(101));
}

#[test]
fn compiled_overflow_exits_before_updating_destination() {
    let executable = overflow_function();
    let plan = JitPlan {
        region_id: RegionId(0),
        header: 3,
        backedge: 5,
        exits: vec![RegionExit { target: 6 }],
        live_slots: executable.structure_map().regions[0].live_slots.clone(),
    };
    let wxir = build_region(&executable, &plan).unwrap();
    let mut compiler = CraneliftRegionCompiler::new(executable.id());
    let mut compiled = compiler.compile(&wxir).unwrap();
    // Given a checked native SmallInt addition at the i64 boundary,
    // When the compiled region executes the overflowing instruction,
    // Then it exits for replay before mutating the destination register.
    let mut registers = vec![
        Value::SmallInt(i64::MAX),
        Value::SmallInt(1),
        Value::SmallInt(99),
        Value::Bool(true),
    ];

    let exit = compiled.execute(&mut registers).unwrap();
    assert_eq!(exit.kind, WxExitKind::ReplayInstruction);
    assert_eq!(exit.resume_pc, 3);
    assert_eq!(registers[0], Value::SmallInt(i64::MAX));
    assert_eq!(registers[1], Value::SmallInt(1));
    assert_eq!(registers[2], Value::SmallInt(99));
    assert_eq!(registers[3], Value::Bool(true));
}

#[test]
fn vm_automatically_tiers_up_sum_once() {
    let executable = sum_function();
    let mut vm = Vm::with_hot_threshold(10);

    let result = vm.execute(&executable).unwrap();

    assert_eq!(result.value, Value::SmallInt(5050));
    assert_eq!(vm.profile().unwrap().entry_count(RegionId(0)), 10);
    assert_eq!(vm.jit_report().compilation_attempts, 1);
    assert_eq!(vm.jit_report().compiled_regions, 1);
    assert_eq!(vm.jit_report().native_executions, 1);
    assert_eq!(vm.jit_report().last_resume_pc, Some(9));
    assert_eq!(vm.jit_report().last_exit_kind, Some(WxExitKind::RegionExit));
}

#[test]
fn vm_replays_synthetic_overflow_exit_in_interpreter() {
    let executable = overflow_function();
    let mut vm = Vm::with_hot_threshold(0);

    // Given a hot VM whose native SmallInt region exits before committing overflow,
    // When the interpreter replays the overflowing instruction,
    // Then it promotes the result to a heap BigInt and completes successfully.
    let result = vm.execute(&executable).unwrap();

    let Value::Object(object_ref) = result.value else {
        panic!(
            "expected replay to return a heap object, got {:?}",
            result.value
        );
    };
    assert_eq!(
        vm.object(object_ref).unwrap(),
        &Object::BigInt(BigInt::from(i64::MAX) + 1)
    );
    assert_eq!(vm.profile().unwrap().entry_count(RegionId(0)), 1);
    assert_eq!(vm.jit_report().compilation_attempts, 1);
    assert_eq!(vm.jit_report().compiled_regions, 1);
    assert_eq!(vm.jit_report().native_executions, 1);
    assert_eq!(vm.jit_report().last_resume_pc, Some(3));
    assert_eq!(
        vm.jit_report().last_exit_kind,
        Some(WxExitKind::ReplayInstruction)
    );
}

#[test]
fn vm_suppresses_cached_region_for_object_entry_without_disabling_it() {
    let executable = cached_entry_type_mismatch_function();
    let mut vm = Vm::with_hot_threshold(0);

    // Given a cached region whose live entry state is specialized to SmallInt,
    // When the first scalar invocation executes, Then native code is compiled and reused.
    let first = vm
        .execute_with_args(&executable, &[Value::SmallInt(9)])
        .unwrap();
    assert_eq!(first.value, Value::SmallInt(3));
    assert_eq!(vm.jit_report().compilation_attempts, 1);
    assert_eq!(vm.jit_report().compiled_regions, 1);
    assert_eq!(vm.jit_report().native_executions, 1);

    let object_ref = vm
        .allocate_object(Object::String("object entry".to_string()))
        .unwrap();

    // Given the same executable and a cached native region,
    // When an ObjectRef enters its SmallInt-specialized live state,
    // Then this invocation falls back without disabling the cached region.
    let second = vm
        .execute_with_args(&executable, &[Value::Object(object_ref)])
        .unwrap();
    assert_eq!(second.value, Value::SmallInt(3));
    assert_eq!(vm.jit_report().compilation_attempts, 0);
    assert_eq!(vm.jit_report().compiled_regions, 0);
    assert_eq!(vm.jit_report().native_executions, 0);
    assert_eq!(vm.jit_report().disabled_regions, 0);
    assert!(vm.jit_report().failures.is_empty());

    // Given the cached region survived the object-entry fallback,
    // When a later scalar invocation runs, Then native execution is reused.
    let third = vm
        .execute_with_args(&executable, &[Value::SmallInt(9)])
        .unwrap();
    assert_eq!(third.value, Value::SmallInt(3));
    assert_eq!(vm.jit_report().compilation_attempts, 0);
    assert_eq!(vm.jit_report().compiled_regions, 0);
    assert_eq!(vm.jit_report().native_executions, 1);
    assert_eq!(vm.jit_report().disabled_regions, 0);
    assert!(vm.jit_report().failures.is_empty());
}

#[test]
fn invalid_region_metadata_is_disabled_after_one_attempt() {
    let executable = invalid_region_metadata_function();
    let mut vm = Vm::with_hot_threshold(0);

    let result = vm.execute(&executable).unwrap();

    assert_eq!(result.value, Value::SmallInt(3));
    assert_eq!(vm.profile().unwrap().entry_count(RegionId(0)), 4);
    assert_eq!(vm.jit_report().compilation_attempts, 1);
    assert_eq!(vm.jit_report().compiled_regions, 0);
    assert_eq!(vm.jit_report().disabled_regions, 1);
    assert_eq!(vm.jit_report().native_executions, 0);
    assert_eq!(vm.jit_report().failures.len(), 1);
    let failure = &vm.jit_report().failures[0];
    assert_eq!(failure.region_id, RegionId(0));
    assert_eq!(failure.stage, JitFailureStage::BuildWxir);
    assert!(failure.reason.contains("has no JitPlan exit"));
}

#[test]
fn backend_rejects_unsupported_f64_state() {
    let executable = sum_function();
    let error = CraneliftRegionCompiler::new(executable.id())
        .compile(&unsupported_f64_wxir())
        .err()
        .unwrap();

    assert_eq!(
        error,
        CompileError::UnsupportedType(WxType::Scalar(WxScalarType::F64))
    );
}

#[test]
fn high_threshold_keeps_sum_interpreter_only() {
    let executable = sum_function();
    let mut vm = Vm::with_hot_threshold(102);

    let result = vm.execute(&executable).unwrap();

    assert_eq!(result.value, Value::SmallInt(5050));
    assert_eq!(vm.profile().unwrap().entry_count(RegionId(0)), 101);
    assert_eq!(vm.jit_report().compilation_attempts, 0);
    assert_eq!(vm.jit_report().native_executions, 0);
    assert_eq!(vm.jit_report().last_resume_pc, None);
}

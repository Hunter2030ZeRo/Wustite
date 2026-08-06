use num_bigint::BigInt;
use wustite::bytecode::{BinaryOperator, CompareOperator, Function, Instruction};
use wustite::executable::ExecutableFunction;
use wustite::object::Object;
use wustite::structure_map::{
    LiveSlot, LoopRegion, OperationSite, OperationSiteId, RegionExit, SlotType, StructureMap,
    TypeFact,
};
use wustite::value::Value;
use wustite::wvm::Vm;
use wustite::wxir::WxExitKind;

fn exact(ty: SlotType) -> TypeFact {
    TypeFact::Exact(ty)
}

fn site(pc: usize, lhs: TypeFact, rhs: TypeFact, result: TypeFact) -> OperationSite {
    OperationSite {
        pc,
        lhs,
        rhs,
        result,
    }
}

fn exact_add_lt() -> ExecutableFunction {
    ExecutableFunction::new(
        Function {
            register_count: 5,
            code: vec![
                Instruction::ConstSmallInt { dst: 0, value: 40 },
                Instruction::ConstSmallInt { dst: 1, value: 2 },
                Instruction::BinaryOp {
                    dst: 2,
                    op: BinaryOperator::Add,
                    lhs: 0,
                    rhs: 1,
                    site: OperationSiteId(0),
                },
                Instruction::ConstSmallInt { dst: 3, value: 43 },
                Instruction::CompareOp {
                    dst: 4,
                    op: CompareOperator::Lt,
                    lhs: 2,
                    rhs: 3,
                    site: OperationSiteId(1),
                },
                Instruction::Return { src: 4 },
            ],
        },
        StructureMap {
            loops: Vec::new(),
            operation_sites: vec![
                site(
                    2,
                    exact(SlotType::SmallInt),
                    exact(SlotType::SmallInt),
                    exact(SlotType::SmallInt),
                ),
                site(
                    4,
                    exact(SlotType::SmallInt),
                    exact(SlotType::SmallInt),
                    exact(SlotType::Bool),
                ),
            ],
        },
    )
}

fn semantic_overflow_loop() -> ExecutableFunction {
    let small = exact(SlotType::SmallInt);
    ExecutableFunction::new(
        Function {
            register_count: 5,
            code: vec![
                Instruction::ConstSmallInt {
                    dst: 0,
                    value: i64::MAX,
                },
                Instruction::ConstSmallInt { dst: 1, value: 1 },
                Instruction::ConstBool {
                    dst: 4,
                    value: true,
                },
                Instruction::BinaryOp {
                    dst: 2,
                    op: BinaryOperator::Add,
                    lhs: 0,
                    rhs: 1,
                    site: OperationSiteId(0),
                },
                Instruction::BinaryOp {
                    dst: 3,
                    op: BinaryOperator::Add,
                    lhs: 2,
                    rhs: 1,
                    site: OperationSiteId(1),
                },
                Instruction::Branch {
                    cond: 4,
                    yes: 7,
                    no: 6,
                },
                Instruction::Jump { target: 3 },
                Instruction::Return { src: 3 },
            ],
        },
        StructureMap {
            loops: vec![LoopRegion {
                header: 3,
                backedge: 6,
                exits: vec![RegionExit { target: 7 }],
                live_slots: vec![
                    LiveSlot {
                        register: 0,
                        ty: SlotType::SmallInt,
                    },
                    LiveSlot {
                        register: 1,
                        ty: SlotType::SmallInt,
                    },
                    LiveSlot {
                        register: 4,
                        ty: SlotType::Bool,
                    },
                ],
            }],
            operation_sites: vec![site(3, small, small, small), site(4, small, small, small)],
        },
    )
}

#[test]
fn exact_add_and_lt_execute_without_mutating_semantic_bytecode() {
    let executable = exact_add_lt();
    let clone = executable.clone();
    let bytecode_before = executable.bytecode().clone();
    let structure_before = executable.structure_map().clone();
    let mut vm = Vm::new();

    assert_eq!(vm.execute(&executable).unwrap().value, Value::Bool(true));
    assert_eq!(vm.execute(&executable).unwrap().value, Value::Bool(true));
    assert_eq!(vm.execute(&clone).unwrap().value, Value::Bool(true));
    assert_eq!(executable.bytecode(), &bytecode_before);
    assert_eq!(executable.structure_map(), &structure_before);
}

#[test]
fn unknown_unsupported_and_runtime_mismatch_use_semantic_behavior() {
    let executable = ExecutableFunction::new(
        Function {
            register_count: 6,
            code: vec![
                Instruction::ConstFloat { dst: 0, value: 1.5 },
                Instruction::ConstFloat { dst: 1, value: 2.5 },
                Instruction::BinaryOp {
                    dst: 2,
                    op: BinaryOperator::Add,
                    lhs: 0,
                    rhs: 1,
                    site: OperationSiteId(0),
                },
                Instruction::ConstSmallInt { dst: 3, value: 9 },
                Instruction::ConstSmallInt { dst: 4, value: 4 },
                Instruction::BinaryOp {
                    dst: 5,
                    op: BinaryOperator::Subtract,
                    lhs: 3,
                    rhs: 4,
                    site: OperationSiteId(1),
                },
                Instruction::Return { src: 2 },
            ],
        },
        StructureMap {
            loops: Vec::new(),
            operation_sites: vec![
                site(
                    2,
                    exact(SlotType::SmallInt),
                    exact(SlotType::SmallInt),
                    exact(SlotType::SmallInt),
                ),
                site(5, TypeFact::Unknown, TypeFact::Unknown, TypeFact::Unknown),
            ],
        },
    );

    assert_eq!(
        Vm::new().execute(&executable).unwrap().value,
        Value::Float(4.0)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn semantic_jit_replay_promotes_then_falls_back_for_bigint() {
    let executable = semantic_overflow_loop();
    let bytecode_before = executable.bytecode().clone();
    let structure_before = executable.structure_map().clone();
    let mut vm = Vm::with_hot_threshold(0);

    for expected_attempts in [1, 0] {
        let result = vm.execute(&executable).unwrap();
        let Value::Object(reference) = result.value else {
            panic!("expected promoted BigInt")
        };
        assert_eq!(
            vm.object(reference).unwrap(),
            &Object::BigInt(BigInt::from(i64::MAX) + 2)
        );
        assert_eq!(vm.jit_report().compilation_attempts, expected_attempts);
        assert_eq!(vm.jit_report().native_executions, 1);
        assert_eq!(vm.jit_report().last_resume_pc, Some(3));
        assert_eq!(
            vm.jit_report().last_exit_kind,
            Some(WxExitKind::ReplayInstruction)
        );
        assert_eq!(executable.bytecode(), &bytecode_before);
        assert_eq!(executable.structure_map(), &structure_before);
    }
}

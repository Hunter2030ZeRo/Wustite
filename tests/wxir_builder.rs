use wustite::bytecode::{Function, Instruction};
use wustite::executable::ExecutableFunction;
use wustite::planner::{JitPlan, select_hot_loop};
use wustite::structure_map::{LiveSlot, LoopRegion, RegionExit, RegionId, SlotType, StructureMap};
use wustite::wvm::Vm;
use wustite::wxir::{
    self, WxBuildError, WxExitKind, WxGuardMode, WxInstKind, WxIntOverflowOp, WxScalarType,
    WxTerminator, WxType, build_region,
};

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
                live_slots: vec![
                    LiveSlot {
                        register: 0,
                        ty: SlotType::I64,
                    },
                    LiveSlot {
                        register: 1,
                        ty: SlotType::I64,
                    },
                    LiveSlot {
                        register: 2,
                        ty: SlotType::I64,
                    },
                    LiveSlot {
                        register: 3,
                        ty: SlotType::I64,
                    },
                ],
            }],
        },
    }
}

#[test]
fn sum_region_lowers_to_verified_ssa() {
    let executable = sum_function();
    let mut vm = Vm::new();
    vm.execute(&executable).unwrap();
    let plan = select_hot_loop(&executable.structure_map, vm.profile().unwrap(), 50).unwrap();

    let function = build_region(&executable, &plan).unwrap();
    wxir::verify(&function).unwrap();

    let header = function
        .blocks
        .iter()
        .find(|block| block.id == function.entry)
        .unwrap();
    assert_eq!(header.parameters.len(), plan.live_slots.len());
    assert!(
        header
            .parameters
            .iter()
            .all(|parameter| parameter.ty == WxType::Scalar(WxScalarType::I64))
    );

    let backedge = function
        .blocks
        .iter()
        .find(|block| {
            matches!(
                block.terminator,
                WxTerminator::Jump { target, .. } if target == function.entry
            )
        })
        .unwrap();
    let checked_adds: Vec<_> = backedge
        .instructions
        .windows(2)
        .filter_map(
            |instructions| match (&instructions[0].kind, &instructions[1].kind) {
                (
                    WxInstKind::IntegerBinaryWithOverflow {
                        op: WxIntOverflowOp::Add,
                        lhs,
                        ..
                    },
                    WxInstKind::Guard {
                        condition,
                        exit,
                        mode: WxGuardMode::ExitWhenTrue,
                    },
                ) => Some((
                    instructions[0].results[0].id,
                    instructions[0].results[1].id,
                    *lhs,
                    *condition,
                    *exit,
                )),
                _ => None,
            },
        )
        .collect();
    assert_eq!(checked_adds.len(), 2);
    assert!(
        checked_adds
            .iter()
            .all(|(_, overflow, _, condition, _)| overflow == condition)
    );

    let WxTerminator::Jump { target, arguments } = &backedge.terminator else {
        unreachable!();
    };
    assert_eq!(*target, function.entry);
    assert_eq!(arguments[0], checked_adds[0].0);
    assert_eq!(arguments[1], checked_adds[1].0);
    assert_ne!(arguments[0], header.parameters[0].id);
    assert_ne!(arguments[1], header.parameters[1].id);

    assert_eq!(function.side_exits.len(), 3);
    let normal_exit = function
        .side_exits
        .iter()
        .find(|side_exit| side_exit.resume_pc == 9)
        .unwrap();
    assert_eq!(normal_exit.kind, WxExitKind::RegionExit);
    assert_eq!(
        normal_exit
            .state
            .iter()
            .map(|state| state.register)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    for ((_, _, lhs, _, exit), (resume_pc, dst)) in checked_adds.iter().zip([(6, 0), (7, 1)]) {
        let metadata = function
            .side_exits
            .iter()
            .find(|side_exit| side_exit.id == *exit)
            .unwrap();
        assert_eq!(metadata.kind, WxExitKind::ReplayInstruction);
        assert_eq!(metadata.resume_pc, resume_pc);
        assert_eq!(
            metadata
                .state
                .iter()
                .find(|state| state.register == dst)
                .unwrap()
                .value,
            *lhs
        );
    }
    let second_overflow = function
        .side_exits
        .iter()
        .find(|side_exit| side_exit.id == checked_adds[1].4)
        .unwrap();
    assert_eq!(
        second_overflow
            .state
            .iter()
            .find(|state| state.register == 0)
            .unwrap()
            .value,
        checked_adds[0].0
    );

    let printed = wxir::print_function(&function);
    assert!(printed.contains("icmp.slt"));
    assert!(printed.contains("iadd.with_overflow"));
    assert!(printed.contains("guard.exit_when_true"));
    assert!(printed.contains("side_exit x0 kind=region resume_pc=9"));
    assert!(printed.contains("kind=replay_instruction resume_pc=6"));
}

#[test]
fn return_inside_region_is_rejected_without_panicking() {
    let executable = ExecutableFunction {
        bytecode: Function {
            register_count: 1,
            code: vec![
                Instruction::Return { src: 0 },
                Instruction::Jump { target: 0 },
            ],
        },
        structure_map: StructureMap {
            loops: vec![LoopRegion {
                header: 0,
                backedge: 1,
                exits: vec![],
                live_slots: vec![LiveSlot {
                    register: 0,
                    ty: SlotType::I64,
                }],
            }],
        },
    };
    let plan = JitPlan {
        region_id: RegionId(0),
        header: 0,
        backedge: 1,
        exits: vec![],
        live_slots: executable.structure_map.loops[0].live_slots.clone(),
    };

    assert_eq!(
        build_region(&executable, &plan),
        Err(WxBuildError::UnsupportedInstruction {
            pc: 0,
            instruction: "Return",
        })
    );
}

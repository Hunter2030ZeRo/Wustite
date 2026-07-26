use wustite::bytecode::{Function, Instruction};
use wustite::executable::ExecutableFunction;
use wustite::planner::{JitPlan, select_hot_loop};
use wustite::structure_map::{LiveSlot, LoopRegion, RegionExit, RegionId, SlotType, StructureMap};
use wustite::wvm::Vm;
use wustite::wxir::{
    self, WxBinaryOp, WxBuildError, WxInstKind, WxIntBinaryOp, WxOverflowBehavior, WxScalarType,
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
        .iter()
        .filter_map(|instruction| match instruction.kind {
            WxInstKind::Binary {
                op: WxBinaryOp::Integer(WxIntBinaryOp::Add(WxOverflowBehavior::Checked)),
                ..
            } => Some(instruction.results[0].id),
            _ => None,
        })
        .collect();
    assert_eq!(checked_adds.len(), 2);

    let WxTerminator::Jump { target, arguments } = &backedge.terminator else {
        unreachable!();
    };
    assert_eq!(*target, function.entry);
    assert_eq!(arguments[0], checked_adds[0]);
    assert_eq!(arguments[1], checked_adds[1]);
    assert_ne!(arguments[0], header.parameters[0].id);
    assert_ne!(arguments[1], header.parameters[1].id);

    assert_eq!(function.side_exits.len(), 1);
    assert_eq!(function.side_exits[0].resume_pc, 9);
    assert_eq!(
        function.side_exits[0]
            .state
            .iter()
            .map(|state| state.register)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );

    let printed = wxir::print_function(&function);
    assert!(printed.contains("icmp.slt"));
    assert!(printed.contains("iadd.checked"));
    assert!(printed.contains("side_exit x0 resume_pc=9"));
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

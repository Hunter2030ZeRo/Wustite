use wustite::structure_map::RegionId;
use wustite::wxir::{
    self, WxBlock, WxBlockId, WxBlockParam, WxBlockTarget, WxCompareOp, WxConstant, WxExitId,
    WxFunction, WxGuardMode, WxInst, WxInstKind, WxInstResult, WxIntCompareOp, WxIntOverflowOp,
    WxRegionOrigin, WxScalarType, WxSideExit, WxStateValue, WxTerminator, WxType, WxValueId,
};

fn i64_type() -> WxType {
    WxType::Scalar(WxScalarType::I64)
}

fn valid_function() -> WxFunction {
    WxFunction {
        origin: WxRegionOrigin {
            region_id: RegionId(0),
            bytecode_header: 4,
            bytecode_backedge: 8,
        },
        entry: WxBlockId(0),
        blocks: vec![
            WxBlock {
                id: WxBlockId(0),
                parameters: vec![WxBlockParam {
                    id: WxValueId(0),
                    ty: i64_type(),
                }],
                instructions: vec![
                    WxInst {
                        results: vec![WxInstResult {
                            id: WxValueId(1),
                            ty: i64_type(),
                        }],
                        kind: WxInstKind::Constant(WxConstant::Int(10)),
                    },
                    WxInst {
                        results: vec![WxInstResult {
                            id: WxValueId(2),
                            ty: WxType::Scalar(WxScalarType::I1),
                        }],
                        kind: WxInstKind::Compare {
                            op: WxCompareOp::Integer(WxIntCompareOp::SignedLt),
                            lhs: WxValueId(0),
                            rhs: WxValueId(1),
                        },
                    },
                ],
                terminator: WxTerminator::Branch {
                    condition: WxValueId(2),
                    yes: WxBlockTarget {
                        block: WxBlockId(1),
                        arguments: vec![WxValueId(0)],
                    },
                    no: WxBlockTarget {
                        block: WxBlockId(2),
                        arguments: vec![WxValueId(0)],
                    },
                },
            },
            WxBlock {
                id: WxBlockId(1),
                parameters: vec![WxBlockParam {
                    id: WxValueId(3),
                    ty: i64_type(),
                }],
                instructions: vec![],
                terminator: WxTerminator::Return {
                    values: vec![WxValueId(3)],
                },
            },
            WxBlock {
                id: WxBlockId(2),
                parameters: vec![WxBlockParam {
                    id: WxValueId(4),
                    ty: i64_type(),
                }],
                instructions: vec![],
                terminator: WxTerminator::SideExit {
                    exit: WxExitId(0),
                    values: vec![WxValueId(4)],
                },
            },
        ],
        returns: vec![i64_type()],
        side_exits: vec![WxSideExit {
            id: WxExitId(0),
            resume_pc: 9,
            state: vec![WxStateValue {
                register: 0,
                value: WxValueId(4),
                ty: i64_type(),
            }],
        }],
    }
}

#[test]
fn valid_function_verifies_and_prints() {
    let function = valid_function();
    wxir::verify(&function).unwrap();

    let printed = wxir::print_function(&function);
    assert!(printed.contains("b0("));
    assert!(printed.contains("icmp.slt"));
    assert!(printed.contains("side_exit x0"));
}

#[test]
fn duplicate_value_id_is_rejected() {
    let mut function = valid_function();
    function.blocks[0].instructions[0].results[0].id = WxValueId(0);

    assert!(
        wxir::verify(&function)
            .unwrap_err()
            .contains("defined more than once")
    );
}

#[test]
fn non_boolean_branch_condition_is_rejected() {
    let mut function = valid_function();
    function.blocks[0].terminator = WxTerminator::Branch {
        condition: WxValueId(0),
        yes: WxBlockTarget {
            block: WxBlockId(1),
            arguments: vec![WxValueId(0)],
        },
        no: WxBlockTarget {
            block: WxBlockId(2),
            arguments: vec![WxValueId(0)],
        },
    };

    assert!(wxir::verify(&function).unwrap_err().contains("expected i1"));
}

#[test]
fn block_argument_type_mismatch_is_rejected() {
    let mut function = valid_function();
    function.blocks[1].parameters[0].ty = WxType::Scalar(WxScalarType::F64);

    assert!(
        wxir::verify(&function)
            .unwrap_err()
            .contains("edge to b1 argument 0")
    );
}

#[test]
fn non_pointer_load_address_is_rejected() {
    let mut function = valid_function();
    function.blocks[0].instructions.push(WxInst {
        results: vec![WxInstResult {
            id: WxValueId(5),
            ty: i64_type(),
        }],
        kind: WxInstKind::Load {
            address: WxValueId(0),
        },
    });

    assert!(
        wxir::verify(&function)
            .unwrap_err()
            .contains("expected ptr")
    );
}

#[test]
fn duplicate_side_exit_register_is_rejected() {
    let mut function = valid_function();
    function.side_exits[0].state.push(WxStateValue {
        register: 0,
        value: WxValueId(4),
        ty: i64_type(),
    });
    if let WxTerminator::SideExit { values, .. } = &mut function.blocks[2].terminator {
        values.push(WxValueId(4));
    }

    assert!(
        wxir::verify(&function)
            .unwrap_err()
            .contains("duplicate WVM register")
    );
}

#[test]
fn invalid_checked_integer_result_signature_is_rejected() {
    let mut function = valid_function();
    function.blocks[0].instructions.insert(
        1,
        WxInst {
            results: vec![WxInstResult {
                id: WxValueId(5),
                ty: i64_type(),
            }],
            kind: WxInstKind::IntegerBinaryWithOverflow {
                op: WxIntOverflowOp::Add,
                lhs: WxValueId(0),
                rhs: WxValueId(1),
            },
        },
    );

    assert!(
        wxir::verify(&function)
            .unwrap_err()
            .contains("requires two results")
    );
}

#[test]
fn invalid_guard_condition_is_rejected() {
    let mut function = valid_function();
    function.side_exits.push(WxSideExit {
        id: WxExitId(1),
        resume_pc: 6,
        state: vec![WxStateValue {
            register: 0,
            value: WxValueId(0),
            ty: i64_type(),
        }],
    });
    function.blocks[0].instructions.insert(
        1,
        WxInst {
            results: vec![],
            kind: WxInstKind::Guard {
                condition: WxValueId(0),
                exit: WxExitId(1),
                mode: WxGuardMode::ExitWhenTrue,
            },
        },
    );

    assert!(wxir::verify(&function).unwrap_err().contains("expected i1"));
}

#[test]
fn vector_masks_are_valid_but_pointer_lanes_are_rejected() {
    let function_with_parameter = |ty| WxFunction {
        origin: WxRegionOrigin {
            region_id: RegionId(0),
            bytecode_header: 0,
            bytecode_backedge: 0,
        },
        entry: WxBlockId(0),
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
    };

    let mask = function_with_parameter(WxType::Vector {
        lane: WxScalarType::I1,
        lanes: 8,
    });
    wxir::verify(&mask).unwrap();

    let pointers = function_with_parameter(WxType::Vector {
        lane: WxScalarType::Ptr,
        lanes: 4,
    });
    assert!(
        wxir::verify(&pointers)
            .unwrap_err()
            .contains("Ptr vector lanes")
    );
}

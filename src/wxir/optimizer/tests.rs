use super::*;
use crate::structure_map::RegionId;
use crate::wxir::{
    WxBinaryOp, WxBlock, WxBlockId, WxConstant, WxExitId, WxExitKind, WxGuardMode, WxInst,
    WxInstKind, WxInstResult, WxIntBinaryOp, WxIntOverflowOp, WxRegionOrigin, WxRuntimeInput,
    WxScalarType, WxSideExit, WxTerminator, WxType, WxValueId, verify,
};

fn result(id: u32) -> WxInstResult {
    WxInstResult {
        id: WxValueId(id),
        ty: WxType::Scalar(WxScalarType::I64),
    }
}

fn constant(id: u32, value: i64) -> WxInst {
    WxInst {
        results: vec![result(id)],
        kind: WxInstKind::Constant(WxConstant::Int(value)),
    }
}

fn add(id: u32, lhs: u32, rhs: u32) -> WxInst {
    WxInst {
        results: vec![result(id)],
        kind: WxInstKind::Binary {
            op: WxBinaryOp::Integer(WxIntBinaryOp::Add),
            lhs: WxValueId(lhs),
            rhs: WxValueId(rhs),
        },
    }
}

fn function(instructions: Vec<WxInst>, returns: Vec<WxValueId>) -> WxFunction {
    WxFunction {
        origin: WxRegionOrigin {
            region_id: RegionId(0),
            bytecode_header: 0,
            bytecode_backedge: 0,
        },
        entry: WxBlockId(0),
        entry_state: Vec::new(),
        blocks: vec![WxBlock {
            id: WxBlockId(0),
            parameters: Vec::new(),
            instructions,
            terminator: WxTerminator::Return {
                values: returns.clone(),
            },
        }],
        returns: vec![WxType::Scalar(WxScalarType::I64); returns.len()],
        side_exits: Vec::new(),
    }
}

#[test]
fn optimizer_canonicalizes_constants_cses_expressions_and_removes_dead_values() {
    // Given: duplicate constants, duplicate pure additions, and one dead definition.
    let mut function = function(
        vec![
            constant(0, 2),
            constant(1, 2),
            constant(2, 3),
            add(3, 0, 2),
            add(4, 1, 2),
            constant(5, 99),
        ],
        vec![WxValueId(4)],
    );

    // When: the deterministic optimizer runs twice.
    optimize(&mut function);
    let once = function.clone();
    optimize(&mut function);

    // Then: the canonical expression remains and the pass is idempotent and valid.
    assert_eq!(function, once);
    assert_eq!(function.blocks[0].instructions.len(), 3);
    assert_eq!(
        function.blocks[0].terminator,
        WxTerminator::Return {
            values: vec![WxValueId(3)]
        }
    );
    verify(&function).unwrap();
}

#[test]
fn optimizer_preserves_runtime_barriers_and_values_live_on_both_sides() {
    // Given: equal expressions separated by a runtime state synchronization barrier.
    let mut function = function(
        vec![
            constant(0, 2),
            constant(1, 3),
            add(2, 0, 1),
            WxInst {
                results: Vec::new(),
                kind: WxInstKind::RuntimeCall {
                    pc: 0,
                    inputs: vec![WxRuntimeInput {
                        register: 0,
                        value: WxValueId(2),
                        ty: WxType::Scalar(WxScalarType::I64),
                    }],
                    output: None,
                    effects: crate::structure_map::EffectSummary::default(),
                },
            },
            add(3, 0, 1),
        ],
        vec![WxValueId(2), WxValueId(3)],
    );

    // When: local value numbering reaches the barrier.
    optimize(&mut function);

    // Then: both additions and the runtime call retain their relative order.
    assert_eq!(function.blocks[0].instructions.len(), 5);
    assert!(matches!(
        function.blocks[0].instructions[3].kind,
        WxInstKind::RuntimeCall { .. }
    ));
    verify(&function).unwrap();
}

#[test]
fn optimizer_eliminates_repeated_checked_arithmetic_and_its_guard() {
    // Given: identical checked additions guarded twice without an intervening effect.
    let checked = |value, overflow| WxInst {
        results: vec![
            result(value),
            WxInstResult {
                id: WxValueId(overflow),
                ty: WxType::Scalar(WxScalarType::I1),
            },
        ],
        kind: WxInstKind::IntegerBinaryWithOverflow {
            op: WxIntOverflowOp::Add,
            lhs: WxValueId(0),
            rhs: WxValueId(1),
        },
    };
    let guard = |condition, exit| WxInst {
        results: Vec::new(),
        kind: WxInstKind::Guard {
            condition: WxValueId(condition),
            exit: WxExitId(exit),
            mode: WxGuardMode::ExitWhenTrue,
        },
    };
    let mut function = function(
        vec![
            constant(0, 2),
            constant(1, 3),
            checked(2, 3),
            guard(3, 0),
            checked(4, 5),
            guard(5, 1),
        ],
        vec![WxValueId(4)],
    );
    function.side_exits = [0, 1]
        .into_iter()
        .map(|exit| WxSideExit {
            id: WxExitId(exit),
            kind: WxExitKind::ReplayInstruction,
            resume_pc: usize::try_from(exit).unwrap(),
            state: Vec::new(),
        })
        .collect();

    // When: local value numbering sees the second checked operation and guard.
    optimize(&mut function);

    // Then: the successful first guard dominates the duplicate and only one pair remains.
    assert_eq!(
        function.blocks[0]
            .instructions
            .iter()
            .filter(|instruction| matches!(
                instruction.kind,
                WxInstKind::IntegerBinaryWithOverflow { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        function.blocks[0]
            .instructions
            .iter()
            .filter(|instruction| matches!(instruction.kind, WxInstKind::Guard { .. }))
            .count(),
        1
    );
    verify(&function).unwrap();
}

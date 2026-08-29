use std::collections::HashMap;

use crate::wxir::{
    WxBinaryOp, WxCastOp, WxCompareOp, WxConstant, WxFloatBinaryOp, WxFloatCompareOp, WxInst,
    WxInstKind, WxIntBinaryOp, WxIntCompareOp, WxType, WxValueId,
};

use super::checked::{canonical_checked_operands, overflow_code};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ExpressionKey {
    Constant(WxType, ConstantKey),
    Binary(WxType, u8, WxValueId, WxValueId),
    Checked(u8, WxValueId, WxValueId),
    Compare(u8, WxValueId, WxValueId),
    Cast(WxType, u8, WxValueId),
}

impl ExpressionKey {
    pub(super) fn new(instruction: &WxInst) -> Option<Self> {
        let result = instruction.results.first()?;
        match instruction.kind {
            WxInstKind::Constant(constant) => {
                Some(Self::Constant(result.ty, ConstantKey::new(constant)))
            }
            WxInstKind::Binary { op, lhs, rhs } if instruction.results.len() == 1 => {
                let (lhs, rhs) = canonical_operands(op, lhs, rhs);
                Some(Self::Binary(result.ty, binary_code(op), lhs, rhs))
            }
            WxInstKind::IntegerBinaryWithOverflow { op, lhs, rhs }
                if instruction.results.len() == 2 =>
            {
                let (lhs, rhs) = canonical_checked_operands(op, lhs, rhs);
                Some(Self::Checked(overflow_code(op), lhs, rhs))
            }
            WxInstKind::Compare { op, lhs, rhs } if instruction.results.len() == 1 => {
                let (lhs, rhs) = canonical_compare_operands(op, lhs, rhs);
                Some(Self::Compare(compare_code(op), lhs, rhs))
            }
            WxInstKind::Cast { op, value } if instruction.results.len() == 1 => {
                Some(Self::Cast(result.ty, cast_code(op), value))
            }
            WxInstKind::Binary { .. }
            | WxInstKind::IntegerBinaryWithOverflow { .. }
            | WxInstKind::Compare { .. }
            | WxInstKind::Cast { .. }
            | WxInstKind::Load { .. }
            | WxInstKind::Store { .. }
            | WxInstKind::PointerOffset { .. }
            | WxInstKind::Splat { .. }
            | WxInstKind::ExtractLane { .. }
            | WxInstKind::InsertLane { .. }
            | WxInstKind::Shuffle { .. }
            | WxInstKind::Guard { .. }
            | WxInstKind::GuardSequence { .. }
            | WxInstKind::SequenceLength { .. }
            | WxInstKind::SequenceGet { .. }
            | WxInstKind::SequenceSet { .. }
            | WxInstKind::SequenceMutate { .. }
            | WxInstKind::MaterializeSequence { .. }
            | WxInstKind::Call { .. }
            | WxInstKind::RuntimeCall { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ConstantKey {
    Bool(bool),
    Int(i64),
    F32(u32),
    F64(u64),
    NullPtr,
}

impl ConstantKey {
    fn new(constant: WxConstant) -> Self {
        match constant {
            WxConstant::Bool(value) => Self::Bool(value),
            WxConstant::Int(value) => Self::Int(value),
            WxConstant::F32(value) => Self::F32(value.to_bits()),
            WxConstant::F64(value) => Self::F64(value.to_bits()),
            WxConstant::NullPtr => Self::NullPtr,
        }
    }
}

pub(super) fn folded_instruction(instruction: &WxInst, prior: &[WxInst]) -> Option<WxInst> {
    let constants = prior
        .iter()
        .filter_map(
            |candidate| match (&candidate.results[..], &candidate.kind) {
                ([result], WxInstKind::Constant(value)) => Some((result.id, *value)),
                _ => None,
            },
        )
        .collect::<HashMap<_, _>>();
    let result = *instruction.results.first()?;
    let constant = match instruction.kind {
        WxInstKind::Binary { op, lhs, rhs } => {
            fold_binary(op, constants.get(&lhs)?, constants.get(&rhs)?)?
        }
        WxInstKind::Compare { op, lhs, rhs } => {
            fold_compare(op, constants.get(&lhs)?, constants.get(&rhs)?)?
        }
        _ => return None,
    };
    Some(WxInst {
        results: vec![result],
        kind: WxInstKind::Constant(constant),
    })
}

fn fold_binary(op: WxBinaryOp, lhs: &WxConstant, rhs: &WxConstant) -> Option<WxConstant> {
    match (op, lhs, rhs) {
        (WxBinaryOp::Integer(WxIntBinaryOp::And), WxConstant::Bool(lhs), WxConstant::Bool(rhs)) => {
            Some(WxConstant::Bool(*lhs && *rhs))
        }
        (WxBinaryOp::Integer(WxIntBinaryOp::Or), WxConstant::Bool(lhs), WxConstant::Bool(rhs)) => {
            Some(WxConstant::Bool(*lhs || *rhs))
        }
        (WxBinaryOp::Integer(WxIntBinaryOp::Xor), WxConstant::Bool(lhs), WxConstant::Bool(rhs)) => {
            Some(WxConstant::Bool(*lhs ^ *rhs))
        }
        _ => None,
    }
}

fn fold_compare(op: WxCompareOp, lhs: &WxConstant, rhs: &WxConstant) -> Option<WxConstant> {
    let value = match (op, lhs, rhs) {
        (WxCompareOp::Integer(op), WxConstant::Int(lhs), WxConstant::Int(rhs)) => {
            compare_integer(op, *lhs, *rhs)
        }
        (
            WxCompareOp::Integer(WxIntCompareOp::Eq),
            WxConstant::Bool(lhs),
            WxConstant::Bool(rhs),
        ) => lhs == rhs,
        (
            WxCompareOp::Integer(WxIntCompareOp::Ne),
            WxConstant::Bool(lhs),
            WxConstant::Bool(rhs),
        ) => lhs != rhs,
        _ => return None,
    };
    Some(WxConstant::Bool(value))
}

fn compare_integer(op: WxIntCompareOp, lhs: i64, rhs: i64) -> bool {
    match op {
        WxIntCompareOp::Eq => lhs == rhs,
        WxIntCompareOp::Ne => lhs != rhs,
        WxIntCompareOp::SignedLt => lhs < rhs,
        WxIntCompareOp::SignedLe => lhs <= rhs,
        WxIntCompareOp::UnsignedLt => {
            u64::from_ne_bytes(lhs.to_ne_bytes()) < u64::from_ne_bytes(rhs.to_ne_bytes())
        }
        WxIntCompareOp::UnsignedLe => {
            u64::from_ne_bytes(lhs.to_ne_bytes()) <= u64::from_ne_bytes(rhs.to_ne_bytes())
        }
    }
}

fn canonical_operands(op: WxBinaryOp, lhs: WxValueId, rhs: WxValueId) -> (WxValueId, WxValueId) {
    if matches!(
        op,
        WxBinaryOp::Integer(
            WxIntBinaryOp::Add
                | WxIntBinaryOp::Mul
                | WxIntBinaryOp::And
                | WxIntBinaryOp::Or
                | WxIntBinaryOp::Xor
        ) | WxBinaryOp::Float(WxFloatBinaryOp::Add | WxFloatBinaryOp::Mul)
    ) && rhs.0 < lhs.0
    {
        (rhs, lhs)
    } else {
        (lhs, rhs)
    }
}

fn canonical_compare_operands(
    op: WxCompareOp,
    lhs: WxValueId,
    rhs: WxValueId,
) -> (WxValueId, WxValueId) {
    if matches!(
        op,
        WxCompareOp::Integer(WxIntCompareOp::Eq | WxIntCompareOp::Ne)
            | WxCompareOp::Float(WxFloatCompareOp::Eq | WxFloatCompareOp::Ne)
    ) && rhs.0 < lhs.0
    {
        (rhs, lhs)
    } else {
        (lhs, rhs)
    }
}

fn binary_code(op: WxBinaryOp) -> u8 {
    match op {
        WxBinaryOp::Integer(WxIntBinaryOp::Add) => 0,
        WxBinaryOp::Integer(WxIntBinaryOp::Sub) => 1,
        WxBinaryOp::Integer(WxIntBinaryOp::Mul) => 2,
        WxBinaryOp::Integer(WxIntBinaryOp::FloorDiv) => 3,
        WxBinaryOp::Integer(WxIntBinaryOp::And) => 4,
        WxBinaryOp::Integer(WxIntBinaryOp::Or) => 5,
        WxBinaryOp::Integer(WxIntBinaryOp::Xor) => 6,
        WxBinaryOp::Float(WxFloatBinaryOp::Add) => 7,
        WxBinaryOp::Float(WxFloatBinaryOp::Sub) => 8,
        WxBinaryOp::Float(WxFloatBinaryOp::Mul) => 9,
        WxBinaryOp::Float(WxFloatBinaryOp::Div) => 10,
    }
}

fn compare_code(op: WxCompareOp) -> u8 {
    match op {
        WxCompareOp::Integer(WxIntCompareOp::Eq) => 0,
        WxCompareOp::Integer(WxIntCompareOp::Ne) => 1,
        WxCompareOp::Integer(WxIntCompareOp::SignedLt) => 2,
        WxCompareOp::Integer(WxIntCompareOp::SignedLe) => 3,
        WxCompareOp::Integer(WxIntCompareOp::UnsignedLt) => 4,
        WxCompareOp::Integer(WxIntCompareOp::UnsignedLe) => 5,
        WxCompareOp::Float(WxFloatCompareOp::Eq) => 6,
        WxCompareOp::Float(WxFloatCompareOp::Ne) => 7,
        WxCompareOp::Float(WxFloatCompareOp::Lt) => 8,
        WxCompareOp::Float(WxFloatCompareOp::Le) => 9,
        WxCompareOp::Float(WxFloatCompareOp::Gt) => 10,
        WxCompareOp::Float(WxFloatCompareOp::Ge) => 11,
    }
}

fn cast_code(op: WxCastOp) -> u8 {
    match op {
        WxCastOp::ZeroExtend => 0,
        WxCastOp::SignExtend => 1,
        WxCastOp::Truncate => 2,
        WxCastOp::IntToFloat { signed: true } => 3,
        WxCastOp::IntToFloat { signed: false } => 4,
        WxCastOp::FloatToInt { signed: true } => 5,
        WxCastOp::FloatToInt { signed: false } => 6,
        WxCastOp::FloatPromote => 7,
        WxCastOp::FloatDemote => 8,
        WxCastOp::PtrToInt => 9,
        WxCastOp::IntToPtr => 10,
        WxCastOp::Bitcast => 11,
    }
}

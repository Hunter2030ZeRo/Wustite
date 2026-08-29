use crate::wxir::{WxIntOverflowOp, WxValueId};

pub(super) fn canonical_checked_operands(
    op: WxIntOverflowOp,
    lhs: WxValueId,
    rhs: WxValueId,
) -> (WxValueId, WxValueId) {
    if matches!(op, WxIntOverflowOp::Add | WxIntOverflowOp::Mul) && rhs.0 < lhs.0 {
        (rhs, lhs)
    } else {
        (lhs, rhs)
    }
}

pub(super) const fn overflow_code(op: WxIntOverflowOp) -> u8 {
    match op {
        WxIntOverflowOp::Add => 0,
        WxIntOverflowOp::Sub => 1,
        WxIntOverflowOp::Mul => 2,
    }
}

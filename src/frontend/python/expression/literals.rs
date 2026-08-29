use std::str::FromStr;

use num_bigint::BigInt;
use rustpython_parser::ast::{self, CmpOp, Constant, Operator};

use super::super::hir::HirExpressionKind;
use super::super::{Compiler, PythonFrontendError, error_at};
use crate::bytecode::{BinaryOperator, CompareOperator};

impl Compiler<'_> {
    pub(super) fn lower_constant(
        &self,
        constant: &ast::ExprConstant,
        negative: bool,
    ) -> Result<HirExpressionKind, PythonFrontendError> {
        match &constant.value {
            Constant::Int(value) => {
                let digits = value.to_string();
                let literal = if negative {
                    format!("-{digits}")
                } else {
                    digits
                };
                if let Ok(value) = literal.parse::<i64>() {
                    Ok(HirExpressionKind::SmallInt(value))
                } else {
                    BigInt::from_str(&literal)
                        .map(HirExpressionKind::BigInt)
                        .map_err(|_| error_at(self.source, constant, "invalid integer literal"))
                }
            }
            Constant::Float(value) if !negative => Ok(HirExpressionKind::Float(*value)),
            Constant::Bool(value) if !negative => Ok(HirExpressionKind::Bool(*value)),
            Constant::Str(value) if !negative => Ok(HirExpressionKind::String(value.clone())),
            Constant::None if !negative => Ok(HirExpressionKind::None),
            Constant::None
            | Constant::Bool(_)
            | Constant::Str(_)
            | Constant::Bytes(_)
            | Constant::Tuple(_)
            | Constant::Float(_)
            | Constant::Complex { .. }
            | Constant::Ellipsis => Err(error_at(
                self.source,
                constant,
                "unsupported Python constant",
            )),
        }
    }
}

pub(super) fn binary_operator(
    source: &str,
    binary: &ast::ExprBinOp,
) -> Result<BinaryOperator, PythonFrontendError> {
    binary_operator_kind(binary.op)
        .ok_or_else(|| error_at(source, binary, "unsupported binary operator"))
}

pub(crate) const fn binary_operator_kind(op: Operator) -> Option<BinaryOperator> {
    match op {
        Operator::Add => Some(BinaryOperator::Add),
        Operator::Sub => Some(BinaryOperator::Subtract),
        Operator::Mult => Some(BinaryOperator::Multiply),
        Operator::Div => Some(BinaryOperator::Divide),
        Operator::FloorDiv => Some(BinaryOperator::FloorDivide),
        Operator::Pow => Some(BinaryOperator::Power),
        _ => None,
    }
}

pub(super) fn compare_operator(
    source: &str,
    compare: &ast::ExprCompare,
) -> Result<CompareOperator, PythonFrontendError> {
    match compare.ops[0] {
        CmpOp::Eq => Ok(CompareOperator::Eq),
        CmpOp::NotEq => Ok(CompareOperator::NotEq),
        CmpOp::Lt => Ok(CompareOperator::Lt),
        CmpOp::LtE => Ok(CompareOperator::Le),
        CmpOp::Gt => Ok(CompareOperator::Gt),
        CmpOp::GtE => Ok(CompareOperator::Ge),
        _ => Err(error_at(source, compare, "unsupported comparison operator")),
    }
}

use std::collections::HashSet;

use rustpython_parser::ast;

use super::hir::{HirExpression, HirExpressionKind};
use super::statements::FunctionScope;
use super::{Compiler, PythonFrontendError, error_at, location_of};

impl Compiler<'_> {
    pub(super) fn lower_range(
        &mut self,
        iterator: &ast::Expr,
        scope: FunctionScope<'_>,
        initialized_names: &HashSet<String>,
    ) -> Result<(HirExpression, HirExpression, i64), PythonFrontendError> {
        let ast::Expr::Call(call) = iterator else {
            return Err(error_at(
                self.source,
                iterator,
                "for iterator must be range(...)",
            ));
        };
        if !call.keywords.is_empty()
            || !matches!(call.func.as_ref(), ast::Expr::Name(name) if name.id.as_str() == "range")
        {
            return Err(error_at(
                self.source,
                iterator,
                "for iterator must be range with one to three positional arguments",
            ));
        }
        let zero = HirExpression {
            kind: HirExpressionKind::SmallInt(0),
            location: location_of(self.source, iterator),
        };
        let (start, stop) = match call.args.as_slice() {
            [stop] => (
                zero,
                self.lower_expression(
                    stop,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )?,
            ),
            [start, stop] | [start, stop, _] => (
                self.lower_expression(
                    start,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )?,
                self.lower_expression(
                    stop,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )?,
            ),
            [] | [_, _, _, ..] => {
                return Err(error_at(
                    self.source,
                    iterator,
                    "for iterator must be range with one to three positional arguments",
                ));
            }
        };
        let step = if let [_, _, step] = call.args.as_slice() {
            match self
                .lower_expression(
                    step,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )?
                .kind
            {
                HirExpressionKind::SmallInt(0) => {
                    return Err(error_at(self.source, step, "range step cannot be zero"));
                }
                HirExpressionKind::SmallInt(value) => value,
                HirExpressionKind::Float(_)
                | HirExpressionKind::Bool(_)
                | HirExpressionKind::None
                | HirExpressionKind::String(_)
                | HirExpressionKind::BigInt(_)
                | HirExpressionKind::Function(_)
                | HirExpressionKind::Class(_)
                | HirExpressionKind::CurrentFunction
                | HirExpressionKind::Name(_)
                | HirExpressionKind::Unary { .. }
                | HirExpressionKind::Binary { .. }
                | HirExpressionKind::Compare { .. }
                | HirExpressionKind::Boolean { .. }
                | HirExpressionKind::Tuple(_)
                | HirExpressionKind::List(_)
                | HirExpressionKind::ListComprehension { .. }
                | HirExpressionKind::Dict(_)
                | HirExpressionKind::GetItem { .. }
                | HirExpressionKind::GetAttr { .. }
                | HirExpressionKind::GetSlice { .. }
                | HirExpressionKind::ListPop { .. }
                | HirExpressionKind::Length(_)
                | HirExpressionKind::Call { .. }
                | HirExpressionKind::CallMethod { .. } => {
                    return Err(error_at(
                        self.source,
                        step,
                        "range step must be a non-zero integer literal",
                    ));
                }
            }
        } else {
            1
        };
        Ok((start, stop, step))
    }
}

pub(super) fn guarantees_iteration(iterator: &ast::Expr) -> bool {
    let ast::Expr::Call(call) = iterator else {
        return false;
    };
    if !call.keywords.is_empty()
        || !matches!(call.func.as_ref(), ast::Expr::Name(name) if name.id.as_str() == "range")
    {
        return false;
    }
    let (start, stop, step) = match call.args.as_slice() {
        [stop] => (Some(0), integer_literal(stop), Some(1)),
        [start, stop] => (integer_literal(start), integer_literal(stop), Some(1)),
        [start, stop, step] => (
            integer_literal(start),
            integer_literal(stop),
            integer_literal(step),
        ),
        _ => return false,
    };
    let (Some(start), Some(stop), Some(step)) = (start, stop, step) else {
        return false;
    };
    (step > 0 && start < stop) || (step < 0 && start > stop)
}

fn integer_literal(expression: &ast::Expr) -> Option<i64> {
    match expression {
        ast::Expr::Constant(constant) => match &constant.value {
            rustpython_parser::ast::Constant::Int(value) => value.to_string().parse().ok(),
            _ => None,
        },
        ast::Expr::UnaryOp(unary) if unary.op == rustpython_parser::ast::UnaryOp::USub => {
            integer_literal(&unary.operand)?.checked_neg()
        }
        _ => None,
    }
}

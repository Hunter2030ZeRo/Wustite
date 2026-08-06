use std::collections::HashSet;

use rustpython_parser::ast::{self, BoolOp, Constant, UnaryOp};

use super::hir::{HirExpression, HirExpressionKind};
use super::{Compiler, PythonFrontendError, error_at, location_of};
use crate::bytecode::{BooleanOperator, UnaryOperator};

mod literals;

use literals::{binary_operator, compare_operator};

impl Compiler<'_> {
    pub(super) fn lower_expression(
        &mut self,
        expression: &ast::Expr,
        current_name: &str,
        local_names: &HashSet<String>,
        initialized_names: &HashSet<String>,
    ) -> Result<HirExpression, PythonFrontendError> {
        let location = location_of(self.source, expression);
        let kind = match expression {
            ast::Expr::Constant(constant) => self.lower_constant(constant, false)?,
            ast::Expr::UnaryOp(unary) if unary.op == UnaryOp::USub => {
                if let ast::Expr::Constant(constant) = unary.operand.as_ref()
                    && matches!(constant.value, Constant::Int(_))
                {
                    self.lower_constant(constant, true)?
                } else {
                    HirExpressionKind::Unary {
                        op: UnaryOperator::Negate,
                        operand: Box::new(self.lower_expression(
                            &unary.operand,
                            current_name,
                            local_names,
                            initialized_names,
                        )?),
                    }
                }
            }
            ast::Expr::UnaryOp(unary) if unary.op == UnaryOp::Not => HirExpressionKind::Unary {
                op: UnaryOperator::Not,
                operand: Box::new(self.lower_expression(
                    &unary.operand,
                    current_name,
                    local_names,
                    initialized_names,
                )?),
            },
            ast::Expr::Name(name)
                if local_names.contains(name.id.as_str())
                    || initialized_names.contains(name.id.as_str()) =>
            {
                HirExpressionKind::Name(name.id.to_string())
            }
            ast::Expr::Name(name) if name.id.as_str() == current_name => {
                HirExpressionKind::CurrentFunction
            }
            ast::Expr::Name(name) if self.has_function(name.id.as_str()) => {
                let function = self.compile_named(name.id.as_str(), Some(name))?;
                HirExpressionKind::Function(Box::new(function))
            }
            ast::Expr::Name(name) => HirExpressionKind::Name(name.id.to_string()),
            ast::Expr::BinOp(binary) => HirExpressionKind::Binary {
                op: binary_operator(self.source, binary)?,
                lhs: Box::new(self.lower_expression(
                    &binary.left,
                    current_name,
                    local_names,
                    initialized_names,
                )?),
                rhs: Box::new(self.lower_expression(
                    &binary.right,
                    current_name,
                    local_names,
                    initialized_names,
                )?),
            },
            ast::Expr::Compare(compare)
                if compare.ops.len() == 1 && compare.comparators.len() == 1 =>
            {
                HirExpressionKind::Compare {
                    op: compare_operator(self.source, compare)?,
                    lhs: Box::new(self.lower_expression(
                        &compare.left,
                        current_name,
                        local_names,
                        initialized_names,
                    )?),
                    rhs: Box::new(self.lower_expression(
                        &compare.comparators[0],
                        current_name,
                        local_names,
                        initialized_names,
                    )?),
                }
            }
            ast::Expr::Compare(compare) => {
                return Err(error_at(
                    self.source,
                    compare,
                    "chained comparisons are unsupported",
                ));
            }
            ast::Expr::BoolOp(boolean) => HirExpressionKind::Boolean {
                op: match boolean.op {
                    BoolOp::And => BooleanOperator::And,
                    BoolOp::Or => BooleanOperator::Or,
                },
                values: boolean
                    .values
                    .iter()
                    .map(|value| {
                        self.lower_expression(value, current_name, local_names, initialized_names)
                    })
                    .collect::<Result<_, _>>()?,
            },
            ast::Expr::Tuple(tuple) => HirExpressionKind::Tuple(self.lower_items(
                &tuple.elts,
                current_name,
                local_names,
                initialized_names,
            )?),
            ast::Expr::List(list) => HirExpressionKind::List(self.lower_items(
                &list.elts,
                current_name,
                local_names,
                initialized_names,
            )?),
            ast::Expr::Dict(dict) => HirExpressionKind::Dict(
                dict.keys
                    .iter()
                    .zip(&dict.values)
                    .map(|(key, value)| {
                        let key = key.as_ref().ok_or_else(|| {
                            error_at(self.source, dict, "dictionary unpacking is unsupported")
                        })?;
                        Ok((
                            self.lower_expression(
                                key,
                                current_name,
                                local_names,
                                initialized_names,
                            )?,
                            self.lower_expression(
                                value,
                                current_name,
                                local_names,
                                initialized_names,
                            )?,
                        ))
                    })
                    .collect::<Result<_, PythonFrontendError>>()?,
            ),
            ast::Expr::Subscript(subscript) => HirExpressionKind::GetItem {
                object: Box::new(self.lower_expression(
                    &subscript.value,
                    current_name,
                    local_names,
                    initialized_names,
                )?),
                key: Box::new(self.lower_expression(
                    &subscript.slice,
                    current_name,
                    local_names,
                    initialized_names,
                )?),
            },
            ast::Expr::Call(call) => {
                self.lower_call(call, current_name, local_names, initialized_names)?
            }
            _ => {
                return Err(error_at(
                    self.source,
                    expression,
                    "unsupported Python expression",
                ));
            }
        };
        Ok(HirExpression { kind, location })
    }

    fn lower_items(
        &mut self,
        items: &[ast::Expr],
        current_name: &str,
        local_names: &HashSet<String>,
        initialized_names: &HashSet<String>,
    ) -> Result<Vec<HirExpression>, PythonFrontendError> {
        items
            .iter()
            .map(|item| self.lower_expression(item, current_name, local_names, initialized_names))
            .collect()
    }

    fn lower_call(
        &mut self,
        call: &ast::ExprCall,
        current_name: &str,
        local_names: &HashSet<String>,
        initialized_names: &HashSet<String>,
    ) -> Result<HirExpressionKind, PythonFrontendError> {
        if !call.keywords.is_empty() {
            return Err(error_at(
                self.source,
                call,
                "keyword arguments are unsupported",
            ));
        }
        if matches!(call.func.as_ref(), ast::Expr::Name(name) if name.id.as_str() == "len") {
            if call.args.len() != 1 {
                return Err(error_at(
                    self.source,
                    call,
                    "len requires exactly one argument",
                ));
            }
            return Ok(HirExpressionKind::Length(Box::new(self.lower_expression(
                &call.args[0],
                current_name,
                local_names,
                initialized_names,
            )?)));
        }
        Ok(HirExpressionKind::Call {
            callable: Box::new(self.lower_expression(
                &call.func,
                current_name,
                local_names,
                initialized_names,
            )?),
            args: self.lower_items(&call.args, current_name, local_names, initialized_names)?,
        })
    }
}

use std::collections::HashSet;

use rustpython_parser::ast;

use super::FunctionScope;
use super::iterators::{lower_target, target_names};
use crate::frontend::python::expression::literals::binary_operator_kind;
use crate::frontend::python::hir::{HirExpression, HirExpressionKind, HirStatementKind, HirTarget};
use crate::frontend::python::{
    Compiler, PythonFrontendError, SourceLocation, error_at, location_of,
};

impl Compiler<'_> {
    pub(super) fn lower_assignment(
        &mut self,
        assign: &ast::StmtAssign,
        scope: FunctionScope<'_>,
        initialized_names: &mut HashSet<String>,
    ) -> Result<HirStatementKind, PythonFrontendError> {
        if assign.targets.len() != 1 || assign.type_comment.is_some() {
            return Err(error_at(
                self.source,
                assign,
                "only a single unannotated assignment target is supported",
            ));
        }
        let value = self.lower_expression(
            &assign.value,
            scope.current_name,
            scope.local_names,
            initialized_names,
        )?;
        if let ast::Expr::Subscript(subscript) = &assign.targets[0] {
            self.lower_subscript_assignment(subscript, value, scope, initialized_names)
        } else if let ast::Expr::Attribute(attribute) = &assign.targets[0] {
            Ok(HirStatementKind::SetAttr {
                object: self.lower_expression(
                    &attribute.value,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )?,
                name: attribute.attr.to_string(),
                value,
            })
        } else {
            let names = target_names(self.source, &assign.targets[0], "assignment")?;
            let target = lower_target(self.source, &assign.targets[0], "assignment")?;
            initialized_names.extend(names);
            Ok(HirStatementKind::Assign { target, value })
        }
    }

    pub(super) fn lower_augmented_assignment(
        &mut self,
        assign: &ast::StmtAugAssign,
        scope: FunctionScope<'_>,
        initialized_names: &mut HashSet<String>,
        location: SourceLocation,
    ) -> Result<HirStatementKind, PythonFrontendError> {
        if let ast::Expr::Subscript(subscript) = assign.target.as_ref() {
            let op = binary_operator_kind(assign.op).ok_or_else(|| {
                error_at(
                    self.source,
                    assign,
                    "unsupported augmented assignment operator",
                )
            })?;
            let (object, key) = self.lower_subscript_parts(
                subscript,
                scope,
                initialized_names,
                "augmented slice assignment is unsupported",
            )?;
            let value = self.lower_expression(
                &assign.value,
                scope.current_name,
                scope.local_names,
                initialized_names,
            )?;
            Ok(HirStatementKind::AugSetItem {
                object,
                key,
                op,
                value,
            })
        } else {
            let names = target_names(self.source, &assign.target, "augmented assignment")?;
            let [name] = names.as_slice() else {
                return Err(error_at(
                    self.source,
                    assign.target.as_ref(),
                    "augmented assignment target must be a local name",
                ));
            };
            if !initialized_names.contains(name) {
                return Err(error_at(
                    self.source,
                    assign.target.as_ref(),
                    format!("name `{name}` is used before assignment"),
                ));
            }
            let op = binary_operator_kind(assign.op).ok_or_else(|| {
                error_at(
                    self.source,
                    assign,
                    "unsupported augmented assignment operator",
                )
            })?;
            let lhs = HirExpression {
                kind: HirExpressionKind::Name(name.clone()),
                location: location_of(self.source, assign.target.as_ref()),
            };
            let rhs = self.lower_expression(
                &assign.value,
                scope.current_name,
                scope.local_names,
                initialized_names,
            )?;
            Ok(HirStatementKind::Assign {
                target: HirTarget::Name(name.clone()),
                value: HirExpression {
                    kind: HirExpressionKind::Binary {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    location,
                },
            })
        }
    }
}

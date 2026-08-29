use std::collections::HashSet;

use rustpython_parser::ast;

use super::FunctionScope;
use crate::frontend::python::hir::{HirExpression, HirStatementKind};
use crate::frontend::python::{Compiler, PythonFrontendError, error_at};

impl Compiler<'_> {
    pub(super) fn lower_subscript_assignment(
        &mut self,
        subscript: &ast::ExprSubscript,
        value: HirExpression,
        scope: FunctionScope<'_>,
        initialized_names: &HashSet<String>,
    ) -> Result<HirStatementKind, PythonFrontendError> {
        let object = self.lower_expression(
            &subscript.value,
            scope.current_name,
            scope.local_names,
            initialized_names,
        )?;
        if let ast::Expr::Slice(slice) = subscript.slice.as_ref() {
            let (start, stop, step) = self.lower_slice_parts(
                slice,
                scope.current_name,
                scope.local_names,
                initialized_names,
            )?;
            Ok(HirStatementKind::SetSlice {
                object,
                start,
                stop,
                step,
                value,
            })
        } else {
            let key = self.lower_expression(
                &subscript.slice,
                scope.current_name,
                scope.local_names,
                initialized_names,
            )?;
            Ok(HirStatementKind::SetItem { object, key, value })
        }
    }

    pub(super) fn lower_subscript_parts(
        &mut self,
        subscript: &ast::ExprSubscript,
        scope: FunctionScope<'_>,
        initialized_names: &HashSet<String>,
        slice_error: &str,
    ) -> Result<(HirExpression, HirExpression), PythonFrontendError> {
        if matches!(subscript.slice.as_ref(), ast::Expr::Slice(_)) {
            return Err(error_at(self.source, subscript, slice_error));
        }
        Ok((
            self.lower_expression(
                &subscript.value,
                scope.current_name,
                scope.local_names,
                initialized_names,
            )?,
            self.lower_expression(
                &subscript.slice,
                scope.current_name,
                scope.local_names,
                initialized_names,
            )?,
        ))
    }

    pub(super) fn lower_expression_statement(
        &mut self,
        expression: &ast::Expr,
        scope: FunctionScope<'_>,
        initialized_names: &HashSet<String>,
    ) -> Result<HirStatementKind, PythonFrontendError> {
        if let ast::Expr::Call(call) = expression
            && let ast::Expr::Attribute(attribute) = call.func.as_ref()
            && call.keywords.is_empty()
        {
            let list = self.lower_expression(
                &attribute.value,
                scope.current_name,
                scope.local_names,
                initialized_names,
            )?;
            match (attribute.attr.as_str(), call.args.as_slice()) {
                ("append", [value]) => {
                    return Ok(HirStatementKind::ListAppend {
                        list,
                        value: self.lower_expression(
                            value,
                            scope.current_name,
                            scope.local_names,
                            initialized_names,
                        )?,
                    });
                }
                ("insert", [index, value]) => {
                    return Ok(HirStatementKind::ListInsert {
                        list,
                        index: self.lower_expression(
                            index,
                            scope.current_name,
                            scope.local_names,
                            initialized_names,
                        )?,
                        value: self.lower_expression(
                            value,
                            scope.current_name,
                            scope.local_names,
                            initialized_names,
                        )?,
                    });
                }
                _ => {}
            }
        }
        Ok(HirStatementKind::Expression(self.lower_expression(
            expression,
            scope.current_name,
            scope.local_names,
            initialized_names,
        )?))
    }
}

use std::collections::HashSet;

use rustpython_parser::ast;

use super::hir::{HirStatement, HirStatementKind, HirTarget};
use super::{Compiler, PythonFrontendError, error_at, location_of};

mod assignment;
pub(super) mod iterators;
mod mutation;

use iterators::{is_named_call, lower_target, target_names};

#[derive(Clone, Copy)]
pub(super) struct FunctionScope<'a> {
    pub current_name: &'a str,
    pub local_names: &'a HashSet<String>,
}

impl Compiler<'_> {
    pub(super) fn lower_statements(
        &mut self,
        statements: &[ast::Stmt],
        scope: FunctionScope<'_>,
        initialized_names: &mut HashSet<String>,
    ) -> Result<Vec<HirStatement>, PythonFrontendError> {
        let mut lowered = Vec::with_capacity(statements.len());
        for statement in statements {
            lowered.push(self.lower_statement(statement, scope, initialized_names)?);
        }
        Ok(lowered)
    }

    fn lower_statement(
        &mut self,
        statement: &ast::Stmt,
        scope: FunctionScope<'_>,
        initialized_names: &mut HashSet<String>,
    ) -> Result<HirStatement, PythonFrontendError> {
        let location = location_of(self.source, statement);
        let kind = match statement {
            ast::Stmt::Assign(assign) => self.lower_assignment(assign, scope, initialized_names)?,
            ast::Stmt::AugAssign(assign) => {
                self.lower_augmented_assignment(assign, scope, initialized_names, location)?
            }
            ast::Stmt::While(while_statement) => {
                let condition = self.lower_expression(
                    &while_statement.test,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )?;
                let mut body_names = initialized_names.clone();
                HirStatementKind::While {
                    condition,
                    body: self.lower_statements(&while_statement.body, scope, &mut body_names)?,
                    orelse: self.lower_statements(
                        &while_statement.orelse,
                        scope,
                        initialized_names,
                    )?,
                }
            }
            ast::Stmt::If(if_statement) => {
                let condition = self.lower_expression(
                    &if_statement.test,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )?;
                let mut body_names = initialized_names.clone();
                let body = self.lower_statements(&if_statement.body, scope, &mut body_names)?;
                let mut orelse_names = initialized_names.clone();
                let orelse =
                    self.lower_statements(&if_statement.orelse, scope, &mut orelse_names)?;
                initialized_names
                    .retain(|name| body_names.contains(name) && orelse_names.contains(name));
                HirStatementKind::If {
                    condition,
                    body,
                    orelse,
                }
            }
            ast::Stmt::For(for_statement) => {
                if for_statement.type_comment.is_some() {
                    return Err(error_at(
                        self.source,
                        for_statement,
                        "for type comments are unsupported",
                    ));
                }
                let names = target_names(self.source, &for_statement.target, "for")?;
                let target = lower_target(self.source, &for_statement.target, "for")?;
                if is_named_call(&for_statement.iter, "range") {
                    let [target_name] = names.as_slice() else {
                        return Err(error_at(
                            self.source,
                            for_statement.target.as_ref(),
                            "for range target must be a local name",
                        ));
                    };
                    let (start, stop, step) =
                        self.lower_range(&for_statement.iter, scope, initialized_names)?;
                    let guarantees_iteration =
                        super::range::guarantees_iteration(&for_statement.iter);
                    initialized_names.insert(target_name.clone());
                    let mut body_names = initialized_names.clone();
                    let body =
                        self.lower_statements(&for_statement.body, scope, &mut body_names)?;
                    if guarantees_iteration {
                        initialized_names.extend(body_names);
                    }
                    HirStatementKind::ForRange {
                        target: target_name.clone(),
                        start,
                        stop,
                        step,
                        guaranteed_non_empty: guarantees_iteration,
                        body,
                        orelse: self.lower_statements(
                            &for_statement.orelse,
                            scope,
                            initialized_names,
                        )?,
                    }
                } else {
                    let targets = if is_named_call(&for_statement.iter, "enumerate")
                        || is_named_call(&for_statement.iter, "zip")
                    {
                        let HirTarget::Tuple(items) = target else {
                            return Err(error_at(
                                self.source,
                                for_statement.target.as_ref(),
                                "enumerate and zip require tuple targets",
                            ));
                        };
                        items
                    } else {
                        vec![target]
                    };
                    let (iterables, include_index) = self.lower_sequence_iterables(
                        &for_statement.iter,
                        &targets,
                        scope,
                        initialized_names,
                    )?;
                    initialized_names.extend(names);
                    let mut body_names = initialized_names.clone();
                    HirStatementKind::ForSequence {
                        targets,
                        iterables,
                        include_index,
                        body: self.lower_statements(&for_statement.body, scope, &mut body_names)?,
                        orelse: self.lower_statements(
                            &for_statement.orelse,
                            scope,
                            initialized_names,
                        )?,
                    }
                }
            }
            ast::Stmt::Expr(expression) => {
                self.lower_expression_statement(&expression.value, scope, initialized_names)?
            }
            ast::Stmt::Break(_) => HirStatementKind::Break,
            ast::Stmt::Return(return_statement) => {
                let value = return_statement.value.as_ref().ok_or_else(|| {
                    error_at(self.source, return_statement, "return must include a value")
                })?;
                HirStatementKind::Return(self.lower_expression(
                    value,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )?)
            }
            _ => {
                return Err(error_at(
                    self.source,
                    statement,
                    "unsupported Python statement",
                ));
            }
        };
        Ok(HirStatement { kind, location })
    }
}

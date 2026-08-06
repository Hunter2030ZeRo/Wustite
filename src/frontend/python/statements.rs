use std::collections::HashSet;

use rustpython_parser::ast;

use super::hir::{HirStatement, HirStatementKind};
use super::{Compiler, PythonFrontendError, error_at, location_of};

impl Compiler<'_> {
    pub(super) fn lower_statements(
        &mut self,
        statements: &[ast::Stmt],
        in_loop: bool,
        current_name: &str,
        local_names: &HashSet<String>,
        initialized_names: &mut HashSet<String>,
    ) -> Result<Vec<HirStatement>, PythonFrontendError> {
        let mut lowered = Vec::with_capacity(statements.len());
        for statement in statements {
            lowered.push(self.lower_statement(
                statement,
                in_loop,
                current_name,
                local_names,
                initialized_names,
            )?);
        }
        Ok(lowered)
    }

    fn lower_statement(
        &mut self,
        statement: &ast::Stmt,
        in_loop: bool,
        current_name: &str,
        local_names: &HashSet<String>,
        initialized_names: &mut HashSet<String>,
    ) -> Result<HirStatement, PythonFrontendError> {
        let location = location_of(self.source, statement);
        let kind = match statement {
            ast::Stmt::Assign(assign) => {
                if assign.targets.len() != 1 || assign.type_comment.is_some() {
                    return Err(error_at(
                        self.source,
                        assign,
                        "only a single unannotated assignment target is supported",
                    ));
                }
                let ast::Expr::Name(target) = &assign.targets[0] else {
                    return Err(error_at(
                        self.source,
                        &assign.targets[0],
                        "assignment target must be a local name",
                    ));
                };
                let name = target.id.to_string();
                let value = self.lower_expression(
                    &assign.value,
                    current_name,
                    local_names,
                    initialized_names,
                )?;
                initialized_names.insert(name.clone());
                HirStatementKind::Assign { name, value }
            }
            ast::Stmt::While(while_statement) => {
                if in_loop {
                    return Err(error_at(
                        self.source,
                        while_statement,
                        "nested while loops are unsupported",
                    ));
                }
                if !while_statement.orelse.is_empty() {
                    return Err(error_at(
                        self.source,
                        while_statement,
                        "while else is unsupported",
                    ));
                }
                let condition = self.lower_expression(
                    &while_statement.test,
                    current_name,
                    local_names,
                    initialized_names,
                )?;
                let mut body_names = initialized_names.clone();
                HirStatementKind::While {
                    condition,
                    body: self.lower_statements(
                        &while_statement.body,
                        true,
                        current_name,
                        local_names,
                        &mut body_names,
                    )?,
                }
            }
            ast::Stmt::Return(return_statement) if !in_loop => {
                let value = return_statement.value.as_ref().ok_or_else(|| {
                    error_at(self.source, return_statement, "return must include a value")
                })?;
                HirStatementKind::Return(self.lower_expression(
                    value,
                    current_name,
                    local_names,
                    initialized_names,
                )?)
            }
            ast::Stmt::Return(return_statement) => {
                return Err(error_at(
                    self.source,
                    return_statement,
                    "return inside a while loop is unsupported",
                ));
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

use std::collections::HashSet;

use rustpython_parser::ast;

use super::FunctionScope;
use crate::frontend::python::hir::{HirExpression, HirTarget};
use crate::frontend::python::{Compiler, PythonFrontendError, error_at};

impl Compiler<'_> {
    pub(super) fn lower_sequence_iterables(
        &mut self,
        iterator: &ast::Expr,
        targets: &[HirTarget],
        scope: FunctionScope<'_>,
        initialized_names: &HashSet<String>,
    ) -> Result<(Vec<HirExpression>, bool), PythonFrontendError> {
        let ast::Expr::Call(call) = iterator else {
            if targets.len() != 1 {
                return Err(error_at(
                    self.source,
                    iterator,
                    "direct iteration requires exactly one target",
                ));
            }
            return self
                .lower_expression(
                    iterator,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )
                .map(|iterable| (vec![iterable], false));
        };
        if !call.keywords.is_empty() {
            return Err(error_at(
                self.source,
                call,
                "for iterator does not support keyword arguments",
            ));
        }
        let include_index = is_named_call(iterator, "enumerate");
        let is_zip = is_named_call(iterator, "zip");
        let valid_arity = (include_index && call.args.len() == 1 && targets.len() == 2)
            || (is_zip && !call.args.is_empty() && call.args.len() == targets.len());
        if !valid_arity {
            return Err(error_at(
                self.source,
                call,
                "enumerate requires two targets and one iterable; zip requires one target per iterable",
            ));
        }
        let iterables = call
            .args
            .iter()
            .map(|expression| {
                self.lower_expression(
                    expression,
                    scope.current_name,
                    scope.local_names,
                    initialized_names,
                )
            })
            .collect::<Result<_, _>>()?;
        Ok((iterables, include_index))
    }
}

pub(super) fn target_names(
    source: &str,
    target: &ast::Expr,
    context: &str,
) -> Result<Vec<String>, PythonFrontendError> {
    let target = lower_target(source, target, context)?;
    let mut names = Vec::new();
    collect_names(&target, &mut names);
    Ok(names)
}

pub(super) fn lower_target(
    source: &str,
    target: &ast::Expr,
    context: &str,
) -> Result<HirTarget, PythonFrontendError> {
    match target {
        ast::Expr::Name(target) => Ok(HirTarget::Name(target.id.to_string())),
        ast::Expr::Tuple(tuple) if !tuple.elts.is_empty() => Ok(HirTarget::Tuple(
            tuple
                .elts
                .iter()
                .map(|element| lower_target(source, element, context))
                .collect::<Result<_, _>>()?,
        )),
        _ => Err(error_at(
            source,
            target,
            format!("{context} target must be a local name or tuple of local names"),
        )),
    }
}

fn collect_names(target: &HirTarget, names: &mut Vec<String>) {
    match target {
        HirTarget::Name(name) => names.push(name.clone()),
        HirTarget::Tuple(items) => {
            for item in items {
                collect_names(item, names);
            }
        }
    }
}

pub(crate) fn is_named_call(expression: &ast::Expr, expected: &str) -> bool {
    matches!(
        expression,
        ast::Expr::Call(call)
            if matches!(call.func.as_ref(), ast::Expr::Name(name) if name.id.as_str() == expected)
    )
}

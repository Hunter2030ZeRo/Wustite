use std::collections::HashSet;

use rustpython_parser::ast;

use super::super::hir::{HirComprehensionIterator, HirExpressionKind};
use super::super::statements::FunctionScope;
use super::super::{Compiler, PythonFrontendError, error_at};

pub(super) struct ComprehensionScope<'a> {
    pub current_name: &'a str,
    pub local_names: &'a HashSet<String>,
    pub initialized_names: &'a HashSet<String>,
}

impl Compiler<'_> {
    pub(super) fn lower_list_comprehension(
        &mut self,
        comprehension: &ast::ExprListComp,
        scope: ComprehensionScope<'_>,
    ) -> Result<HirExpressionKind, PythonFrontendError> {
        let [generator] = comprehension.generators.as_slice() else {
            return Err(error_at(
                self.source,
                comprehension,
                "list comprehension requires exactly one generator",
            ));
        };
        if generator.is_async {
            return Err(error_at(
                self.source,
                comprehension,
                "async list comprehensions are unsupported",
            ));
        }
        if !generator.ifs.is_empty() {
            return Err(error_at(
                self.source,
                &generator.ifs[0],
                "list comprehension filters are unsupported",
            ));
        }
        let ast::Expr::Name(target) = &generator.target else {
            return Err(error_at(
                self.source,
                &generator.target,
                "list comprehension target must be a local name",
            ));
        };
        let iterator = if matches!(
            &generator.iter,
            ast::Expr::Call(call)
                if matches!(call.func.as_ref(), ast::Expr::Name(name) if name.id.as_str() == "range")
        ) {
            let (start, stop, step) = self.lower_range(
                &generator.iter,
                FunctionScope {
                    current_name: scope.current_name,
                    local_names: scope.local_names,
                },
                scope.initialized_names,
            )?;
            HirComprehensionIterator::Range {
                start: Box::new(start),
                stop: Box::new(stop),
                step,
            }
        } else {
            HirComprehensionIterator::Iterable(Box::new(self.lower_expression(
                &generator.iter,
                scope.current_name,
                scope.local_names,
                scope.initialized_names,
            )?))
        };
        let target = target.id.to_string();
        let mut comprehension_names = scope.initialized_names.clone();
        comprehension_names.insert(target.clone());
        let element = self.lower_expression(
            &comprehension.elt,
            scope.current_name,
            scope.local_names,
            &comprehension_names,
        )?;
        Ok(HirExpressionKind::ListComprehension {
            element: Box::new(element),
            target,
            iterator,
        })
    }
}

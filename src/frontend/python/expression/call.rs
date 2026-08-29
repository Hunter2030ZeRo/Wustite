use std::collections::HashSet;

use rustpython_parser::ast;

use super::{
    Compiler, HirExpression, HirExpressionKind, PythonFrontendError, error_at, location_of,
};

impl Compiler<'_> {
    pub(super) fn lower_call(
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
            let argument = match &call.args[0] {
                ast::Expr::Call(copy)
                    if copy.keywords.is_empty()
                        && copy.args.len() == 1
                        && matches!(copy.func.as_ref(), ast::Expr::Name(name) if name.id.as_str() == "list")
                        && matches!(&copy.args[0], ast::Expr::Name(name) if self.exact_list_parameter(current_name, name.id.as_str())) =>
                {
                    &copy.args[0]
                }
                argument => argument,
            };
            return Ok(HirExpressionKind::Length(Box::new(self.lower_expression(
                argument,
                current_name,
                local_names,
                initialized_names,
            )?)));
        }
        if matches!(call.func.as_ref(), ast::Expr::Name(name) if name.id.as_str() == "list") {
            if call.args.len() != 1 {
                return Err(error_at(
                    self.source,
                    call,
                    "list requires exactly one argument",
                ));
            }
            let target = "\0list-item".to_string();
            let element = HirExpression {
                kind: HirExpressionKind::Name(target.clone()),
                location: location_of(self.source, call),
            };
            let iterator = if super::statements::iterators::is_named_call(&call.args[0], "range") {
                let (start, stop, step) = self.lower_range(
                    &call.args[0],
                    super::statements::FunctionScope {
                        current_name,
                        local_names,
                    },
                    initialized_names,
                )?;
                super::hir::HirComprehensionIterator::Range {
                    start: Box::new(start),
                    stop: Box::new(stop),
                    step,
                }
            } else {
                super::hir::HirComprehensionIterator::Iterable(Box::new(self.lower_expression(
                    &call.args[0],
                    current_name,
                    local_names,
                    initialized_names,
                )?))
            };
            return Ok(HirExpressionKind::ListComprehension {
                element: Box::new(element),
                target,
                iterator,
            });
        }
        if let ast::Expr::Attribute(attribute) = call.func.as_ref()
            && attribute.attr.as_str() == "pop"
        {
            if !call.keywords.is_empty() || call.args.len() > 1 {
                return Err(error_at(
                    self.source,
                    call,
                    "list pop accepts at most one positional argument",
                ));
            }
            let index = if let Some(index) = call.args.first() {
                self.lower_expression(index, current_name, local_names, initialized_names)?
            } else {
                HirExpression {
                    kind: HirExpressionKind::SmallInt(-1),
                    location: location_of(self.source, call),
                }
            };
            return Ok(HirExpressionKind::ListPop {
                list: Box::new(self.lower_expression(
                    &attribute.value,
                    current_name,
                    local_names,
                    initialized_names,
                )?),
                index: Box::new(index),
            });
        }
        if let ast::Expr::Attribute(attribute) = call.func.as_ref() {
            return Ok(HirExpressionKind::CallMethod {
                receiver: Box::new(self.lower_expression(
                    &attribute.value,
                    current_name,
                    local_names,
                    initialized_names,
                )?),
                name: attribute.attr.to_string(),
                args: self.lower_items(&call.args, current_name, local_names, initialized_names)?,
            });
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

    fn exact_list_parameter(&self, current_name: &str, name: &str) -> bool {
        self.find_function(current_name).is_ok_and(|function| {
            function
                .args
                .posonlyargs
                .iter()
                .chain(&function.args.args)
                .any(|argument| {
                    argument.def.arg.as_str() == name
                        && matches!(
                            argument.def.annotation.as_deref(),
                            Some(ast::Expr::Name(annotation)) if annotation.id.as_str() == "list"
                        )
                })
        })
    }
}

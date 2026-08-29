use std::collections::HashSet;

use rustpython_parser::ast;

use crate::frontend::python::hir::HirExpression;
use crate::frontend::python::{Compiler, PythonFrontendError};

type SliceParts = (
    Option<HirExpression>,
    Option<HirExpression>,
    Option<HirExpression>,
);

impl Compiler<'_> {
    pub(crate) fn lower_slice_parts(
        &mut self,
        slice: &ast::ExprSlice,
        current_name: &str,
        local_names: &HashSet<String>,
        initialized_names: &HashSet<String>,
    ) -> Result<SliceParts, PythonFrontendError> {
        let lower = match &slice.lower {
            Some(value) => {
                Some(self.lower_expression(value, current_name, local_names, initialized_names)?)
            }
            None => None,
        };
        let upper = match &slice.upper {
            Some(value) => {
                Some(self.lower_expression(value, current_name, local_names, initialized_names)?)
            }
            None => None,
        };
        let step = match &slice.step {
            Some(value) => {
                Some(self.lower_expression(value, current_name, local_names, initialized_names)?)
            }
            None => None,
        };
        Ok((lower, upper, step))
    }
}

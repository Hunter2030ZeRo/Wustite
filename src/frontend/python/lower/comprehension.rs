use crate::bytecode::{Instruction, Register};
use crate::frontend::python::hir::{HirComprehensionIterator, HirExpression, HirExpressionKind};
use crate::frontend::python::{PythonFrontendError, SourceLocation};
use crate::object::ObjectKind;
use crate::structure_map::SlotType;

use super::Lowerer;

pub(super) enum ComprehensionLoopIterator<'a> {
    Range {
        start: &'a HirExpression,
        stop: &'a HirExpression,
        step: i64,
    },
    Sequence(&'a HirExpression),
}

pub(super) struct ComprehensionLoop<'a> {
    pub target: &'a str,
    pub iterator: ComprehensionLoopIterator<'a>,
    pub element: &'a HirExpression,
    pub result: Register,
    pub location: SourceLocation,
}

impl Lowerer {
    pub(super) fn lower_list_comprehension(
        &mut self,
        expression: &HirExpression,
    ) -> Result<(Register, SlotType), PythonFrontendError> {
        let HirExpressionKind::ListComprehension {
            element,
            target,
            iterator,
        } = &expression.kind
        else {
            return Err(PythonFrontendError::new(
                "internal error lowering non-comprehension as list comprehension",
                Some(expression.location),
            ));
        };
        let result = self.allocate_register(expression.location)?;
        self.code.push(Instruction::BuildList {
            dst: result,
            items: Vec::new(),
        });
        let iterator = match iterator {
            HirComprehensionIterator::Range { start, stop, step } => {
                ComprehensionLoopIterator::Range {
                    start,
                    stop,
                    step: *step,
                }
            }
            HirComprehensionIterator::Iterable(iterable) => {
                ComprehensionLoopIterator::Sequence(iterable)
            }
        };
        self.lower_comprehension_loop(ComprehensionLoop {
            target,
            iterator,
            element,
            result,
            location: expression.location,
        })?;
        Ok((result, SlotType::Object(ObjectKind::List)))
    }

    pub(super) fn accumulate_comprehension_value(
        &mut self,
        result: Register,
        element: &HirExpression,
    ) -> Result<(), PythonFrontendError> {
        let (value, _) = self.lower_expression(element)?;
        self.code.push(Instruction::ListAppend {
            list: result,
            value,
        });
        Ok(())
    }
}

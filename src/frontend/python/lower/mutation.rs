use crate::bytecode::Instruction;
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::{HirStatement, HirStatementKind};
use crate::structure_map::TypeFact;

use super::Lowerer;

impl Lowerer {
    pub(super) fn lower_mutation(
        &mut self,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        match &statement.kind {
            HirStatementKind::SetAttr {
                object,
                name,
                value,
            } => {
                let (object, _) = self.lower_expression(object)?;
                let (value, _) = self.lower_expression(value)?;
                self.code.push(Instruction::SetAttr {
                    object,
                    name: name.clone(),
                    value,
                });
            }
            HirStatementKind::SetItem { object, key, value } => {
                let (object, _) = self.lower_expression(object)?;
                let (key, _) = self.lower_expression(key)?;
                let (value, _) = self.lower_expression(value)?;
                self.code.push(Instruction::SetItem { object, key, value });
            }
            HirStatementKind::SetSlice {
                object,
                start,
                stop,
                step,
                value,
            } => {
                let (object, _) = self.lower_expression(object)?;
                let start = self.lower_optional_expression(start.as_ref())?;
                let stop = self.lower_optional_expression(stop.as_ref())?;
                let step = self.lower_optional_expression(step.as_ref())?;
                let (value, _) = self.lower_expression(value)?;
                self.code.push(Instruction::SetSlice {
                    object,
                    start,
                    stop,
                    step,
                    value,
                });
            }
            HirStatementKind::AugSetItem {
                object,
                key,
                op,
                value,
            } => {
                let (object, _) = self.lower_expression(object)?;
                let (key, _) = self.lower_expression(key)?;
                let current = self.allocate_register(statement.location)?;
                self.code.push(Instruction::GetItem {
                    dst: current,
                    object,
                    key,
                });
                let (rhs, _) = self.lower_expression(value)?;
                let result = self.allocate_register(statement.location)?;
                let site = self
                    .map_builder
                    .record_operation(
                        self.code.len(),
                        TypeFact::Unknown,
                        TypeFact::Unknown,
                        TypeFact::Unknown,
                    )
                    .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))?;
                self.code.push(Instruction::BinaryOp {
                    dst: result,
                    op: *op,
                    lhs: current,
                    rhs,
                    site,
                });
                self.code.push(Instruction::SetItem {
                    object,
                    key,
                    value: result,
                });
            }
            HirStatementKind::ListAppend { list, value } => {
                let (list, _) = self.lower_expression(list)?;
                let (value, _) = self.lower_expression(value)?;
                self.code.push(Instruction::ListAppend { list, value });
            }
            HirStatementKind::ListInsert { list, index, value } => {
                let (list, _) = self.lower_expression(list)?;
                let (index, _) = self.lower_expression(index)?;
                let (value, _) = self.lower_expression(value)?;
                self.code
                    .push(Instruction::ListInsert { list, index, value });
            }
            HirStatementKind::Expression(expression) => {
                self.lower_expression(expression)?;
            }
            _ => {
                return Err(PythonFrontendError::new(
                    "internal error lowering non-mutation statement",
                    Some(statement.location),
                ));
            }
        }
        Ok(())
    }
}

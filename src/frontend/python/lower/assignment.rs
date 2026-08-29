use crate::bytecode::{Instruction, Register};
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::{HirExpression, HirStatement, HirTarget};
use crate::structure_map::SlotType;

use super::{Lowerer, Variable};

impl Lowerer {
    pub(super) fn lower_assignment(
        &mut self,
        target: &HirTarget,
        value: &HirExpression,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        if let HirTarget::Name(name) = target {
            let variable = if let Some(variable) = self.variables.get(name).copied() {
                variable
            } else {
                let variable = Variable {
                    register: self.allocate_register(statement.location)?,
                    ty: None,
                };
                self.variable_order.push(name.clone());
                variable
            };
            let (src, ty) = self.lower_expression(value)?;
            return self.store_variable(name, variable, src, ty, statement);
        }
        let (src, ty) = self.lower_expression(value)?;
        self.bind_target(target, src, ty, statement)
    }

    pub(super) fn bind_target(
        &mut self,
        target: &HirTarget,
        src: Register,
        ty: SlotType,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        match target {
            HirTarget::Name(name) => self.bind_register(name, src, ty, statement),
            HirTarget::Tuple(items) => {
                for (index, item) in items.iter().enumerate() {
                    let index = i64::try_from(index).map_err(|_| {
                        PythonFrontendError::new(
                            "unpacking target contains too many items",
                            Some(statement.location),
                        )
                    })?;
                    let key = self.allocate_register(statement.location)?;
                    self.code.push(Instruction::ConstSmallInt {
                        dst: key,
                        value: index,
                    });
                    let value = self.allocate_register(statement.location)?;
                    self.code.push(Instruction::GetItem {
                        dst: value,
                        object: src,
                        key,
                    });
                    self.bind_target(item, value, SlotType::Any, statement)?;
                }
                Ok(())
            }
        }
    }

    pub(super) fn bind_register(
        &mut self,
        name: &str,
        src: Register,
        ty: SlotType,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        let variable = if let Some(variable) = self.variables.get(name).copied() {
            variable
        } else {
            let variable = Variable {
                register: self.allocate_register(statement.location)?,
                ty: None,
            };
            self.variable_order.push(name.to_string());
            variable
        };
        self.store_variable(name, variable, src, ty, statement)
    }

    fn store_variable(
        &mut self,
        name: &str,
        variable: Variable,
        src: Register,
        ty: SlotType,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        let merged_type = match variable.ty {
            None => ty,
            Some(previous) if previous == ty => previous,
            Some(SlotType::Any) => SlotType::Any,
            Some(_) if ty == SlotType::Any => SlotType::Any,
            Some(previous) => {
                return Err(PythonFrontendError::new(
                    format!("variable `{name}` cannot change type from {previous:?} to {ty:?}"),
                    Some(statement.location),
                ));
            }
        };
        self.code.push(Instruction::Move {
            dst: variable.register,
            src,
        });
        self.variables.insert(
            name.to_string(),
            Variable {
                register: variable.register,
                ty: Some(merged_type),
            },
        );
        Ok(())
    }
}

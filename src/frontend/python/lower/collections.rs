use crate::bytecode::{BooleanOperator, Instruction, Register};
use crate::object::ObjectKind;
use crate::structure_map::SlotType;

use super::Lowerer;
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::HirExpression;

impl Lowerer {
    pub(super) fn lower_boolean(
        &mut self,
        op: BooleanOperator,
        values: &[HirExpression],
        expression: &HirExpression,
    ) -> Result<(Register, SlotType), PythonFrontendError> {
        let Some((first, rest)) = values.split_first() else {
            return Err(PythonFrontendError::new(
                "boolean expression requires at least one operand",
                Some(expression.location),
            ));
        };
        let (first, first_ty) = self.lower_expression(first)?;
        self.expect_type(first_ty, SlotType::Bool, expression, "boolean operand")?;
        let dst = self.allocate_register(expression.location)?;
        self.code.push(Instruction::Move { dst, src: first });
        for value in rest {
            let branch_pc = self.code.len();
            self.code.push(Instruction::Branch {
                cond: dst,
                yes: 0,
                no: 0,
            });
            let rhs_pc = self.code.len();
            let (rhs, rhs_ty) = self.lower_expression(value)?;
            self.expect_type(rhs_ty, SlotType::Bool, value, "boolean operand")?;
            self.code.push(Instruction::Move { dst, src: rhs });
            let exit = self.code.len();
            let Instruction::Branch { yes, no, .. } = &mut self.code[branch_pc] else {
                return Err(PythonFrontendError::new(
                    "internal error while patching boolean branch",
                    Some(expression.location),
                ));
            };
            match op {
                BooleanOperator::And => (*yes, *no) = (rhs_pc, exit),
                BooleanOperator::Or => (*yes, *no) = (exit, rhs_pc),
            }
        }
        Ok((dst, SlotType::Bool))
    }

    pub(super) fn lower_collection(
        &mut self,
        items: &[HirExpression],
        expression: &HirExpression,
        tuple: bool,
    ) -> Result<(Register, SlotType), PythonFrontendError> {
        let items = self.lower_registers(items)?;
        let dst = self.allocate_register(expression.location)?;
        let kind = if tuple {
            ObjectKind::Tuple
        } else {
            ObjectKind::List
        };
        if tuple {
            self.code.push(Instruction::BuildTuple { dst, items });
        } else {
            self.code.push(Instruction::BuildList { dst, items });
        }
        Ok((dst, SlotType::Object(kind)))
    }

    pub(super) fn lower_dict(
        &mut self,
        entries: &[(HirExpression, HirExpression)],
        expression: &HirExpression,
    ) -> Result<(Register, SlotType), PythonFrontendError> {
        let mut lowered = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            lowered.push((
                self.lower_expression(key)?.0,
                self.lower_expression(value)?.0,
            ));
        }
        let dst = self.allocate_register(expression.location)?;
        self.code.push(Instruction::BuildDict {
            dst,
            entries: lowered,
        });
        Ok((dst, SlotType::Object(ObjectKind::Dict)))
    }

    pub(super) fn lower_registers(
        &mut self,
        expressions: &[HirExpression],
    ) -> Result<Vec<Register>, PythonFrontendError> {
        expressions
            .iter()
            .map(|value| self.lower_expression(value).map(|item| item.0))
            .collect()
    }
}

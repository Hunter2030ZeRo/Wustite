use crate::bytecode::{Instruction, Register, UnaryOperator};
use crate::executable::{ConstantId, ExecutableConstant};
use crate::object::ObjectKind;
use crate::structure_map::{SlotType, TypeFact};

use super::Lowerer;
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::{HirExpression, HirExpressionKind};

impl Lowerer {
    pub(super) fn lower_expression(
        &mut self,
        expression: &HirExpression,
    ) -> Result<(Register, SlotType), PythonFrontendError> {
        match &expression.kind {
            HirExpressionKind::SmallInt(value) => {
                let dst = self.allocate_register(expression.location)?;
                self.code
                    .push(Instruction::ConstSmallInt { dst, value: *value });
                Ok((dst, SlotType::SmallInt))
            }
            HirExpressionKind::Float(value) => {
                let dst = self.allocate_register(expression.location)?;
                self.code
                    .push(Instruction::ConstFloat { dst, value: *value });
                Ok((dst, SlotType::Float))
            }
            HirExpressionKind::Bool(value) => {
                let dst = self.allocate_register(expression.location)?;
                self.code
                    .push(Instruction::ConstBool { dst, value: *value });
                Ok((dst, SlotType::Bool))
            }
            HirExpressionKind::None => {
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::ConstNone { dst });
                Ok((dst, SlotType::Any))
            }
            HirExpressionKind::String(value) => self.lower_constant(
                ExecutableConstant::String(value.clone()),
                SlotType::Object(ObjectKind::String),
                expression,
            ),
            HirExpressionKind::BigInt(value) => self.lower_constant(
                ExecutableConstant::BigInt(value.clone()),
                SlotType::Object(ObjectKind::BigInt),
                expression,
            ),
            HirExpressionKind::Function(function) => self.lower_constant(
                ExecutableConstant::Function(function.clone()),
                SlotType::Object(ObjectKind::Function),
                expression,
            ),
            HirExpressionKind::Class(class) => self.lower_constant(
                ExecutableConstant::Class((**class).clone()),
                SlotType::Object(ObjectKind::Class),
                expression,
            ),
            HirExpressionKind::CurrentFunction => {
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::LoadCurrentFunction { dst });
                Ok((dst, SlotType::Object(ObjectKind::Function)))
            }
            HirExpressionKind::Name(name) => self.lower_name(name, expression),
            HirExpressionKind::Unary { op, operand } => {
                let (src, ty) = self.lower_expression(operand)?;
                let result_ty = match op {
                    UnaryOperator::Negate => ty,
                    UnaryOperator::Not => {
                        self.expect_type(ty, SlotType::Bool, operand, "not operand")?;
                        SlotType::Bool
                    }
                };
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::UnaryOp { dst, op: *op, src });
                Ok((dst, result_ty))
            }
            HirExpressionKind::Binary { op, lhs, rhs } => {
                let (lhs_register, lhs_ty) = self.lower_expression(lhs)?;
                let (rhs_register, rhs_ty) = self.lower_expression(rhs)?;
                let result_ty = binary_result_type(*op, lhs_ty, rhs_ty);
                let dst = self.allocate_register(expression.location)?;
                let site = self
                    .map_builder
                    .record_operation(
                        self.code.len(),
                        type_fact(lhs_ty),
                        type_fact(rhs_ty),
                        type_fact(result_ty),
                    )
                    .map_err(|error| PythonFrontendError::new(error, Some(expression.location)))?;
                self.code.push(Instruction::BinaryOp {
                    dst,
                    op: *op,
                    lhs: lhs_register,
                    rhs: rhs_register,
                    site,
                });
                Ok((dst, result_ty))
            }
            HirExpressionKind::Compare { op, lhs, rhs } => {
                let (lhs_register, lhs_ty) = self.lower_expression(lhs)?;
                let (rhs_register, rhs_ty) = self.lower_expression(rhs)?;
                let dst = self.allocate_register(expression.location)?;
                let site = self
                    .map_builder
                    .record_operation(
                        self.code.len(),
                        type_fact(lhs_ty),
                        type_fact(rhs_ty),
                        TypeFact::Proven(SlotType::Bool),
                    )
                    .map_err(|error| PythonFrontendError::new(error, Some(expression.location)))?;
                self.code.push(Instruction::CompareOp {
                    dst,
                    op: *op,
                    lhs: lhs_register,
                    rhs: rhs_register,
                    site,
                });
                Ok((dst, SlotType::Bool))
            }
            HirExpressionKind::Boolean { op, values } => {
                self.lower_boolean(*op, values, expression)
            }
            HirExpressionKind::Tuple(items) => self.lower_collection(items, expression, true),
            HirExpressionKind::List(items) => self.lower_collection(items, expression, false),
            HirExpressionKind::ListComprehension { .. } => {
                self.lower_list_comprehension(expression)
            }
            HirExpressionKind::Dict(entries) => self.lower_dict(entries, expression),
            HirExpressionKind::GetItem { object, key } => {
                let (object, _) = self.lower_expression(object)?;
                let (key, _) = self.lower_expression(key)?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::GetItem { dst, object, key });
                Ok((dst, SlotType::Any))
            }
            HirExpressionKind::GetAttr { object, name } => {
                let (object, _) = self.lower_expression(object)?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::GetAttr {
                    dst,
                    object,
                    name: name.clone(),
                });
                Ok((dst, SlotType::Any))
            }
            HirExpressionKind::GetSlice {
                object,
                start,
                stop,
                step,
            } => {
                let (object, _) = self.lower_expression(object)?;
                let start = self.lower_optional_expression(start.as_deref())?;
                let stop = self.lower_optional_expression(stop.as_deref())?;
                let step = self.lower_optional_expression(step.as_deref())?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::GetSlice {
                    dst,
                    object,
                    start,
                    stop,
                    step,
                });
                Ok((dst, SlotType::Any))
            }
            HirExpressionKind::ListPop { list, index } => {
                let (list, _) = self.lower_expression(list)?;
                let (index, _) = self.lower_expression(index)?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::ListPop { dst, list, index });
                Ok((dst, SlotType::Any))
            }
            HirExpressionKind::Length(object) => {
                let (object, _) = self.lower_expression(object)?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::Length { dst, object });
                Ok((dst, SlotType::SmallInt))
            }
            HirExpressionKind::Call { callable, args } => {
                let (callable, _) = self.lower_expression(callable)?;
                let args = self.lower_registers(args)?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::Call {
                    dst,
                    callable,
                    args,
                });
                Ok((dst, SlotType::Any))
            }
            HirExpressionKind::CallMethod {
                receiver,
                name,
                args,
            } => {
                let (receiver, _) = self.lower_expression(receiver)?;
                let args = self.lower_registers(args)?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::CallMethod {
                    dst,
                    receiver,
                    name: name.clone(),
                    args,
                });
                Ok((dst, SlotType::Any))
            }
        }
    }

    fn lower_constant(
        &mut self,
        constant: ExecutableConstant,
        ty: SlotType,
        expression: &HirExpression,
    ) -> Result<(Register, SlotType), PythonFrontendError> {
        let constant_id = ConstantId(self.constants.len());
        self.constants.push(constant);
        let dst = self.allocate_register(expression.location)?;
        self.code.push(Instruction::LoadConstant {
            dst,
            constant: constant_id,
        });
        Ok((dst, ty))
    }

    fn lower_name(
        &self,
        name: &str,
        expression: &HirExpression,
    ) -> Result<(Register, SlotType), PythonFrontendError> {
        let variable = self.variables.get(name).copied().ok_or_else(|| {
            PythonFrontendError::new(
                format!("name `{name}` is not initialized"),
                Some(expression.location),
            )
        })?;
        let ty = variable.ty.ok_or_else(|| {
            PythonFrontendError::new(
                format!("name `{name}` is used before assignment"),
                Some(expression.location),
            )
        })?;
        Ok((variable.register, ty))
    }

    pub(super) fn lower_optional_expression(
        &mut self,
        expression: Option<&HirExpression>,
    ) -> Result<Option<Register>, PythonFrontendError> {
        expression
            .map(|value| self.lower_expression(value).map(|(register, _)| register))
            .transpose()
    }
}

fn type_fact(ty: SlotType) -> TypeFact {
    if ty == SlotType::Any {
        TypeFact::Unknown
    } else {
        TypeFact::Proven(ty)
    }
}

fn binary_result_type(
    op: crate::bytecode::BinaryOperator,
    lhs: SlotType,
    rhs: SlotType,
) -> SlotType {
    if is_numeric(lhs) && is_numeric(rhs) {
        if op == crate::bytecode::BinaryOperator::Divide
            || op == crate::bytecode::BinaryOperator::Power
            || lhs == SlotType::Float
            || rhs == SlotType::Float
        {
            SlotType::Float
        } else if lhs == SlotType::Object(ObjectKind::BigInt)
            || rhs == SlotType::Object(ObjectKind::BigInt)
        {
            SlotType::Object(ObjectKind::BigInt)
        } else {
            SlotType::SmallInt
        }
    } else if op == crate::bytecode::BinaryOperator::Add && lhs == rhs && is_sequence(lhs) {
        lhs
    } else if op == crate::bytecode::BinaryOperator::Multiply
        && ((is_sequence(lhs) && is_integer(rhs)) || (is_integer(lhs) && is_sequence(rhs)))
    {
        if is_sequence(lhs) { lhs } else { rhs }
    } else {
        SlotType::Any
    }
}

const fn is_numeric(ty: SlotType) -> bool {
    matches!(
        ty,
        SlotType::SmallInt | SlotType::Float | SlotType::Object(ObjectKind::BigInt)
    )
}

const fn is_sequence(ty: SlotType) -> bool {
    matches!(
        ty,
        SlotType::Object(ObjectKind::String | ObjectKind::Tuple | ObjectKind::List)
    )
}

const fn is_integer(ty: SlotType) -> bool {
    matches!(
        ty,
        SlotType::SmallInt | SlotType::Object(ObjectKind::BigInt)
    )
}

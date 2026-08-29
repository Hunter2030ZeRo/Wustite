mod assignment;
mod collections;
mod comprehension;
mod control_flow;
mod expression;
mod loops;
mod mutation;
mod range;
mod range_loop;
mod sequence;

use std::collections::HashMap;

use crate::bytecode::{Function, Instruction, Register};
use crate::executable::{ExecutableConstant, ExecutableFunction, ExecutableParameter};
use crate::object::ObjectKind;
use crate::structure_map::{SlotType, StructureMapBuilder, TypeFact};
use crate::verifier;

use super::error::PythonFrontendError;
use super::hir::{HirExpression, HirFunction, HirStatement, HirStatementKind};

#[derive(Clone, Copy)]
pub(super) struct Variable {
    pub register: Register,
    pub ty: Option<SlotType>,
}

pub(crate) fn lower(function: HirFunction) -> Result<ExecutableFunction, PythonFrontendError> {
    let mut lowerer = Lowerer::default();
    let mut parameters = Vec::with_capacity(function.parameters.len());
    for parameter in function.parameters {
        let register = lowerer.allocate_register(parameter.location)?;
        lowerer
            .map_builder
            .record_parameter(
                register,
                parameters.len(),
                parameter.name.clone(),
                parameter.ty,
            )
            .map_err(|error| PythonFrontendError::new(error, Some(parameter.location)))?;
        lowerer.variables.insert(
            parameter.name.clone(),
            Variable {
                register,
                ty: Some(parameter.ty),
            },
        );
        lowerer.variable_order.push(parameter.name.clone());
        parameters.push(ExecutableParameter {
            name: parameter.name,
            register,
            ty: parameter.ty,
        });
    }
    lowerer.lower_statements(&function.body)?;
    for (index, constant) in lowerer.constants.iter().enumerate() {
        let kind = match constant {
            ExecutableConstant::String(_) => ObjectKind::String,
            ExecutableConstant::BigInt(_) => ObjectKind::BigInt,
            ExecutableConstant::Function(_) => ObjectKind::Function,
            ExecutableConstant::Class(_) => ObjectKind::Class,
        };
        lowerer
            .map_builder
            .record_constant(index, kind)
            .map_err(|error| PythonFrontendError::new(error, None))?;
    }

    let function = Function {
        code: lowerer.code,
        register_count: lowerer.next_register as usize,
    };
    let structure_map = lowerer
        .map_builder
        .finish(&function.code, function.register_count)
        .map_err(|error| PythonFrontendError::new(error, None))?;
    let executable =
        ExecutableFunction::new_with_abi(function, structure_map, parameters, lowerer.constants);
    verifier::verify(&executable).map_err(|error| {
        PythonFrontendError::new(format!("generated invalid WVM: {error}"), None)
    })?;
    Ok(executable)
}

#[derive(Default)]
pub(super) struct Lowerer {
    pub code: Vec<Instruction>,
    pub constants: Vec<ExecutableConstant>,
    pub variables: HashMap<String, Variable>,
    pub variable_order: Vec<String>,
    pub next_register: u32,
    pub map_builder: StructureMapBuilder,
    pub loop_breaks: Vec<Vec<usize>>,
}

impl Lowerer {
    pub(super) fn refresh_live_slot_types(&self, slots: &mut [crate::structure_map::StateSlot]) {
        for slot in slots {
            if let Some(ty) = self
                .variables
                .values()
                .find(|variable| variable.register == slot.register)
                .and_then(|variable| variable.ty)
            {
                slot.ty = ty;
            }
        }
    }

    fn lower_statements(&mut self, statements: &[HirStatement]) -> Result<(), PythonFrontendError> {
        for statement in statements {
            match &statement.kind {
                HirStatementKind::Assign { target, value } => {
                    self.lower_assignment(target, value, statement)?;
                }
                HirStatementKind::SetItem { .. }
                | HirStatementKind::SetAttr { .. }
                | HirStatementKind::SetSlice { .. }
                | HirStatementKind::AugSetItem { .. }
                | HirStatementKind::ListAppend { .. }
                | HirStatementKind::ListInsert { .. }
                | HirStatementKind::Expression(_) => {
                    self.lower_mutation(statement)?;
                }
                HirStatementKind::Break => self.lower_break(statement)?,
                HirStatementKind::While {
                    condition,
                    body,
                    orelse,
                } => {
                    self.lower_while(condition, body, orelse, statement)?;
                }
                HirStatementKind::If { .. } => {
                    self.lower_if(statement)?;
                }
                HirStatementKind::ForRange { .. } => {
                    self.lower_for_range(statement)?;
                }
                HirStatementKind::ForSequence { .. } => {
                    self.lower_for_sequence(statement)?;
                }
                HirStatementKind::Return(value) => {
                    let (src, _) = self.lower_expression(value)?;
                    self.code.push(Instruction::Return { src });
                }
            }
        }
        Ok(())
    }

    pub(super) fn expect_type(
        &self,
        actual: SlotType,
        expected: SlotType,
        expression: &HirExpression,
        context: &str,
    ) -> Result<(), PythonFrontendError> {
        if actual == expected {
            Ok(())
        } else {
            Err(PythonFrontendError::new(
                format!("{context} must be {expected:?}, found {actual:?}"),
                Some(expression.location),
            ))
        }
    }

    pub(super) fn lower_condition(
        &mut self,
        expression: &HirExpression,
        context: &str,
    ) -> Result<Register, PythonFrontendError> {
        let (value, ty) = self.lower_expression(expression)?;
        if ty == SlotType::Bool {
            return Ok(value);
        }
        let zero = self.allocate_register(expression.location)?;
        let zero_type = match ty {
            SlotType::SmallInt
            | SlotType::Any
            | SlotType::Object(crate::object::ObjectKind::BigInt) => {
                self.code.push(Instruction::ConstSmallInt {
                    dst: zero,
                    value: 0,
                });
                SlotType::SmallInt
            }
            SlotType::Float => {
                self.code.push(Instruction::ConstFloat {
                    dst: zero,
                    value: 0.0,
                });
                SlotType::Float
            }
            SlotType::Bool | SlotType::Object(_) => {
                return Err(PythonFrontendError::new(
                    format!("{context} must be Bool or numeric, found {ty:?}"),
                    Some(expression.location),
                ));
            }
        };
        let result = self.allocate_register(expression.location)?;
        let lhs_fact = if ty == SlotType::Any {
            TypeFact::Unknown
        } else {
            TypeFact::Proven(ty)
        };
        let site = self
            .map_builder
            .record_operation(
                self.code.len(),
                lhs_fact,
                TypeFact::Proven(zero_type),
                TypeFact::Proven(SlotType::Bool),
            )
            .map_err(|error| PythonFrontendError::new(error, Some(expression.location)))?;
        self.code.push(Instruction::CompareOp {
            dst: result,
            op: crate::bytecode::CompareOperator::NotEq,
            lhs: value,
            rhs: zero,
            site,
        });
        Ok(result)
    }

    pub(super) fn allocate_register(
        &mut self,
        location: super::SourceLocation,
    ) -> Result<Register, PythonFrontendError> {
        let register = Register::try_from(self.next_register).map_err(|_| {
            PythonFrontendError::new("function requires too many WVM registers", Some(location))
        })?;
        self.next_register += 1;
        Ok(register)
    }
}

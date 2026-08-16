mod collections;
mod expression;

use std::collections::HashMap;

use crate::bytecode::{Function, Instruction, Register};
use crate::executable::{ExecutableConstant, ExecutableFunction, ExecutableParameter};
use crate::structure_map::{RegionExit, RegionKind, SlotType, StateSlot, StructureMapBuilder};
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
    lowerer.lower_statements(&function.body, false)?;

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
}

impl Lowerer {
    fn lower_statements(
        &mut self,
        statements: &[HirStatement],
        in_loop: bool,
    ) -> Result<(), PythonFrontendError> {
        for statement in statements {
            match &statement.kind {
                HirStatementKind::Assign { name, value } => {
                    self.lower_assignment(name, value, statement, in_loop)?;
                }
                HirStatementKind::While { condition, body } => {
                    self.lower_while(condition, body, statement)?;
                }
                HirStatementKind::Return(value) => {
                    let (src, _) = self.lower_expression(value)?;
                    self.code.push(Instruction::Return { src });
                }
            }
        }
        Ok(())
    }

    fn lower_assignment(
        &mut self,
        name: &str,
        value: &HirExpression,
        statement: &HirStatement,
        in_loop: bool,
    ) -> Result<(), PythonFrontendError> {
        if in_loop && !self.variables.contains_key(name) {
            return Err(PythonFrontendError::new(
                format!("variable `{name}` is first introduced inside a while loop"),
                Some(statement.location),
            ));
        }
        let variable = if let Some(variable) = self.variables.get(name).copied() {
            variable
        } else {
            let variable = Variable {
                register: self.allocate_register(statement.location)?,
                ty: None,
            };
            self.variables.insert(name.to_string(), variable);
            self.variable_order.push(name.to_string());
            variable
        };
        let (src, ty) = self.lower_expression(value)?;
        if let Some(previous) = variable.ty
            && previous != SlotType::Any
            && ty != SlotType::Any
            && previous != ty
        {
            return Err(PythonFrontendError::new(
                format!("variable `{name}` cannot change type from {previous:?} to {ty:?}"),
                Some(statement.location),
            ));
        }
        self.code.push(Instruction::Move {
            dst: variable.register,
            src,
        });
        self.variables.insert(
            name.to_string(),
            Variable {
                register: variable.register,
                ty: Some(variable.ty.unwrap_or(ty)),
            },
        );
        Ok(())
    }

    fn lower_while(
        &mut self,
        condition: &HirExpression,
        body: &[HirStatement],
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        let live_slots = self
            .variable_order
            .iter()
            .filter_map(|name| {
                let variable = self.variables.get(name)?;
                Some(StateSlot {
                    register: variable.register,
                    ty: variable.ty?,
                })
            })
            .collect();
        let header = self.code.len();
        let region = self.map_builder.begin_region(header, live_slots);
        let (cond, ty) = self.lower_expression(condition)?;
        self.expect_type(ty, SlotType::Bool, condition, "while condition")?;
        let branch_pc = self.code.len();
        self.code.push(Instruction::Branch {
            cond,
            yes: 0,
            no: 0,
        });
        let body_pc = self.code.len();
        self.lower_statements(body, true)?;
        let backedge = self.code.len();
        self.code.push(Instruction::Jump { target: header });
        let exit = self.code.len();
        let Instruction::Branch { yes, no, .. } = &mut self.code[branch_pc] else {
            return Err(PythonFrontendError::new(
                "internal error while patching while branch",
                Some(statement.location),
            ));
        };
        *yes = body_pc;
        *no = exit;
        self.map_builder
            .finish_region(
                region,
                RegionKind::Loop { backedge },
                vec![RegionExit { target: exit }],
            )
            .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))?;
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

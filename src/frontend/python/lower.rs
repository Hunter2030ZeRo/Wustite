use std::collections::HashMap;

use crate::bytecode::{Function, Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::structure_map::{LiveSlot, LoopRegion, RegionExit, SlotType, StructureMap};
use crate::verifier;

use super::error::PythonFrontendError;
use super::hir::{HirExpression, HirExpressionKind, HirFunction, HirStatement, HirStatementKind};

#[derive(Debug, Clone, Copy)]
struct Variable {
    register: Register,
    ty: Option<SlotType>,
}

pub(crate) fn lower(function: HirFunction) -> Result<ExecutableFunction, PythonFrontendError> {
    let mut lowerer = Lowerer::default();
    lowerer.lower_statements(&function.body, false)?;

    let executable = ExecutableFunction::new(
        Function {
            code: lowerer.code,
            register_count: lowerer.next_register as usize,
        },
        StructureMap {
            loops: lowerer.loops,
        },
    );
    verifier::verify(&executable).map_err(|error| {
        PythonFrontendError::new(format!("generated invalid WVM: {error}"), None)
    })?;
    Ok(executable)
}

#[derive(Default)]
struct Lowerer {
    code: Vec<Instruction>,
    loops: Vec<LoopRegion>,
    variables: HashMap<String, Variable>,
    variable_order: Vec<String>,
    next_register: u32,
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
                ty: Some(ty),
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
                Some(LiveSlot {
                    register: variable.register,
                    ty: variable.ty?,
                })
            })
            .collect();

        let header = self.code.len();
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

        self.loops.push(LoopRegion {
            header,
            backedge,
            exits: vec![RegionExit { target: exit }],
            live_slots,
        });
        Ok(())
    }

    fn lower_expression(
        &mut self,
        expression: &HirExpression,
    ) -> Result<(Register, SlotType), PythonFrontendError> {
        match &expression.kind {
            HirExpressionKind::I64(value) => {
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::ConstI64 { dst, value: *value });
                Ok((dst, SlotType::I64))
            }
            HirExpressionKind::Name(name) => {
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
            HirExpressionKind::Add(lhs, rhs) => {
                let (lhs, lhs_ty) = self.lower_expression(lhs)?;
                let (rhs, rhs_ty) = self.lower_expression(rhs)?;
                self.expect_type(lhs_ty, SlotType::I64, expression, "addition operand")?;
                self.expect_type(rhs_ty, SlotType::I64, expression, "addition operand")?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::AddI64 { dst, lhs, rhs });
                Ok((dst, SlotType::I64))
            }
            HirExpressionKind::SignedLt(lhs, rhs) => {
                let (lhs, lhs_ty) = self.lower_expression(lhs)?;
                let (rhs, rhs_ty) = self.lower_expression(rhs)?;
                self.expect_type(lhs_ty, SlotType::I64, expression, "comparison operand")?;
                self.expect_type(rhs_ty, SlotType::I64, expression, "comparison operand")?;
                let dst = self.allocate_register(expression.location)?;
                self.code.push(Instruction::LtI64 { dst, lhs, rhs });
                Ok((dst, SlotType::Bool))
            }
        }
    }

    fn expect_type(
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

    fn allocate_register(
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

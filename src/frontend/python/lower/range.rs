use crate::bytecode::{BinaryOperator, CompareOperator, Instruction};
use crate::object::ObjectKind;
use crate::structure_map::{RegionExit, RegionKind, SlotType, StateSlot, TypeFact};

use super::{Lowerer, Variable};
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::{HirExpression, HirStatement, HirStatementKind};

impl Lowerer {
    pub(super) fn lower_for_range(
        &mut self,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        let HirStatementKind::ForRange {
            target,
            start,
            stop,
            step,
            guaranteed_non_empty,
            body,
            orelse,
        } = &statement.kind
        else {
            return Err(PythonFrontendError::new(
                "internal error lowering non-range statement as range",
                Some(statement.location),
            ));
        };
        let (start_value, cursor_type) = self.lower_expression(start)?;
        self.expect_integer(cursor_type, start, "range start")?;
        let cursor = self.allocate_register(statement.location)?;
        self.code.push(Instruction::Move {
            dst: cursor,
            src: start_value,
        });
        let (stop_value, stop_type) = self.lower_expression(stop)?;
        self.expect_integer(stop_type, stop, "range stop")?;
        let stop_register = self.allocate_register(statement.location)?;
        self.code.push(Instruction::Move {
            dst: stop_register,
            src: stop_value,
        });
        let step_register = self.allocate_register(statement.location)?;
        self.code.push(Instruction::ConstSmallInt {
            dst: step_register,
            value: *step,
        });

        let mut live_slots = self.live_slots();
        for slot in [
            StateSlot {
                register: cursor,
                ty: cursor_type,
            },
            StateSlot {
                register: stop_register,
                ty: stop_type,
            },
            StateSlot {
                register: step_register,
                ty: SlotType::SmallInt,
            },
        ] {
            if !live_slots.iter().any(|live| live.register == slot.register) {
                live_slots.push(slot);
            }
        }
        let target_variable = if let Some(variable) = self.variables.get(target).copied() {
            if let Some(previous) = variable.ty
                && previous != SlotType::Any
                && previous != cursor_type
            {
                return Err(PythonFrontendError::new(
                    format!(
                        "variable `{target}` cannot change type from {previous:?} to {cursor_type:?}"
                    ),
                    Some(statement.location),
                ));
            }
            Variable {
                register: variable.register,
                ty: Some(variable.ty.unwrap_or(cursor_type)),
            }
        } else {
            let variable = Variable {
                register: self.allocate_register(statement.location)?,
                ty: Some(cursor_type),
            };
            self.variable_order.push(target.clone());
            variable
        };
        self.variables.insert(target.clone(), target_variable);
        let first_iteration_jump = if *guaranteed_non_empty {
            let pc = self.code.len();
            self.code.push(Instruction::Jump { target: 0 });
            Some(pc)
        } else {
            None
        };
        let header = self.code.len();
        let region = self.map_builder.begin_region(header, live_slots.clone());
        let condition = self.allocate_register(statement.location)?;
        let comparison = if *step > 0 {
            CompareOperator::Lt
        } else {
            CompareOperator::Gt
        };
        let comparison_site = self
            .map_builder
            .record_operation(
                self.code.len(),
                type_fact(cursor_type),
                type_fact(stop_type),
                TypeFact::Proven(SlotType::Bool),
            )
            .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))?;
        self.code.push(Instruction::CompareOp {
            dst: condition,
            op: comparison,
            lhs: cursor,
            rhs: stop_register,
            site: comparison_site,
        });
        let branch_pc = self.code.len();
        self.code.push(Instruction::Branch {
            cond: condition,
            yes: 0,
            no: 0,
        });
        let body_pc = self.code.len();
        if let Some(pc) = first_iteration_jump {
            let Instruction::Jump { target } = &mut self.code[pc] else {
                return Err(PythonFrontendError::new(
                    "internal error while patching guaranteed range entry",
                    Some(statement.location),
                ));
            };
            *target = body_pc;
        }
        self.code.push(Instruction::Move {
            dst: target_variable.register,
            src: cursor,
        });
        let breaks = self.lower_loop_body(body)?;
        let next_value = self.allocate_register(statement.location)?;
        let increment_site = self
            .map_builder
            .record_operation(
                self.code.len(),
                type_fact(cursor_type),
                TypeFact::Proven(SlotType::SmallInt),
                type_fact(cursor_type),
            )
            .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))?;
        self.code.push(Instruction::BinaryOp {
            dst: next_value,
            op: BinaryOperator::Add,
            lhs: cursor,
            rhs: step_register,
            site: increment_site,
        });
        self.code.push(Instruction::Move {
            dst: cursor,
            src: next_value,
        });
        let backedge = self.code.len();
        self.code.push(Instruction::Jump { target: header });
        self.refresh_live_slot_types(&mut live_slots);
        self.map_builder
            .update_region_entry_summary(region, live_slots)
            .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))?;
        let orelse_pc = self.code.len();
        self.lower_statements(orelse)?;
        let exit = self.code.len();
        let Instruction::Branch { yes, no, .. } = &mut self.code[branch_pc] else {
            return Err(PythonFrontendError::new(
                "internal error while patching for range branch",
                Some(statement.location),
            ));
        };
        (*yes, *no) = (body_pc, orelse_pc);
        self.patch_breaks(&breaks, exit, statement)?;
        self.map_builder
            .finish_region(
                region,
                RegionKind::Loop { backedge },
                vec![RegionExit { target: exit }],
            )
            .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))
    }

    fn live_slots(&self) -> Vec<StateSlot> {
        self.variable_order
            .iter()
            .filter_map(|name| {
                let variable = self.variables.get(name)?;
                Some(StateSlot {
                    register: variable.register,
                    ty: variable.ty?,
                })
            })
            .collect()
    }

    fn expect_integer(
        &self,
        actual: SlotType,
        expression: &HirExpression,
        context: &str,
    ) -> Result<(), PythonFrontendError> {
        if matches!(
            actual,
            SlotType::SmallInt | SlotType::Object(ObjectKind::BigInt)
        ) {
            Ok(())
        } else {
            Err(PythonFrontendError::new(
                format!("{context} must be an integer, found {actual:?}"),
                Some(expression.location),
            ))
        }
    }
}

fn type_fact(ty: SlotType) -> TypeFact {
    if ty == SlotType::Any {
        TypeFact::Unknown
    } else {
        TypeFact::Proven(ty)
    }
}

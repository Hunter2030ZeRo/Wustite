use crate::bytecode::{BinaryOperator, CompareOperator, Instruction};
use crate::frontend::python::PythonFrontendError;
use crate::object::ObjectKind;
use crate::structure_map::{RegionExit, RegionKind, SlotType, StateSlot, TypeFact};

use super::comprehension::ComprehensionLoop;
use super::{Lowerer, Variable};

mod state;

impl Lowerer {
    pub(super) fn lower_comprehension_loop(
        &mut self,
        loop_spec: ComprehensionLoop<'_>,
    ) -> Result<(), PythonFrontendError> {
        let state = self.initialize_comprehension_range(&loop_spec)?;
        let mut live_slots = self.comprehension_live_slots();
        for slot in [
            StateSlot {
                register: state.cursor,
                ty: state.cursor_type,
            },
            StateSlot {
                register: state.stop,
                ty: state.stop_type,
            },
            StateSlot {
                register: state.step,
                ty: SlotType::SmallInt,
            },
            StateSlot {
                register: loop_spec.result,
                ty: SlotType::Object(ObjectKind::List),
            },
        ] {
            push_live_slot(&mut live_slots, slot);
        }
        if let Some(sequence) = state.sequence {
            push_live_slot(
                &mut live_slots,
                StateSlot {
                    register: sequence,
                    ty: SlotType::Any,
                },
            );
        }

        let previous_target = self.variables.remove(loop_spec.target);
        let target = Variable {
            register: self.allocate_register(loop_spec.location)?,
            ty: Some(if state.sequence.is_some() {
                SlotType::Any
            } else {
                state.cursor_type
            }),
        };
        self.variables.insert(loop_spec.target.to_string(), target);

        let header = self.code.len();
        let region = self.map_builder.begin_region(header, live_slots.clone());
        let condition = self.allocate_register(loop_spec.location)?;
        let comparison = if state.step_value > 0 {
            CompareOperator::Lt
        } else {
            CompareOperator::Gt
        };
        let comparison_site = self
            .map_builder
            .record_operation(
                self.code.len(),
                type_fact(state.cursor_type),
                type_fact(state.stop_type),
                TypeFact::Proven(SlotType::Bool),
            )
            .map_err(|error| PythonFrontendError::new(error, Some(loop_spec.location)))?;
        self.code.push(Instruction::CompareOp {
            dst: condition,
            op: comparison,
            lhs: state.cursor,
            rhs: state.stop,
            site: comparison_site,
        });
        let branch_pc = self.code.len();
        self.code.push(Instruction::Branch {
            cond: condition,
            yes: 0,
            no: 0,
        });
        let body_pc = self.code.len();
        if let Some(sequence) = state.sequence {
            self.code.push(Instruction::GetItem {
                dst: target.register,
                object: sequence,
                key: state.cursor,
            });
        } else {
            self.code.push(Instruction::Move {
                dst: target.register,
                src: state.cursor,
            });
        }
        self.accumulate_comprehension_value(loop_spec.result, loop_spec.element)?;
        let next_value = self.allocate_register(loop_spec.location)?;
        let increment_site = self
            .map_builder
            .record_operation(
                self.code.len(),
                type_fact(state.cursor_type),
                TypeFact::Proven(SlotType::SmallInt),
                type_fact(state.cursor_type),
            )
            .map_err(|error| PythonFrontendError::new(error, Some(loop_spec.location)))?;
        self.code.push(Instruction::BinaryOp {
            dst: next_value,
            op: BinaryOperator::Add,
            lhs: state.cursor,
            rhs: state.step,
            site: increment_site,
        });
        self.code.push(Instruction::Move {
            dst: state.cursor,
            src: next_value,
        });
        let backedge = self.code.len();
        self.code.push(Instruction::Jump { target: header });
        self.refresh_live_slot_types(&mut live_slots);
        self.map_builder
            .update_region_entry_summary(region, live_slots)
            .map_err(|error| PythonFrontendError::new(error, Some(loop_spec.location)))?;
        let exit = self.code.len();
        let Instruction::Branch { yes, no, .. } = &mut self.code[branch_pc] else {
            return Err(PythonFrontendError::new(
                "internal error while patching comprehension branch",
                Some(loop_spec.location),
            ));
        };
        (*yes, *no) = (body_pc, exit);
        let result = self
            .map_builder
            .finish_region(
                region,
                RegionKind::Loop { backedge },
                vec![RegionExit { target: exit }],
            )
            .map_err(|error| PythonFrontendError::new(error, Some(loop_spec.location)));
        if let Some(previous_target) = previous_target {
            self.variables
                .insert(loop_spec.target.to_string(), previous_target);
        } else {
            self.variables.remove(loop_spec.target);
        }
        result
    }

    fn comprehension_live_slots(&self) -> Vec<StateSlot> {
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
}

fn push_live_slot(live_slots: &mut Vec<StateSlot>, slot: StateSlot) {
    if !live_slots.iter().any(|live| live.register == slot.register) {
        live_slots.push(slot);
    }
}

fn type_fact(ty: SlotType) -> TypeFact {
    if ty == SlotType::Any {
        TypeFact::Unknown
    } else {
        TypeFact::Proven(ty)
    }
}

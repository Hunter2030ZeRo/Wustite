use crate::bytecode::{BinaryOperator, BooleanOperator, CompareOperator, Instruction, Register};
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::{HirStatement, HirStatementKind};
use crate::structure_map::{RegionExit, RegionKind, SlotType, StateSlot, TypeFact};

use super::Lowerer;

impl Lowerer {
    pub(super) fn lower_for_sequence(
        &mut self,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        let HirStatementKind::ForSequence {
            targets,
            iterables,
            include_index,
            body,
            orelse,
        } = &statement.kind
        else {
            return Err(PythonFrontendError::new(
                "internal error lowering non-sequence for statement",
                Some(statement.location),
            ));
        };

        let mut sequences = Vec::with_capacity(iterables.len());
        let mut stops = Vec::with_capacity(iterables.len());
        for iterable in iterables {
            let (sequence, _) = self.lower_expression(iterable)?;
            let stop = self.allocate_register(statement.location)?;
            self.code.push(Instruction::Length {
                dst: stop,
                object: sequence,
            });
            sequences.push(sequence);
            stops.push(stop);
        }
        let cursor = self.allocate_register(statement.location)?;
        self.code.push(Instruction::ConstSmallInt {
            dst: cursor,
            value: 0,
        });
        let step = self.allocate_register(statement.location)?;
        self.code.push(Instruction::ConstSmallInt {
            dst: step,
            value: 1,
        });

        let mut live_slots = self.sequence_live_slots();
        push_live_slot(
            &mut live_slots,
            StateSlot {
                register: cursor,
                ty: SlotType::SmallInt,
            },
        );
        push_live_slot(
            &mut live_slots,
            StateSlot {
                register: step,
                ty: SlotType::SmallInt,
            },
        );
        for register in sequences.iter().copied() {
            push_live_slot(
                &mut live_slots,
                StateSlot {
                    register,
                    ty: SlotType::Any,
                },
            );
        }
        for register in stops.iter().copied() {
            push_live_slot(
                &mut live_slots,
                StateSlot {
                    register,
                    ty: SlotType::SmallInt,
                },
            );
        }

        let header = self.code.len();
        let region = self.map_builder.begin_region(header, live_slots.clone());
        let condition = self.sequence_condition(cursor, &stops, statement)?;
        let branch_pc = self.code.len();
        self.code.push(Instruction::Branch {
            cond: condition,
            yes: 0,
            no: 0,
        });
        let body_pc = self.code.len();
        if *include_index {
            self.bind_target(&targets[0], cursor, SlotType::SmallInt, statement)?;
            let value = self.allocate_register(statement.location)?;
            self.code.push(Instruction::GetItem {
                dst: value,
                object: sequences[0],
                key: cursor,
            });
            self.bind_target(&targets[1], value, SlotType::Any, statement)?;
        } else {
            for (target, sequence) in targets.iter().zip(&sequences) {
                let value = self.allocate_register(statement.location)?;
                self.code.push(Instruction::GetItem {
                    dst: value,
                    object: *sequence,
                    key: cursor,
                });
                self.bind_target(target, value, SlotType::Any, statement)?;
            }
        }
        let breaks = self.lower_loop_body(body)?;
        let next = self.allocate_register(statement.location)?;
        let increment_site = self
            .map_builder
            .record_operation(
                self.code.len(),
                TypeFact::Proven(SlotType::SmallInt),
                TypeFact::Proven(SlotType::SmallInt),
                TypeFact::Proven(SlotType::SmallInt),
            )
            .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))?;
        self.code.push(Instruction::BinaryOp {
            dst: next,
            op: BinaryOperator::Add,
            lhs: cursor,
            rhs: step,
            site: increment_site,
        });
        self.code.push(Instruction::Move {
            dst: cursor,
            src: next,
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
                "internal error while patching sequence for branch",
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

    fn sequence_condition(
        &mut self,
        cursor: Register,
        stops: &[Register],
        statement: &HirStatement,
    ) -> Result<Register, PythonFrontendError> {
        let mut conditions = Vec::with_capacity(stops.len());
        for stop in stops {
            let condition = self.allocate_register(statement.location)?;
            let site = self
                .map_builder
                .record_operation(
                    self.code.len(),
                    TypeFact::Proven(SlotType::SmallInt),
                    TypeFact::Proven(SlotType::SmallInt),
                    TypeFact::Proven(SlotType::Bool),
                )
                .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))?;
            self.code.push(Instruction::CompareOp {
                dst: condition,
                op: CompareOperator::Lt,
                lhs: cursor,
                rhs: *stop,
                site,
            });
            conditions.push(condition);
        }
        let mut result = conditions[0];
        for condition in &conditions[1..] {
            let combined = self.allocate_register(statement.location)?;
            self.code.push(Instruction::BooleanOp {
                dst: combined,
                op: BooleanOperator::And,
                lhs: result,
                rhs: *condition,
            });
            result = combined;
        }
        Ok(result)
    }

    fn sequence_live_slots(&self) -> Vec<StateSlot> {
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

use crate::bytecode::Instruction;
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::{HirExpression, HirStatement};
use crate::structure_map::{RegionExit, RegionKind, StateSlot};

use super::Lowerer;

impl Lowerer {
    pub(super) fn lower_while(
        &mut self,
        condition: &HirExpression,
        body: &[HirStatement],
        orelse: &[HirStatement],
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        let mut live_slots: Vec<StateSlot> = self
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
        let region = self.map_builder.begin_region(header, live_slots.clone());
        let cond = self.lower_condition(condition, "while condition")?;
        let branch_pc = self.code.len();
        self.code.push(Instruction::Branch {
            cond,
            yes: 0,
            no: 0,
        });
        let body_pc = self.code.len();
        let breaks = self.lower_loop_body(body)?;
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
                "internal error while patching while branch",
                Some(statement.location),
            ));
        };
        *yes = body_pc;
        *no = orelse_pc;
        self.patch_breaks(&breaks, exit, statement)?;
        let mut exits = vec![RegionExit { target: orelse_pc }];
        if !breaks.is_empty() && exit != orelse_pc {
            exits.push(RegionExit { target: exit });
        }
        self.map_builder
            .finish_region(region, RegionKind::Loop { backedge }, exits)
            .map_err(|error| PythonFrontendError::new(error, Some(statement.location)))?;
        Ok(())
    }

    pub(super) fn lower_loop_body(
        &mut self,
        body: &[HirStatement],
    ) -> Result<Vec<usize>, PythonFrontendError> {
        self.loop_breaks.push(Vec::new());
        let result = self.lower_statements(body);
        let breaks = self
            .loop_breaks
            .pop()
            .ok_or_else(|| PythonFrontendError::new("internal loop context underflow", None))?;
        result?;
        Ok(breaks)
    }

    pub(super) fn lower_break(
        &mut self,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        let breaks = self.loop_breaks.last_mut().ok_or_else(|| {
            PythonFrontendError::new("break outside loop", Some(statement.location))
        })?;
        breaks.push(self.code.len());
        self.code.push(Instruction::Jump { target: 0 });
        Ok(())
    }

    pub(super) fn patch_breaks(
        &mut self,
        breaks: &[usize],
        target_pc: usize,
        statement: &HirStatement,
    ) -> Result<(), PythonFrontendError> {
        for break_pc in breaks {
            let Some(Instruction::Jump { target }) = self.code.get_mut(*break_pc) else {
                return Err(PythonFrontendError::new(
                    "internal error while patching break",
                    Some(statement.location),
                ));
            };
            *target = target_pc;
        }
        Ok(())
    }
}

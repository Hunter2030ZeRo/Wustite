use crate::bytecode::Instruction;

use super::Lowerer;
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::HirStatement;

impl Lowerer {
    pub(super) fn lower_if(&mut self, statement: &HirStatement) -> Result<(), PythonFrontendError> {
        let crate::frontend::python::hir::HirStatementKind::If {
            condition,
            body,
            orelse,
        } = &statement.kind
        else {
            return Err(PythonFrontendError::new(
                "internal error lowering non-if statement as if",
                Some(statement.location),
            ));
        };
        let condition_register = self.lower_condition(condition, "if condition")?;
        let branch_pc = self.code.len();
        self.code.push(Instruction::Branch {
            cond: condition_register,
            yes: 0,
            no: 0,
        });
        let body_pc = self.code.len();
        self.lower_statements(body)?;
        let jump_pc = self.code.len();
        self.code.push(Instruction::Jump { target: 0 });
        let orelse_pc = self.code.len();
        self.lower_statements(orelse)?;
        let exit = self.code.len();
        let Instruction::Branch { yes, no, .. } = &mut self.code[branch_pc] else {
            return Err(PythonFrontendError::new(
                "internal error while patching if branch",
                Some(statement.location),
            ));
        };
        (*yes, *no) = (body_pc, orelse_pc);
        let Instruction::Jump { target } = &mut self.code[jump_pc] else {
            return Err(PythonFrontendError::new(
                "internal error while patching if exit",
                Some(statement.location),
            ));
        };
        *target = exit;
        Ok(())
    }
}

use crate::bytecode::{BinaryOperator, CompareOperator, Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::object::ObjectHeap;
use crate::structure_map::{SlotType, TypeFact};
use crate::value::Value;

use super::arithmetic::ValueOps;
use super::{Frame, Vm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuickInstruction {
    Add {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
    Lt {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
}

pub(super) struct QuickCode(Box<[Option<QuickInstruction>]>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuickOutcome {
    Handled,
    GuardMiss,
}

pub(super) fn execute_quick(
    instruction: QuickInstruction,
    frame: &mut Frame,
    heap: &mut ObjectHeap,
) -> Result<QuickOutcome, String> {
    let (dst, lhs, rhs) = match instruction {
        QuickInstruction::Add { dst, lhs, rhs } | QuickInstruction::Lt { dst, lhs, rhs } => {
            (dst, lhs, rhs)
        }
    };
    Vm::read_register(frame, dst)?;
    let (Value::SmallInt(lhs), Value::SmallInt(rhs)) = (
        Vm::read_register(frame, lhs)?,
        Vm::read_register(frame, rhs)?,
    ) else {
        return Ok(QuickOutcome::GuardMiss);
    };
    let value = match instruction {
        QuickInstruction::Add { .. } => ValueOps::new(heap).smallint_add(lhs, rhs)?,
        QuickInstruction::Lt { .. } => Value::Bool(lhs < rhs),
    };
    Vm::write_register(frame, dst, value)?;
    frame.pc += 1;
    Ok(QuickOutcome::Handled)
}

impl QuickCode {
    pub(super) fn new(executable: &ExecutableFunction) -> Self {
        let small = TypeFact::Exact(SlotType::SmallInt);
        let boolean = TypeFact::Exact(SlotType::Bool);
        let slots = executable
            .bytecode()
            .code
            .iter()
            .enumerate()
            .map(|(pc, instruction)| match instruction {
                Instruction::BinaryOp {
                    dst,
                    op: BinaryOperator::Add,
                    lhs,
                    rhs,
                    site,
                } => executable
                    .structure_map()
                    .operation_site(*site)
                    .filter(|facts| {
                        facts.pc == pc
                            && facts.lhs == small
                            && facts.rhs == small
                            && facts.result == small
                    })
                    .map(|_| QuickInstruction::Add {
                        dst: *dst,
                        lhs: *lhs,
                        rhs: *rhs,
                    }),
                Instruction::CompareOp {
                    dst,
                    op: CompareOperator::Lt,
                    lhs,
                    rhs,
                    site,
                } => executable
                    .structure_map()
                    .operation_site(*site)
                    .filter(|facts| {
                        facts.pc == pc
                            && facts.lhs == small
                            && facts.rhs == small
                            && facts.result == boolean
                    })
                    .map(|_| QuickInstruction::Lt {
                        dst: *dst,
                        lhs: *lhs,
                        rhs: *rhs,
                    }),
                Instruction::BinaryOp {
                    op: BinaryOperator::Subtract,
                    ..
                }
                | Instruction::BinaryOp {
                    op: BinaryOperator::Multiply,
                    ..
                }
                | Instruction::BinaryOp {
                    op: BinaryOperator::Divide,
                    ..
                }
                | Instruction::CompareOp {
                    op: CompareOperator::Eq,
                    ..
                }
                | Instruction::CompareOp {
                    op: CompareOperator::NotEq,
                    ..
                }
                | Instruction::CompareOp {
                    op: CompareOperator::Le,
                    ..
                }
                | Instruction::CompareOp {
                    op: CompareOperator::Gt,
                    ..
                }
                | Instruction::CompareOp {
                    op: CompareOperator::Ge,
                    ..
                }
                | Instruction::ConstSmallInt { .. }
                | Instruction::ConstFloat { .. }
                | Instruction::ConstBool { .. }
                | Instruction::LoadConstant { .. }
                | Instruction::ConstI64 { .. }
                | Instruction::UnaryOp { .. }
                | Instruction::BooleanOp { .. }
                | Instruction::BuildTuple { .. }
                | Instruction::BuildList { .. }
                | Instruction::BuildDict { .. }
                | Instruction::GetItem { .. }
                | Instruction::SetItem { .. }
                | Instruction::Length { .. }
                | Instruction::LoadCurrentFunction { .. }
                | Instruction::Call { .. }
                | Instruction::AddI64 { .. }
                | Instruction::LtI64 { .. }
                | Instruction::Jump { .. }
                | Instruction::Branch { .. }
                | Instruction::Return { .. }
                | Instruction::Move { .. } => None,
            })
            .collect();
        Self(slots)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn get(&self, pc: usize) -> Option<QuickInstruction> {
        self.0.get(pc).copied().flatten()
    }
}

#[cfg(test)]
#[path = "quickening/tests/mod.rs"]
mod tests;

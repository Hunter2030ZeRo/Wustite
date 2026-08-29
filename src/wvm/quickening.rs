use crate::bytecode::{BinaryOperator, CompareOperator, Function, Instruction, Register};
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
    Subtract {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
    Multiply {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
    Divide {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
    FloorDivide {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
    Power {
        dst: Register,
        lhs: Register,
        rhs: Register,
    },
    Compare {
        dst: Register,
        lhs: Register,
        rhs: Register,
        op: CompareOperator,
    },
}

pub(super) struct QuickCode(Box<[Option<QuickInstruction>]>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuickOutcome {
    Handled,
    GuardMiss,
}

#[inline(always)]
pub(super) fn execute_quick(
    instruction: QuickInstruction,
    frame: &mut Frame,
    heap: &mut ObjectHeap,
) -> Result<QuickOutcome, String> {
    let (dst, lhs, rhs) = match instruction {
        QuickInstruction::Add { dst, lhs, rhs }
        | QuickInstruction::Subtract { dst, lhs, rhs }
        | QuickInstruction::Multiply { dst, lhs, rhs }
        | QuickInstruction::Divide { dst, lhs, rhs }
        | QuickInstruction::FloorDivide { dst, lhs, rhs }
        | QuickInstruction::Power { dst, lhs, rhs }
        | QuickInstruction::Compare { dst, lhs, rhs, .. } => (dst, lhs, rhs),
    };
    Vm::read_register(frame, dst)?;
    let lhs = Vm::read_register(frame, lhs)?;
    let rhs = Vm::read_register(frame, rhs)?;
    let value = match instruction {
        QuickInstruction::Add { .. }
        | QuickInstruction::Subtract { .. }
        | QuickInstruction::Multiply { .. }
        | QuickInstruction::Divide { .. }
        | QuickInstruction::FloorDivide { .. }
        | QuickInstruction::Power { .. } => {
            let op = match instruction {
                QuickInstruction::Add { .. } => BinaryOperator::Add,
                QuickInstruction::Subtract { .. } => BinaryOperator::Subtract,
                QuickInstruction::Multiply { .. } => BinaryOperator::Multiply,
                QuickInstruction::Divide { .. } => BinaryOperator::Divide,
                QuickInstruction::FloorDivide { .. } => BinaryOperator::FloorDivide,
                QuickInstruction::Power { .. } => BinaryOperator::Power,
                QuickInstruction::Compare { .. } => unreachable!(),
            };
            let Some(value) = ValueOps::new(heap).immediate_binary(op, lhs, rhs)? else {
                return Ok(QuickOutcome::GuardMiss);
            };
            value
        }
        QuickInstruction::Compare { op, .. } => {
            let (Value::SmallInt(lhs), Value::SmallInt(rhs)) = (lhs, rhs) else {
                return Ok(QuickOutcome::GuardMiss);
            };
            Value::Bool(match op {
                CompareOperator::Eq => lhs == rhs,
                CompareOperator::NotEq => lhs != rhs,
                CompareOperator::Lt => lhs < rhs,
                CompareOperator::Le => lhs <= rhs,
                CompareOperator::Gt => lhs > rhs,
                CompareOperator::Ge => lhs >= rhs,
            })
        }
    };
    Vm::write_register(frame, dst, value)?;
    frame.pc += 1;
    Ok(QuickOutcome::Handled)
}

impl QuickCode {
    pub(super) fn new(executable: &ExecutableFunction) -> Self {
        let small = TypeFact::Proven(SlotType::SmallInt);
        let boolean = TypeFact::Proven(SlotType::Bool);
        let slots = executable
            .bytecode()
            .code
            .iter()
            .enumerate()
            .map(|(pc, instruction)| match instruction {
                Instruction::BinaryOp {
                    dst,
                    op,
                    lhs,
                    rhs,
                    site,
                } if matches!(
                    op,
                    BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply
                ) =>
                {
                    executable
                        .structure_map()
                        .operation_site(*site)
                        .filter(|facts| {
                            facts.pc == pc
                                && facts.lhs == small
                                && facts.rhs == small
                                && facts.result == small
                        })
                        .map(|_| match op {
                            BinaryOperator::Add => QuickInstruction::Add {
                                dst: *dst,
                                lhs: *lhs,
                                rhs: *rhs,
                            },
                            BinaryOperator::Subtract => QuickInstruction::Subtract {
                                dst: *dst,
                                lhs: *lhs,
                                rhs: *rhs,
                            },
                            BinaryOperator::Multiply => QuickInstruction::Multiply {
                                dst: *dst,
                                lhs: *lhs,
                                rhs: *rhs,
                            },
                            BinaryOperator::Divide
                            | BinaryOperator::FloorDivide
                            | BinaryOperator::Power => unreachable!(),
                        })
                }
                Instruction::CompareOp {
                    dst,
                    op,
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
                    .map(|_| QuickInstruction::Compare {
                        dst: *dst,
                        lhs: *lhs,
                        rhs: *rhs,
                        op: *op,
                    }),
                Instruction::BinaryOp { .. }
                | Instruction::ConstSmallInt { .. }
                | Instruction::ConstFloat { .. }
                | Instruction::ConstBool { .. }
                | Instruction::ConstNone { .. }
                | Instruction::LoadConstant { .. }
                | Instruction::ConstI64 { .. }
                | Instruction::UnaryOp { .. }
                | Instruction::BooleanOp { .. }
                | Instruction::BuildTuple { .. }
                | Instruction::BuildList { .. }
                | Instruction::BuildDict { .. }
                | Instruction::GetItem { .. }
                | Instruction::GetAttr { .. }
                | Instruction::GetSlice { .. }
                | Instruction::SetItem { .. }
                | Instruction::SetAttr { .. }
                | Instruction::SetSlice { .. }
                | Instruction::ListAppend { .. }
                | Instruction::ListInsert { .. }
                | Instruction::ListPop { .. }
                | Instruction::Length { .. }
                | Instruction::LoadCurrentFunction { .. }
                | Instruction::Call { .. }
                | Instruction::CallMethod { .. }
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

    pub(super) fn new_interpreter(function: &Function) -> Self {
        let slots = function
            .code
            .iter()
            .map(|instruction| match instruction {
                Instruction::BinaryOp {
                    dst,
                    op: BinaryOperator::Add,
                    lhs,
                    rhs,
                    ..
                } => Some(QuickInstruction::Add {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                }),
                Instruction::BinaryOp {
                    dst,
                    op: BinaryOperator::Subtract,
                    lhs,
                    rhs,
                    ..
                } => Some(QuickInstruction::Subtract {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                }),
                Instruction::BinaryOp {
                    dst,
                    op: BinaryOperator::Multiply,
                    lhs,
                    rhs,
                    ..
                } => Some(QuickInstruction::Multiply {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                }),
                Instruction::BinaryOp {
                    dst,
                    op: BinaryOperator::Divide,
                    lhs,
                    rhs,
                    ..
                } => Some(QuickInstruction::Divide {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                }),
                Instruction::BinaryOp {
                    dst,
                    op: BinaryOperator::FloorDivide,
                    lhs,
                    rhs,
                    ..
                } => Some(QuickInstruction::FloorDivide {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                }),
                Instruction::BinaryOp {
                    dst,
                    op: BinaryOperator::Power,
                    lhs,
                    rhs,
                    ..
                } => Some(QuickInstruction::Power {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                }),
                Instruction::CompareOp {
                    dst, op, lhs, rhs, ..
                } => Some(QuickInstruction::Compare {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                    op: *op,
                }),
                _ => None,
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

use crate::bytecode::{BinaryOperator, Instruction};
use crate::executable::ExecutableFunction;

#[derive(Clone)]
pub(super) struct NumericMethod {
    register_count: usize,
    steps: Vec<NumericStep>,
    result: u16,
}

#[derive(Clone)]
enum NumericStep {
    Constant {
        dst: u16,
        value: i64,
    },
    Move {
        dst: u16,
        src: u16,
    },
    Binary {
        dst: u16,
        op: BinaryOperator,
        lhs: u16,
        rhs: u16,
    },
}

impl NumericMethod {
    pub(super) fn analyze(function: &ExecutableFunction) -> Option<Self> {
        if function.parameters().len() != 2 {
            return None;
        }
        let mut steps = Vec::new();
        let mut result = None;
        for instruction in &function.bytecode().code {
            match instruction {
                Instruction::ConstSmallInt { dst, value }
                | Instruction::ConstI64 { dst, value } => {
                    steps.push(NumericStep::Constant {
                        dst: *dst,
                        value: *value,
                    });
                }
                Instruction::Move { dst, src } => steps.push(NumericStep::Move {
                    dst: *dst,
                    src: *src,
                }),
                Instruction::BinaryOp {
                    dst,
                    op:
                        op @ (BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply),
                    lhs,
                    rhs,
                    ..
                } => steps.push(NumericStep::Binary {
                    dst: *dst,
                    op: *op,
                    lhs: *lhs,
                    rhs: *rhs,
                }),
                Instruction::Return { src } => result = Some(*src),
                _ => return None,
            }
        }
        Some(Self {
            register_count: function.bytecode().register_count,
            steps,
            result: result?,
        })
    }

    pub(super) fn supports(&self, argument: i64) -> bool {
        self.evaluate_checked(argument).is_some()
    }

    pub(super) fn evaluate(&self, argument: i64) -> i64 {
        self.evaluate_checked(argument)
            .expect("numeric method was checked before helper dispatch")
    }

    fn evaluate_checked(&self, argument: i64) -> Option<i64> {
        let mut registers = vec![0; self.register_count];
        if registers.len() <= 1 {
            return None;
        }
        registers[1] = argument;
        for step in &self.steps {
            match *step {
                NumericStep::Constant { dst, value } => registers[usize::from(dst)] = value,
                NumericStep::Move { dst, src } => {
                    registers[usize::from(dst)] = registers[usize::from(src)];
                }
                NumericStep::Binary { dst, op, lhs, rhs } => {
                    let lhs = registers[usize::from(lhs)];
                    let rhs = registers[usize::from(rhs)];
                    registers[usize::from(dst)] = match op {
                        BinaryOperator::Add => lhs.checked_add(rhs)?,
                        BinaryOperator::Subtract => lhs.checked_sub(rhs)?,
                        BinaryOperator::Multiply => lhs.checked_mul(rhs)?,
                        BinaryOperator::Divide
                        | BinaryOperator::FloorDivide
                        | BinaryOperator::Power => {
                            unreachable!("unsupported numeric plan operator")
                        }
                    };
                }
            }
        }
        registers.get(usize::from(self.result)).copied()
    }
}

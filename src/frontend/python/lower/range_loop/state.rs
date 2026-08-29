use crate::bytecode::{Instruction, Register};
use crate::frontend::python::PythonFrontendError;
use crate::frontend::python::hir::HirExpression;
use crate::object::ObjectKind;
use crate::structure_map::SlotType;

use super::super::Lowerer;
use super::super::comprehension::{ComprehensionLoop, ComprehensionLoopIterator};

pub(super) struct ComprehensionRangeState {
    pub cursor: Register,
    pub cursor_type: SlotType,
    pub stop: Register,
    pub stop_type: SlotType,
    pub step: Register,
    pub step_value: i64,
    pub sequence: Option<Register>,
}

impl Lowerer {
    pub(super) fn initialize_comprehension_range(
        &mut self,
        loop_spec: &ComprehensionLoop<'_>,
    ) -> Result<ComprehensionRangeState, PythonFrontendError> {
        match loop_spec.iterator {
            ComprehensionLoopIterator::Range { start, stop, step } => {
                let (start_value, cursor_type) = self.lower_expression(start)?;
                self.comprehension_expect_integer(cursor_type, start, "range start")?;
                let cursor = self.allocate_register(loop_spec.location)?;
                self.code.push(Instruction::Move {
                    dst: cursor,
                    src: start_value,
                });
                let (stop_value, stop_type) = self.lower_expression(stop)?;
                self.comprehension_expect_integer(stop_type, stop, "range stop")?;
                let stop = self.allocate_register(loop_spec.location)?;
                self.code.push(Instruction::Move {
                    dst: stop,
                    src: stop_value,
                });
                let step_register = self.allocate_register(loop_spec.location)?;
                self.code.push(Instruction::ConstSmallInt {
                    dst: step_register,
                    value: step,
                });
                Ok(ComprehensionRangeState {
                    cursor,
                    cursor_type,
                    stop,
                    stop_type,
                    step: step_register,
                    step_value: step,
                    sequence: None,
                })
            }
            ComprehensionLoopIterator::Sequence(iterable) => {
                let (sequence, sequence_type) = self.lower_expression(iterable)?;
                self.comprehension_expect_iterable(sequence_type, iterable)?;
                let cursor = self.allocate_register(loop_spec.location)?;
                self.code.push(Instruction::ConstSmallInt {
                    dst: cursor,
                    value: 0,
                });
                let stop = self.allocate_register(loop_spec.location)?;
                self.code.push(Instruction::Length {
                    dst: stop,
                    object: sequence,
                });
                let step = self.allocate_register(loop_spec.location)?;
                self.code.push(Instruction::ConstSmallInt {
                    dst: step,
                    value: 1,
                });
                Ok(ComprehensionRangeState {
                    cursor,
                    cursor_type: SlotType::SmallInt,
                    stop,
                    stop_type: SlotType::SmallInt,
                    step,
                    step_value: 1,
                    sequence: Some(sequence),
                })
            }
        }
    }

    fn comprehension_expect_integer(
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

    fn comprehension_expect_iterable(
        &self,
        actual: SlotType,
        expression: &HirExpression,
    ) -> Result<(), PythonFrontendError> {
        if matches!(
            actual,
            SlotType::Any
                | SlotType::Object(ObjectKind::String | ObjectKind::Tuple | ObjectKind::List)
        ) {
            Ok(())
        } else {
            Err(PythonFrontendError::new(
                format!("list comprehension iterable must be a sequence, found {actual:?}"),
                Some(expression.location),
            ))
        }
    }
}

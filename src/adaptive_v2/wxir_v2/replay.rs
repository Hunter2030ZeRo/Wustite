use std::collections::BTreeMap;

use super::VerifiedSnapshot;
use super::ir::{Constant, InstructionKind, NumericComparison, Terminator, ValueId};

const REPLAY_STEP_LIMIT: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplayValue {
    Integer(i64),
    FloatBits(u64),
    Boolean(bool),
    Handle(u32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReplayHeap {
    pub(crate) objects: BTreeMap<u32, BTreeMap<i64, ReplayValue>>,
    pub(crate) lists: BTreeMap<u32, Vec<ReplayValue>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplayOutcome {
    Return(Vec<ReplayValue>),
    SideExit(u32),
    Invalid,
    StepLimit,
}

pub(crate) fn replay(
    snapshot: &VerifiedSnapshot,
    arguments: &[ReplayValue],
    heap: &mut ReplayHeap,
) -> ReplayOutcome {
    let body = snapshot.body();
    let blocks = body
        .blocks
        .iter()
        .map(|block| (block.id, block))
        .collect::<BTreeMap<_, _>>();
    let mut values = BTreeMap::new();
    let mut block_id = body.entry;
    let mut incoming = arguments.to_vec();
    for _ in 0..REPLAY_STEP_LIMIT {
        let Some(block) = blocks.get(&block_id) else {
            return ReplayOutcome::Invalid;
        };
        if block.parameters.len() != incoming.len() {
            return ReplayOutcome::Invalid;
        }
        for (parameter, value) in block.parameters.iter().zip(incoming.drain(..)) {
            values.insert(parameter.id, value);
        }
        for instruction in &block.instructions {
            let inputs = instruction
                .inputs
                .iter()
                .map(|id| values.get(id).copied())
                .collect::<Option<Vec<_>>>();
            let Some(inputs) = inputs else {
                return ReplayOutcome::Invalid;
            };
            let result = execute(instruction.kind.semantic(), &inputs, heap);
            let Ok(result) = result else {
                return ReplayOutcome::Invalid;
            };
            if let (Some(output), Some(value)) = (instruction.output, result) {
                values.insert(output.id, value);
            }
            if let InstructionKind::Guard { guard } = instruction.kind.semantic()
                && inputs.first() != Some(&ReplayValue::Boolean(true))
            {
                return ReplayOutcome::SideExit(*guard);
            }
        }
        match &block.terminator {
            Terminator::Jump { target, arguments } => {
                incoming = lookup(arguments, &values).unwrap_or_default();
                block_id = *target;
            }
            Terminator::Branch { condition, yes, no } => match values.get(condition) {
                Some(ReplayValue::Boolean(true)) => {
                    block_id = *yes;
                }
                Some(ReplayValue::Boolean(false)) => {
                    block_id = *no;
                }
                _ => return ReplayOutcome::Invalid,
            },
            Terminator::Return { values: returned } => {
                return lookup(returned, &values)
                    .map_or(ReplayOutcome::Invalid, ReplayOutcome::Return);
            }
            Terminator::SideExit { id, .. } => return ReplayOutcome::SideExit(*id),
            Terminator::Backedge { .. } | Terminator::IrreducibleBackedge => {
                return ReplayOutcome::Invalid;
            }
        }
    }
    ReplayOutcome::StepLimit
}

fn execute(
    kind: &InstructionKind,
    inputs: &[ReplayValue],
    heap: &mut ReplayHeap,
) -> Result<Option<ReplayValue>, ()> {
    match kind {
        InstructionKind::Constant(Constant::Integer(value)) => {
            Ok(Some(ReplayValue::Integer(*value)))
        }
        InstructionKind::Constant(Constant::Boolean(value)) => {
            Ok(Some(ReplayValue::Boolean(*value)))
        }
        InstructionKind::Constant(Constant::FloatBits(value)) => {
            Ok(Some(ReplayValue::FloatBits(*value)))
        }
        InstructionKind::Copy => inputs.first().copied().map(Some).ok_or(()),
        InstructionKind::Allocate => {
            let next = heap
                .objects
                .keys()
                .chain(heap.lists.keys())
                .copied()
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(())?;
            heap.objects.insert(next, BTreeMap::new());
            Ok(Some(ReplayValue::Handle(next)))
        }
        InstructionKind::IntegerAdd | InstructionKind::Call { .. } => {
            let [ReplayValue::Integer(left), ReplayValue::Integer(right)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Integer(left.wrapping_add(*right))))
        }
        InstructionKind::IntegerSubtract | InstructionKind::IntegerMultiply => {
            let [ReplayValue::Integer(left), ReplayValue::Integer(right)] = inputs else {
                return Err(());
            };
            let value = match kind {
                InstructionKind::IntegerSubtract => left.wrapping_sub(*right),
                InstructionKind::IntegerMultiply => left.wrapping_mul(*right),
                _ => return Err(()),
            };
            Ok(Some(ReplayValue::Integer(value)))
        }
        InstructionKind::IntegerFloorDivide { divisor } => {
            let [ReplayValue::Integer(value)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Integer(value.div_euclid(*divisor))))
        }
        InstructionKind::IntegerToFloat => {
            let [ReplayValue::Integer(value)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::FloatBits((*value as f64).to_bits())))
        }
        InstructionKind::IntegerLessThan => {
            let [ReplayValue::Integer(left), ReplayValue::Integer(right)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Boolean(left < right)))
        }
        InstructionKind::IntegerCompare { comparison } => {
            let [ReplayValue::Integer(left), ReplayValue::Integer(right)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Boolean(compare_i64(
                *comparison,
                *left,
                *right,
            ))))
        }
        InstructionKind::FloatAdd
        | InstructionKind::FloatSubtract
        | InstructionKind::FloatMultiply
        | InstructionKind::FloatDivide
        | InstructionKind::FloatPower => {
            let [ReplayValue::FloatBits(left), ReplayValue::FloatBits(right)] = inputs else {
                return Err(());
            };
            let (left, right) = (f64::from_bits(*left), f64::from_bits(*right));
            let value = match kind {
                InstructionKind::FloatAdd => left + right,
                InstructionKind::FloatSubtract => left - right,
                InstructionKind::FloatMultiply => left * right,
                InstructionKind::FloatDivide => left / right,
                InstructionKind::FloatPower => left.powf(right),
                _ => return Err(()),
            };
            Ok(Some(ReplayValue::FloatBits(value.to_bits())))
        }
        InstructionKind::FloatCompare { comparison } => {
            let [ReplayValue::FloatBits(left), ReplayValue::FloatBits(right)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Boolean(compare_f64(
                *comparison,
                f64::from_bits(*left),
                f64::from_bits(*right),
            ))))
        }
        InstructionKind::IntegerNegate => {
            let [ReplayValue::Integer(value)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Integer(value.wrapping_neg())))
        }
        InstructionKind::FloatNegate => {
            let [ReplayValue::FloatBits(value)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::FloatBits(
                (-f64::from_bits(*value)).to_bits(),
            )))
        }
        InstructionKind::BooleanNot => {
            let [ReplayValue::Boolean(value)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Boolean(!value)))
        }
        InstructionKind::BooleanAnd | InstructionKind::BooleanOr => {
            let [ReplayValue::Boolean(left), ReplayValue::Boolean(right)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Boolean(match kind {
                InstructionKind::BooleanAnd => *left && *right,
                InstructionKind::BooleanOr => *left || *right,
                _ => return Err(()),
            })))
        }
        InstructionKind::Select => {
            let [ReplayValue::Boolean(condition), yes, no] = inputs else {
                return Err(());
            };
            Ok(Some(if *condition { *yes } else { *no }))
        }
        InstructionKind::ObjectGet => {
            let [ReplayValue::Handle(object), ReplayValue::Integer(key)] = inputs else {
                return Err(());
            };
            Ok(heap
                .objects
                .get(object)
                .and_then(|fields| fields.get(key))
                .copied())
        }
        InstructionKind::ObjectSet => {
            let [
                ReplayValue::Handle(object),
                ReplayValue::Integer(key),
                value,
            ] = inputs
            else {
                return Err(());
            };
            heap.objects.get_mut(object).ok_or(())?.insert(*key, *value);
            Ok(None)
        }
        InstructionKind::ListGet => {
            let [ReplayValue::Handle(list), ReplayValue::Integer(index)] = inputs else {
                return Err(());
            };
            let index = usize::try_from(*index).map_err(|_| ())?;
            Ok(heap
                .lists
                .get(list)
                .and_then(|items| items.get(index))
                .copied())
        }
        InstructionKind::ListLength => {
            let [ReplayValue::Handle(list)] = inputs else {
                return Err(());
            };
            Ok(Some(ReplayValue::Integer(
                i64::try_from(heap.lists.get(list).ok_or(())?.len()).map_err(|_| ())?,
            )))
        }
        InstructionKind::ListSet => {
            let [
                ReplayValue::Handle(list),
                ReplayValue::Integer(index),
                value,
            ] = inputs
            else {
                return Err(());
            };
            let index = usize::try_from(*index).map_err(|_| ())?;
            *heap
                .lists
                .get_mut(list)
                .and_then(|items| items.get_mut(index))
                .ok_or(())? = *value;
            Ok(None)
        }
        InstructionKind::ListReversePrefix { element_type } => {
            let [ReplayValue::Handle(list), ReplayValue::Integer(end)] = inputs else {
                return Err(());
            };
            if *element_type != super::ir::ValueType::I64 || *end < 1 {
                return Err(());
            }
            let end = usize::try_from(*end).map_err(|_| ())?;
            let items = heap.lists.get_mut(list).ok_or(())?;
            if end > items.len()
                || items[..end]
                    .iter()
                    .any(|value| !matches!(value, ReplayValue::Integer(_)))
            {
                return Err(());
            }
            items[..end].reverse();
            Ok(None)
        }
        InstructionKind::ListClear => {
            let [ReplayValue::Handle(list)] = inputs else {
                return Err(());
            };
            heap.lists.get_mut(list).ok_or(())?.clear();
            Ok(Some(ReplayValue::Handle(*list)))
        }
        InstructionKind::ListAppend => {
            let [ReplayValue::Handle(list), value] = inputs else {
                return Err(());
            };
            heap.lists.get_mut(list).ok_or(())?.push(*value);
            Ok(None)
        }
        InstructionKind::ListInsert => {
            let [
                ReplayValue::Handle(list),
                ReplayValue::Integer(index),
                value,
            ] = inputs
            else {
                return Err(());
            };
            let items = heap.lists.get_mut(list).ok_or(())?;
            let length = i64::try_from(items.len()).map_err(|_| ())?;
            let index = if *index < 0 {
                length.saturating_add(*index).max(0)
            } else {
                (*index).min(length)
            };
            items.insert(usize::try_from(index).map_err(|_| ())?, *value);
            Ok(Some(ReplayValue::Handle(*list)))
        }
        InstructionKind::ListPop => {
            let [ReplayValue::Handle(list), ReplayValue::Integer(index)] = inputs else {
                return Err(());
            };
            let items = heap.lists.get_mut(list).ok_or(())?;
            let length = i64::try_from(items.len()).map_err(|_| ())?;
            let index = if *index < 0 {
                length.checked_add(*index).ok_or(())?
            } else {
                *index
            };
            Ok(Some(items.remove(usize::try_from(index).map_err(|_| ())?)))
        }
        InstructionKind::Guard { .. }
        | InstructionKind::BranchGuard { .. }
        | InstructionKind::NestedLoopExit { .. }
        | InstructionKind::LiveProbe => Ok(None),
        InstructionKind::AtPc { .. }
        | InstructionKind::Constant(_)
        | InstructionKind::Helper { .. }
        | InstructionKind::BorrowView
        | InstructionKind::ResolveHandle
        | InstructionKind::OwnedList { .. } => Err(()),
    }
}

const fn compare_i64(comparison: NumericComparison, left: i64, right: i64) -> bool {
    match comparison {
        NumericComparison::Equal => left == right,
        NumericComparison::NotEqual => left != right,
        NumericComparison::LessThan => left < right,
        NumericComparison::LessEqual => left <= right,
        NumericComparison::GreaterThan => left > right,
        NumericComparison::GreaterEqual => left >= right,
    }
}

fn compare_f64(comparison: NumericComparison, left: f64, right: f64) -> bool {
    match comparison {
        NumericComparison::Equal => left == right,
        NumericComparison::NotEqual => left != right,
        NumericComparison::LessThan => left < right,
        NumericComparison::LessEqual => left <= right,
        NumericComparison::GreaterThan => left > right,
        NumericComparison::GreaterEqual => left >= right,
    }
}

fn lookup(ids: &[ValueId], values: &BTreeMap<ValueId, ReplayValue>) -> Option<Vec<ReplayValue>> {
    ids.iter().map(|id| values.get(id).copied()).collect()
}

#[cfg(test)]
mod tests {
    use super::{ReplayHeap, ReplayValue, execute};
    use crate::adaptive_v2::wxir_v2::ir::{InstructionKind, ValueType};

    #[test]
    fn reverse_prefix_replay_validates_pre_mutation() {
        let operation = InstructionKind::ListReversePrefix {
            element_type: ValueType::I64,
        };
        let original = vec![
            ReplayValue::Integer(1),
            ReplayValue::Integer(-2),
            ReplayValue::Integer(1),
            ReplayValue::Integer(4),
        ];
        for invalid in [i64::MIN, -1, 0, 5, i64::MAX] {
            let mut heap = ReplayHeap::default();
            heap.lists.insert(7, original.clone());
            assert!(
                execute(
                    &operation,
                    &[ReplayValue::Handle(7), ReplayValue::Integer(invalid)],
                    &mut heap,
                )
                .is_err()
            );
            assert_eq!(heap.lists[&7], original);
        }
    }

    #[test]
    fn reverse_prefix_replay_handles_unit_odd_full_lengths() {
        let operation = InstructionKind::ListReversePrefix {
            element_type: ValueType::I64,
        };
        for (end, expected) in [
            (1, vec![1, 2, 3, 4]),
            (3, vec![3, 2, 1, 4]),
            (4, vec![4, 3, 2, 1]),
        ] {
            let mut heap = ReplayHeap::default();
            heap.lists.insert(
                7,
                [1, 2, 3, 4].into_iter().map(ReplayValue::Integer).collect(),
            );
            assert_eq!(
                execute(
                    &operation,
                    &[ReplayValue::Handle(7), ReplayValue::Integer(end)],
                    &mut heap,
                ),
                Ok(None)
            );
            assert_eq!(
                heap.lists[&7],
                expected
                    .into_iter()
                    .map(ReplayValue::Integer)
                    .collect::<Vec<_>>()
            );
        }
    }
}

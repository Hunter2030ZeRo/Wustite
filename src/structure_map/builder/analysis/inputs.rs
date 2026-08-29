use crate::bytecode::{Instruction, Register};

use super::super::super::{ValueId, ValueUse};
use super::input_for;

pub(super) fn input_registers(instruction: &Instruction) -> Vec<Register> {
    match instruction {
        Instruction::BinaryOp { lhs, rhs, .. }
        | Instruction::CompareOp { lhs, rhs, .. }
        | Instruction::BooleanOp { lhs, rhs, .. }
        | Instruction::AddI64 { lhs, rhs, .. }
        | Instruction::LtI64 { lhs, rhs, .. } => vec![*lhs, *rhs],
        Instruction::UnaryOp { src, .. }
        | Instruction::Move { src, .. }
        | Instruction::Return { src }
        | Instruction::Branch { cond: src, .. }
        | Instruction::Length { object: src, .. } => vec![*src],
        Instruction::BuildTuple { items, .. } | Instruction::BuildList { items, .. } => {
            items.clone()
        }
        Instruction::BuildDict { entries, .. } => entries
            .iter()
            .flat_map(|(key, value)| [*key, *value])
            .collect(),
        Instruction::GetItem { object, key, .. } => vec![*object, *key],
        Instruction::GetAttr { object, .. } => vec![*object],
        Instruction::GetSlice {
            object,
            start,
            stop,
            step,
            ..
        } => std::iter::once(*object)
            .chain([*start, *stop, *step].into_iter().flatten())
            .collect(),
        Instruction::SetSlice {
            object,
            start,
            stop,
            step,
            value,
        } => std::iter::once(*object)
            .chain([*start, *stop, *step].into_iter().flatten())
            .chain(std::iter::once(*value))
            .collect(),
        Instruction::SetItem {
            object, key, value, ..
        } => vec![*object, *key, *value],
        Instruction::SetAttr { object, value, .. } => vec![*object, *value],
        Instruction::ListAppend { list, value } => vec![*list, *value],
        Instruction::ListInsert { list, index, value } => vec![*list, *index, *value],
        Instruction::ListPop { list, index, .. } => vec![*list, *index],
        Instruction::Call { callable, args, .. } => std::iter::once(*callable)
            .chain(args.iter().copied())
            .collect(),
        Instruction::CallMethod { receiver, args, .. } => std::iter::once(*receiver)
            .chain(args.iter().copied())
            .collect(),
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstFloat { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstNone { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::LoadCurrentFunction { .. }
        | Instruction::Jump { .. } => Vec::new(),
    }
}

pub(super) fn mutated_values(instruction: &Instruction, inputs: &[ValueUse]) -> Vec<ValueId> {
    let target = match instruction {
        Instruction::SetItem { object, .. }
        | Instruction::SetAttr { object, .. }
        | Instruction::SetSlice { object, .. } => Some(*object),
        Instruction::ListAppend { list, .. }
        | Instruction::ListInsert { list, .. }
        | Instruction::ListPop { list, .. } => Some(*list),
        _ => None,
    };
    target
        .and_then(|register| input_for(inputs, register))
        .and_then(|input| input.value)
        .into_iter()
        .collect()
}

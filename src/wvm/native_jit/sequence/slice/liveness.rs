use crate::bytecode::{Instruction, Register};

pub(crate) fn temporary_is_dead(code: &[Instruction], start: usize, register: Register) -> bool {
    for instruction in code.iter().skip(start) {
        if reads(instruction, register) {
            return false;
        }
        if writes(instruction, register) {
            return true;
        }
    }
    true
}

fn reads(instruction: &Instruction, register: Register) -> bool {
    let contains = |registers: &[Register]| registers.contains(&register);
    match instruction {
        Instruction::BinaryOp { lhs, rhs, .. }
        | Instruction::CompareOp { lhs, rhs, .. }
        | Instruction::BooleanOp { lhs, rhs, .. }
        | Instruction::AddI64 { lhs, rhs, .. }
        | Instruction::LtI64 { lhs, rhs, .. } => [*lhs, *rhs].contains(&register),
        Instruction::UnaryOp { src, .. } | Instruction::Move { src, .. } => *src == register,
        Instruction::BuildTuple { items, .. } | Instruction::BuildList { items, .. } => {
            contains(items)
        }
        Instruction::BuildDict { entries, .. } => entries
            .iter()
            .any(|(key, value)| *key == register || *value == register),
        Instruction::GetItem { object, key, .. } => [*object, *key].contains(&register),
        Instruction::GetAttr { object, .. } => *object == register,
        Instruction::GetSlice {
            object,
            start,
            stop,
            step,
            ..
        }
        | Instruction::SetSlice {
            object,
            start,
            stop,
            step,
            ..
        } => {
            *object == register
                || [*start, *stop, *step]
                    .into_iter()
                    .flatten()
                    .any(|candidate| candidate == register)
                || matches!(instruction, Instruction::SetSlice { value, .. } if *value == register)
        }
        Instruction::SetItem {
            object, key, value, ..
        }
        | Instruction::ListInsert {
            list: object,
            index: key,
            value,
        } => [*object, *key, *value].contains(&register),
        Instruction::SetAttr { object, value, .. } => [*object, *value].contains(&register),
        Instruction::ListAppend { list, value } => [*list, *value].contains(&register),
        Instruction::ListPop { list, index, .. } => [*list, *index].contains(&register),
        Instruction::Length { object, .. } => *object == register,
        Instruction::Call { callable, args, .. } => *callable == register || contains(args),
        Instruction::CallMethod { receiver, args, .. } => *receiver == register || contains(args),
        Instruction::Branch { cond, .. } => *cond == register,
        Instruction::Return { src } => *src == register,
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstFloat { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstNone { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::LoadCurrentFunction { .. }
        | Instruction::Jump { .. } => false,
    }
}

fn writes(instruction: &Instruction, register: Register) -> bool {
    match instruction {
        Instruction::ConstSmallInt { dst, .. }
        | Instruction::ConstFloat { dst, .. }
        | Instruction::ConstBool { dst, .. }
        | Instruction::ConstNone { dst }
        | Instruction::LoadConstant { dst, .. }
        | Instruction::ConstI64 { dst, .. }
        | Instruction::BinaryOp { dst, .. }
        | Instruction::CompareOp { dst, .. }
        | Instruction::UnaryOp { dst, .. }
        | Instruction::BooleanOp { dst, .. }
        | Instruction::BuildTuple { dst, .. }
        | Instruction::BuildList { dst, .. }
        | Instruction::BuildDict { dst, .. }
        | Instruction::GetItem { dst, .. }
        | Instruction::GetAttr { dst, .. }
        | Instruction::GetSlice { dst, .. }
        | Instruction::ListPop { dst, .. }
        | Instruction::Length { dst, .. }
        | Instruction::LoadCurrentFunction { dst }
        | Instruction::Call { dst, .. }
        | Instruction::CallMethod { dst, .. }
        | Instruction::AddI64 { dst, .. }
        | Instruction::LtI64 { dst, .. }
        | Instruction::Move { dst, .. } => *dst == register,
        Instruction::SetItem { .. }
        | Instruction::SetAttr { .. }
        | Instruction::SetSlice { .. }
        | Instruction::ListAppend { .. }
        | Instruction::ListInsert { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => false,
    }
}

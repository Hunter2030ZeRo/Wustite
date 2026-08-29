use std::collections::{HashSet, VecDeque};

use crate::bytecode::{Instruction, Register};
use crate::executable::ExecutableFunction;

pub(super) fn verify_definite_assignment(function: &ExecutableFunction) -> Result<(), String> {
    let code = &function.bytecode().code;
    if code.is_empty() {
        return Err("function has no Return instruction".to_owned());
    }

    let entry = function
        .parameters()
        .iter()
        .map(|parameter| parameter.register)
        .collect::<HashSet<_>>();
    let mut incoming = vec![None; code.len()];
    incoming[0] = Some(entry);
    let mut worklist = VecDeque::from([0]);

    while let Some(pc) = worklist.pop_front() {
        let mut outgoing = incoming[pc]
            .as_ref()
            .ok_or_else(|| format!("instruction {pc} has no incoming assignment state"))?
            .clone();
        if let Some(register) = written_register(&code[pc]) {
            outgoing.insert(register);
        }

        match code[pc] {
            Instruction::Return { .. } => {}
            Instruction::Jump { target } => {
                propagate(target, &outgoing, &mut incoming, &mut worklist);
            }
            Instruction::Branch { yes, no, .. } => {
                propagate(yes, &outgoing, &mut incoming, &mut worklist);
                propagate(no, &outgoing, &mut incoming, &mut worklist);
            }
            _ => {
                let next = pc + 1;
                if next == code.len() {
                    return Err(format!(
                        "reachable path falls off after instruction {pc} without Return"
                    ));
                }
                propagate(next, &outgoing, &mut incoming, &mut worklist);
            }
        }
    }

    for (pc, state) in incoming.iter().enumerate() {
        if let Some(assigned) = state {
            verify_reads(&code[pc], assigned, pc)?;
        }
    }
    Ok(())
}

fn propagate(
    target: usize,
    outgoing: &HashSet<Register>,
    incoming: &mut [Option<HashSet<Register>>],
    worklist: &mut VecDeque<usize>,
) {
    match &mut incoming[target] {
        Some(current) => {
            let previous_len = current.len();
            current.retain(|register| outgoing.contains(register));
            if current.len() != previous_len {
                worklist.push_back(target);
            }
        }
        slot @ None => {
            *slot = Some(outgoing.clone());
            worklist.push_back(target);
        }
    }
}

const fn written_register(instruction: &Instruction) -> Option<Register> {
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
        | Instruction::Move { dst, .. } => Some(*dst),
        Instruction::SetItem { .. }
        | Instruction::SetAttr { .. }
        | Instruction::SetSlice { .. }
        | Instruction::ListAppend { .. }
        | Instruction::ListInsert { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => None,
    }
}

fn verify_reads(
    instruction: &Instruction,
    assigned: &HashSet<Register>,
    pc: usize,
) -> Result<(), String> {
    match instruction {
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstFloat { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstNone { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::LoadCurrentFunction { .. }
        | Instruction::Jump { .. } => Ok(()),
        Instruction::BinaryOp { lhs, rhs, .. } | Instruction::CompareOp { lhs, rhs, .. } => {
            verify_read(*lhs, assigned, pc, "semantic operation lhs")?;
            verify_read(*rhs, assigned, pc, "semantic operation rhs")
        }
        Instruction::UnaryOp { src, .. } => verify_read(*src, assigned, pc, "UnaryOp src"),
        Instruction::BooleanOp { lhs, rhs, .. }
        | Instruction::AddI64 { lhs, rhs, .. }
        | Instruction::LtI64 { lhs, rhs, .. } => {
            verify_read(*lhs, assigned, pc, "binary operation lhs")?;
            verify_read(*rhs, assigned, pc, "binary operation rhs")
        }
        Instruction::BuildTuple { items, .. } | Instruction::BuildList { items, .. } => {
            verify_read_slice(items, assigned, pc, "collection item")
        }
        Instruction::BuildDict { entries, .. } => {
            for (key, value) in entries {
                verify_read(*key, assigned, pc, "BuildDict key")?;
                verify_read(*value, assigned, pc, "BuildDict value")?;
            }
            Ok(())
        }
        Instruction::GetItem { object, key, .. } => {
            verify_read(*object, assigned, pc, "GetItem object")?;
            verify_read(*key, assigned, pc, "GetItem key")
        }
        Instruction::GetAttr { object, .. } => verify_read(*object, assigned, pc, "GetAttr object"),
        Instruction::GetSlice {
            object,
            start,
            stop,
            step,
            ..
        } => {
            verify_read(*object, assigned, pc, "GetSlice object")?;
            verify_optional_read(*start, assigned, pc, "GetSlice start")?;
            verify_optional_read(*stop, assigned, pc, "GetSlice stop")?;
            verify_optional_read(*step, assigned, pc, "GetSlice step")
        }
        Instruction::SetItem { object, key, value } => {
            verify_read(*object, assigned, pc, "SetItem object")?;
            verify_read(*key, assigned, pc, "SetItem key")?;
            verify_read(*value, assigned, pc, "SetItem value")
        }
        Instruction::SetAttr { object, value, .. } => {
            verify_read(*object, assigned, pc, "SetAttr object")?;
            verify_read(*value, assigned, pc, "SetAttr value")
        }
        Instruction::SetSlice {
            object,
            start,
            stop,
            step,
            value,
        } => {
            verify_read(*object, assigned, pc, "SetSlice object")?;
            verify_optional_read(*start, assigned, pc, "SetSlice start")?;
            verify_optional_read(*stop, assigned, pc, "SetSlice stop")?;
            verify_optional_read(*step, assigned, pc, "SetSlice step")?;
            verify_read(*value, assigned, pc, "SetSlice value")
        }
        Instruction::ListAppend { list, value } => {
            verify_read(*list, assigned, pc, "ListAppend list")?;
            verify_read(*value, assigned, pc, "ListAppend value")
        }
        Instruction::ListInsert { list, index, value } => {
            verify_read(*list, assigned, pc, "ListInsert list")?;
            verify_read(*index, assigned, pc, "ListInsert index")?;
            verify_read(*value, assigned, pc, "ListInsert value")
        }
        Instruction::ListPop { list, index, .. } => {
            verify_read(*list, assigned, pc, "ListPop list")?;
            verify_read(*index, assigned, pc, "ListPop index")
        }
        Instruction::Length { object, .. } => verify_read(*object, assigned, pc, "Length object"),
        Instruction::Call { callable, args, .. } => {
            verify_read(*callable, assigned, pc, "Call callable")?;
            verify_read_slice(args, assigned, pc, "Call argument")
        }
        Instruction::CallMethod { receiver, args, .. } => {
            verify_read(*receiver, assigned, pc, "CallMethod receiver")?;
            verify_read_slice(args, assigned, pc, "CallMethod argument")
        }
        Instruction::Branch { cond, .. } => verify_read(*cond, assigned, pc, "Branch cond"),
        Instruction::Return { src } => verify_read(*src, assigned, pc, "Return src"),
        Instruction::Move { src, .. } => verify_read(*src, assigned, pc, "Move src"),
    }
}

fn verify_optional_read(
    register: Option<Register>,
    assigned: &HashSet<Register>,
    pc: usize,
    context: &str,
) -> Result<(), String> {
    register.map_or(Ok(()), |register| {
        verify_read(register, assigned, pc, context)
    })
}

fn verify_read_slice(
    registers: &[Register],
    assigned: &HashSet<Register>,
    pc: usize,
    context: &str,
) -> Result<(), String> {
    for register in registers {
        verify_read(*register, assigned, pc, context)?;
    }
    Ok(())
}

fn verify_read(
    register: Register,
    assigned: &HashSet<Register>,
    pc: usize,
    context: &str,
) -> Result<(), String> {
    if assigned.contains(&register) {
        Ok(())
    } else {
        Err(format!(
            "instruction {pc} {context} reads uninitialized register r{register}"
        ))
    }
}

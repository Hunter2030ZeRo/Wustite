mod initialization;
mod structure;

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use crate::bytecode::{Function, Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::structure_map::OperationSiteId;

pub const MAX_REGISTER_COUNT: usize = 1usize << u16::BITS;

#[cfg(test)]
std::thread_local! {
    static FULL_VERIFICATION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn reset_full_verification_count() {
    FULL_VERIFICATION_COUNT.set(0);
}

#[cfg(test)]
fn full_verification_count() -> usize {
    FULL_VERIFICATION_COUNT.get()
}

pub fn verify(function: &ExecutableFunction) -> Result<(), String> {
    function
        .verification_cache()
        .get_or_init(|| verify_uncached(function))
        .clone()
}

fn verify_uncached(function: &ExecutableFunction) -> Result<(), String> {
    #[cfg(test)]
    FULL_VERIFICATION_COUNT.set(FULL_VERIFICATION_COUNT.get() + 1);

    if function.bytecode().register_count > MAX_REGISTER_COUNT {
        return Err(format!(
            "function register_count {} exceeds maximum {MAX_REGISTER_COUNT}",
            function.bytecode().register_count
        ));
    }
    verify_parameters(function)?;
    let bytecode = function.bytecode();
    let mut used_operation_sites = HashSet::new();

    for (pc, instruction) in bytecode.code.iter().enumerate() {
        match instruction {
            Instruction::ConstSmallInt { dst, .. }
            | Instruction::ConstFloat { dst, .. }
            | Instruction::ConstBool { dst, .. }
            | Instruction::ConstI64 { dst, .. }
            | Instruction::LoadCurrentFunction { dst } => {
                verify_register(bytecode, *dst, pc, "constant dst")?;
            }
            Instruction::LoadConstant { dst, constant } => {
                verify_register(bytecode, *dst, pc, "LoadConstant dst")?;
                if constant.0 >= function.constants().len() {
                    return Err(format!(
                        "instruction {pc} LoadConstant uses invalid constant {}",
                        constant.0
                    ));
                }
            }
            Instruction::BinaryOp {
                dst,
                lhs,
                rhs,
                site,
                ..
            }
            | Instruction::CompareOp {
                dst,
                lhs,
                rhs,
                site,
                ..
            } => {
                verify_register(bytecode, *dst, pc, "semantic operation dst")?;
                verify_register(bytecode, *lhs, pc, "semantic operation lhs")?;
                verify_register(bytecode, *rhs, pc, "semantic operation rhs")?;
                verify_operation_site(function, *site, pc)?;
                if !used_operation_sites.insert(*site) {
                    return Err(format!(
                        "operation site {} is referenced by more than one instruction",
                        site.0
                    ));
                }
            }
            Instruction::UnaryOp { dst, src, .. } => {
                verify_register(bytecode, *dst, pc, "UnaryOp dst")?;
                verify_register(bytecode, *src, pc, "UnaryOp src")?;
            }
            Instruction::BooleanOp { dst, lhs, rhs, .. }
            | Instruction::AddI64 { dst, lhs, rhs }
            | Instruction::LtI64 { dst, lhs, rhs } => {
                verify_register(bytecode, *dst, pc, "binary operation dst")?;
                verify_register(bytecode, *lhs, pc, "binary operation lhs")?;
                verify_register(bytecode, *rhs, pc, "binary operation rhs")?;
            }
            Instruction::BuildTuple { dst, items } | Instruction::BuildList { dst, items } => {
                verify_register(bytecode, *dst, pc, "collection dst")?;
                verify_registers(bytecode, items, pc, "collection item")?;
            }
            Instruction::BuildDict { dst, entries } => {
                verify_register(bytecode, *dst, pc, "BuildDict dst")?;
                for (key, value) in entries {
                    verify_register(bytecode, *key, pc, "BuildDict key")?;
                    verify_register(bytecode, *value, pc, "BuildDict value")?;
                }
            }
            Instruction::GetItem { dst, object, key } => {
                verify_register(bytecode, *dst, pc, "GetItem dst")?;
                verify_register(bytecode, *object, pc, "GetItem object")?;
                verify_register(bytecode, *key, pc, "GetItem key")?;
            }
            Instruction::SetItem { object, key, value } => {
                verify_register(bytecode, *object, pc, "SetItem object")?;
                verify_register(bytecode, *key, pc, "SetItem key")?;
                verify_register(bytecode, *value, pc, "SetItem value")?;
            }
            Instruction::Length { dst, object } => {
                verify_register(bytecode, *dst, pc, "Length dst")?;
                verify_register(bytecode, *object, pc, "Length object")?;
            }
            Instruction::Call {
                dst,
                callable,
                args,
            } => {
                verify_register(bytecode, *dst, pc, "Call dst")?;
                verify_register(bytecode, *callable, pc, "Call callable")?;
                verify_registers(bytecode, args, pc, "Call argument")?;
            }
            Instruction::Jump { target } => verify_target(bytecode, *target, pc, "Jump")?,
            Instruction::Branch { cond, yes, no } => {
                verify_register(bytecode, *cond, pc, "Branch cond")?;
                verify_target(bytecode, *yes, pc, "Branch yes")?;
                verify_target(bytecode, *no, pc, "Branch no")?;
            }
            Instruction::Return { src } => {
                verify_register(bytecode, *src, pc, "Return src")?;
            }
            Instruction::Move { dst, src } => {
                verify_register(bytecode, *dst, pc, "Move dst")?;
                verify_register(bytecode, *src, pc, "Move src")?;
            }
        }
    }

    structure::verify_structure_map(function, &used_operation_sites)?;
    initialization::verify_definite_assignment(function)
}

fn verify_parameters(function: &ExecutableFunction) -> Result<(), String> {
    let mut registers = HashSet::new();
    let mut names = HashSet::new();
    for parameter in function.parameters() {
        verify_register(
            function.bytecode(),
            parameter.register,
            0,
            "function parameter",
        )?;
        if !registers.insert(parameter.register) {
            return Err(format!(
                "function parameters map to duplicate register r{}",
                parameter.register
            ));
        }
        if !names.insert(&parameter.name) {
            return Err(format!(
                "function parameter `{}` is defined more than once",
                parameter.name
            ));
        }
    }
    Ok(())
}

fn verify_operation_site(
    function: &ExecutableFunction,
    site: OperationSiteId,
    pc: usize,
) -> Result<(), String> {
    let metadata = function
        .structure_map()
        .operation_site(site)
        .ok_or_else(|| {
            format!(
                "instruction {pc} references unknown operation site {}",
                site.0
            )
        })?;
    if metadata.pc != pc {
        return Err(format!(
            "operation site {} belongs to pc {}, not instruction {pc}",
            site.0, metadata.pc
        ));
    }
    Ok(())
}

fn verify_registers(
    function: &Function,
    registers: &[Register],
    pc: usize,
    context: &str,
) -> Result<(), String> {
    for register in registers {
        verify_register(function, *register, pc, context)?;
    }
    Ok(())
}

pub(super) fn verify_register(
    function: &Function,
    register: Register,
    pc: usize,
    context: &str,
) -> Result<(), String> {
    if usize::from(register) < function.register_count {
        Ok(())
    } else {
        Err(format!(
            "instruction {pc} {context} uses invalid register r{register}"
        ))
    }
}

fn verify_target(
    function: &Function,
    target: usize,
    pc: usize,
    context: &str,
) -> Result<(), String> {
    if target < function.code.len() {
        Ok(())
    } else {
        Err(format!(
            "instruction {pc} {context} has invalid target {target}"
        ))
    }
}

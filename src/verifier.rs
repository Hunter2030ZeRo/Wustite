use std::collections::HashSet;

use crate::bytecode::{Function, Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::structure_map::OperationSiteId;

pub fn verify(function: &ExecutableFunction) -> Result<(), String> {
    let bytecode = function.bytecode();
    let mut used_operation_sites = HashSet::new();

    for (pc, instruction) in bytecode.code.iter().enumerate() {
        match instruction {
            Instruction::ConstI64 { dst, .. } => {
                verify_register(bytecode, *dst, pc, "ConstI64 dst")?;
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
            Instruction::AddI64 { dst, lhs, rhs } | Instruction::LtI64 { dst, lhs, rhs } => {
                verify_register(bytecode, *dst, pc, "arithmetic dst")?;
                verify_register(bytecode, *lhs, pc, "arithmetic lhs")?;
                verify_register(bytecode, *rhs, pc, "arithmetic rhs")?;
            }
            Instruction::Jump { target } => {
                verify_target(bytecode, *target, pc, "Jump")?;
            }
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

    for (index, site) in function
        .structure_map()
        .operation_sites
        .iter()
        .enumerate()
    {
        let id = OperationSiteId(
            u32::try_from(index)
                .map_err(|_| "StructureMap contains too many operation sites".to_string())?,
        );
        if site.pc >= bytecode.code.len() {
            return Err(format!(
                "operation site {} points outside bytecode at pc {}",
                id.0, site.pc
            ));
        }
        if !used_operation_sites.contains(&id) {
            return Err(format!("operation site {} is not referenced by bytecode", id.0));
        }
    }

    let mut loop_headers = HashSet::new();
    for (loop_index, region) in function.structure_map().loops.iter().enumerate() {
        verify_region_target(bytecode, region.header, loop_index, "header")?;
        verify_region_target(bytecode, region.backedge, loop_index, "backedge")?;

        if !loop_headers.insert(region.header) {
            return Err(format!(
                "loop {loop_index} duplicates loop header {}",
                region.header
            ));
        }

        if region.backedge < region.header {
            return Err(format!(
                "loop {loop_index} backedge {} precedes header {}",
                region.backedge, region.header
            ));
        }

        let mut exit_targets = HashSet::new();
        for (exit_id, exit) in region.exits.iter().enumerate() {
            verify_region_target(
                bytecode,
                exit.target,
                loop_index,
                &format!("exit {exit_id}"),
            )?;
            if !exit_targets.insert(exit.target) {
                return Err(format!(
                    "loop {loop_index} has duplicate exit target {}",
                    exit.target
                ));
            }
        }

        match &bytecode.code[region.backedge] {
            Instruction::Jump { target } if *target == region.header => {}
            Instruction::Jump { target } => {
                return Err(format!(
                    "loop {loop_index} backedge {} jumps to {target}, not header {}",
                    region.backedge, region.header
                ));
            }
            _ => {
                return Err(format!(
                    "loop {loop_index} backedge {} is not a Jump",
                    region.backedge
                ));
            }
        }

        let mut live_registers = HashSet::new();
        for slot in &region.live_slots {
            verify_register(bytecode, slot.register, region.header, "loop live slot")?;

            if !live_registers.insert(slot.register) {
                return Err(format!(
                    "loop {loop_index} has duplicate live slot for r{}",
                    slot.register
                ));
            }
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
        .ok_or_else(|| format!("instruction {pc} references unknown operation site {}", site.0))?;
    if metadata.pc != pc {
        return Err(format!(
            "operation site {} belongs to pc {}, not instruction {pc}",
            site.0, metadata.pc
        ));
    }
    Ok(())
}

fn verify_register(
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

fn verify_region_target(
    function: &Function,
    target: usize,
    loop_index: usize,
    context: &str,
) -> Result<(), String> {
    if target < function.code.len() {
        Ok(())
    } else {
        Err(format!(
            "loop {loop_index} {context} has invalid target {target}"
        ))
    }
}

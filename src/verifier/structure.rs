use std::collections::HashSet;

use crate::bytecode::Instruction;
use crate::executable::ExecutableFunction;
use crate::structure_map::OperationSiteId;

use super::verify_register;

pub(super) fn verify_structure_map(
    function: &ExecutableFunction,
    used_operation_sites: &HashSet<OperationSiteId>,
) -> Result<(), String> {
    let bytecode = function.bytecode();
    for (index, site) in function.structure_map().operation_sites.iter().enumerate() {
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
            return Err(format!(
                "operation site {} is not referenced by bytecode",
                id.0
            ));
        }
    }

    let mut loop_headers = HashSet::new();
    for (loop_index, region) in function.structure_map().regions.iter().enumerate() {
        verify_region_target(function, region.entry, loop_index, "header")?;
        verify_region_target(
            function,
            region.backedge.unwrap_or(0),
            loop_index,
            "backedge",
        )?;
        if !loop_headers.insert(region.entry) {
            return Err(format!(
                "loop {loop_index} duplicates loop header {}",
                region.entry
            ));
        }
        if region.backedge < Some(region.entry) {
            return Err(format!(
                "loop {loop_index} backedge {} precedes header {}",
                region.backedge.unwrap_or(0),
                region.entry
            ));
        }

        let mut exit_targets = HashSet::new();
        for (exit_id, exit) in region.exits.iter().enumerate() {
            verify_region_target(
                function,
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

        match bytecode.code.get(region.backedge.unwrap_or(0)) {
            Some(Instruction::Jump { target }) if *target == region.entry => {}
            Some(Instruction::Jump { target }) => {
                return Err(format!(
                    "loop {loop_index} backedge {} jumps to {target}, not header {}",
                    region.backedge.unwrap_or(0),
                    region.entry
                ));
            }
            Some(_) | None => {
                return Err(format!(
                    "loop {loop_index} backedge {} is not a Jump",
                    region.backedge.unwrap_or(0)
                ));
            }
        }

        let mut live_registers = HashSet::new();
        for slot in &region.live_slots {
            verify_register(bytecode, slot.register, region.entry, "loop live slot")?;
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

fn verify_region_target(
    function: &ExecutableFunction,
    target: usize,
    loop_index: usize,
    context: &str,
) -> Result<(), String> {
    if target < function.bytecode().code.len() {
        Ok(())
    } else {
        Err(format!(
            "loop {loop_index} {context} has invalid target {target}"
        ))
    }
}

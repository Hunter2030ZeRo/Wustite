use std::collections::HashSet;

use crate::bytecode::Instruction;
use crate::executable::ExecutableFunction;
use crate::structure_map::{OperationSiteId, RegionKind};

use super::verify_register;

pub(super) fn verify_structure_map(
    function: &ExecutableFunction,
    used_operation_sites: &HashSet<OperationSiteId>,
) -> Result<(), String> {
    let bytecode = function.bytecode();
    let structure_map = function.structure_map();
    for (index, site) in structure_map.operation_sites().iter().enumerate() {
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

    verify_cfg(function)?;
    for (region_index, region) in structure_map.regions().iter().enumerate() {
        verify_region_target(function, region.entry, region_index, "header")?;
        if structure_map.blocks().is_empty() {
            continue;
        }
        let entry_block = structure_map.block_by_pc(region.entry).ok_or_else(|| {
            format!(
                "region {region_index} header {} is not covered by a basic block",
                region.entry
            )
        })?;
        if !region.blocks.contains(&entry_block.id) {
            return Err(format!(
                "region {region_index} does not contain its header block {}",
                entry_block.id.0
            ));
        }
        for block_id in &region.blocks {
            if structure_map.block(*block_id).is_none() {
                return Err(format!(
                    "region {region_index} references unknown block {}",
                    block_id.0
                ));
            }
        }
    }

    let mut loop_headers = HashSet::new();
    for (region_id, region) in structure_map.loop_regions() {
        let loop_index = region_id.0;
        let RegionKind::Loop { backedge } = region.kind else {
            continue;
        };
        verify_region_target(function, backedge, loop_index, "backedge")?;
        if !loop_headers.insert(region.entry) {
            return Err(format!(
                "loop {loop_index} duplicates loop header {}",
                region.entry
            ));
        }
        if backedge < region.entry {
            return Err(format!(
                "loop {loop_index} backedge {} precedes header {}",
                backedge, region.entry
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

        match bytecode.code.get(backedge) {
            Some(Instruction::Jump { target }) if *target == region.entry => {}
            Some(Instruction::Jump { target }) => {
                return Err(format!(
                    "loop {loop_index} backedge {} jumps to {target}, not header {}",
                    backedge, region.entry
                ));
            }
            Some(_) | None => {
                return Err(format!(
                    "loop {loop_index} backedge {} is not a Jump",
                    backedge
                ));
            }
        }

        let mut live_registers = HashSet::new();
        for slot in &region.entry_summary {
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

fn verify_cfg(function: &ExecutableFunction) -> Result<(), String> {
    let structure_map = function.structure_map();
    if structure_map.blocks().is_empty() {
        return Ok(());
    }

    for (pc, _) in function.bytecode().code.iter().enumerate() {
        if structure_map.block_by_pc(pc).is_none() {
            return Err(format!("basic blocks do not cover bytecode pc {pc}"));
        }
    }

    for block in structure_map.blocks() {
        if block.start_pc >= block.end_pc || block.end_pc > function.bytecode().code.len() {
            return Err(format!(
                "basic block {} has invalid pc range {}..{}",
                block.id.0, block.start_pc, block.end_pc
            ));
        }
        for edge in &block.successors {
            let target = structure_map.block(edge.target).ok_or_else(|| {
                format!(
                    "basic block {} has an edge to unknown block {}",
                    block.id.0, edge.target.0
                )
            })?;
            if !target.predecessors.contains(&block.id) {
                return Err(format!(
                    "basic block {} is missing from successor {} predecessors",
                    block.id.0, edge.target.0
                ));
            }
        }
        for predecessor in &block.predecessors {
            let source = structure_map.block(*predecessor).ok_or_else(|| {
                format!(
                    "basic block {} has an unknown predecessor {}",
                    block.id.0, predecessor.0
                )
            })?;
            if !source.successors.iter().any(|edge| edge.target == block.id) {
                return Err(format!(
                    "basic block {} is missing from predecessor {} successors",
                    block.id.0, predecessor.0
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

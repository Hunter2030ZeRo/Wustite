use crate::bytecode::Instruction;

use super::{BasicBlock, Region, RegionKind, RegionSummary, StructureMap, StructureMapBuilder};

mod analysis;
mod cfg;

struct RegionFacts<'a> {
    blocks: &'a [BasicBlock],
    operation_prefix: &'a [usize],
    call_prefix: &'a [usize],
    code_len: usize,
}

pub(super) fn finish(
    builder: StructureMapBuilder,
    code: &[Instruction],
    register_count: usize,
) -> Result<StructureMap, String> {
    validate_drafts(&builder, code.len(), register_count)?;

    let region_entries = builder
        .regions
        .iter()
        .map(|draft| draft.entry)
        .collect::<Vec<_>>();
    let cfg = cfg::build(code, &region_entries)?;
    let blocks = cfg.blocks;
    let block_by_pc = cfg.block_by_pc;
    let (mut regions, region_by_entry_pc) = build_regions(
        builder.regions.clone(),
        RegionFacts {
            blocks: &blocks,
            operation_prefix: &cfg.operation_prefix,
            call_prefix: &cfg.call_prefix,
            code_len: code.len(),
        },
    )?;
    let analysis = analysis::analyze(
        &builder,
        code,
        register_count,
        &blocks,
        &block_by_pc,
        &mut regions,
    )?;

    Ok(StructureMap {
        blocks,
        regions,
        operation_sites: builder.operation_sites,
        values: analysis.values,
        instructions: analysis.instructions,
        block_by_pc,
        region_by_entry_pc,
    })
}

fn validate_drafts(
    builder: &StructureMapBuilder,
    code_len: usize,
    register_count: usize,
) -> Result<(), String> {
    for (id, draft) in builder.regions.iter().enumerate() {
        validate_target(draft.entry, code_len, draft.entry)?;
        for slot in &draft.entry_summary {
            if usize::from(slot.register) >= register_count {
                return Err(format!(
                    "region {id} entry summary has invalid register r{}",
                    slot.register
                ));
            }
        }
        let (kind, exits) = draft
            .completion
            .as_ref()
            .ok_or_else(|| format!("region {id} is unfinished"))?;
        if let RegionKind::Loop { backedge } = kind {
            validate_target(*backedge, code_len, *backedge)?;
        }
        for exit in exits {
            validate_target(exit.target, code_len, exit.target)?;
        }
    }
    Ok(())
}

fn validate_target(target: usize, code_len: usize, pc: usize) -> Result<(), String> {
    if target < code_len {
        Ok(())
    } else {
        Err(format!(
            "instruction at pc {pc} has out-of-range target {target}"
        ))
    }
}

fn build_regions(
    drafts: Vec<super::RegionDraft>,
    facts: RegionFacts<'_>,
) -> Result<(Vec<Region>, Vec<Option<super::RegionId>>), String> {
    let mut regions = Vec::with_capacity(drafts.len());
    let mut region_by_entry_pc = vec![None; facts.code_len];
    for (index, draft) in drafts.into_iter().enumerate() {
        let id = super::RegionId(index);
        let (kind, exits) = draft
            .completion
            .ok_or_else(|| format!("region {index} is unfinished"))?;
        let region_blocks: Vec<_> = match kind {
            RegionKind::Loop { backedge } => facts
                .blocks
                .iter()
                .filter(|block| block.start_pc <= backedge && block.end_pc > draft.entry)
                .map(|block| block.id)
                .collect(),
            RegionKind::Branch => facts
                .blocks
                .iter()
                .filter(|block| block.start_pc <= draft.entry && block.end_pc > draft.entry)
                .map(|block| block.id)
                .collect(),
        };
        let mut summary = RegionSummary {
            block_count: region_blocks.len(),
            ..RegionSummary::default()
        };
        for block_id in &region_blocks {
            let block = &facts.blocks[block_id.0 as usize];
            summary.instruction_count += block.end_pc - block.start_pc;
            summary.operation_count +=
                facts.operation_prefix[block.end_pc] - facts.operation_prefix[block.start_pc];
            summary.call_count +=
                facts.call_prefix[block.end_pc] - facts.call_prefix[block.start_pc];
        }
        region_by_entry_pc[draft.entry].get_or_insert(id);
        regions.push(Region {
            kind,
            entry: draft.entry,
            blocks: region_blocks,
            exits,
            entry_summary: draft.entry_summary,
            summary,
        });
    }
    Ok((regions, region_by_entry_pc))
}

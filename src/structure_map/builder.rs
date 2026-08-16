use crate::bytecode::Instruction;

use super::{
    BasicBlock, BlockEdge, BlockId, EdgeKind, Region, RegionKind, RegionSummary, StructureMap,
    StructureMapBuilder,
};

#[derive(Clone, Copy)]
enum Terminator {
    Next,
    Jump(usize),
    Branch { yes: usize, no: usize },
    Return,
}

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

    let mut leaders = vec![false; code.len()];
    if !code.is_empty() {
        leaders[0] = true;
    }
    for draft in &builder.regions {
        leaders[draft.entry] = true;
    }

    let mut terminators = Vec::with_capacity(code.len());
    let mut operation_prefix = Vec::with_capacity(code.len() + 1);
    let mut call_prefix = Vec::with_capacity(code.len() + 1);
    operation_prefix.push(0usize);
    call_prefix.push(0usize);
    for (pc, instruction) in code.iter().enumerate() {
        let (terminator, is_operation, is_call) = match instruction {
            Instruction::Jump { target } => {
                validate_target(*target, code.len(), pc)?;
                leaders[*target] = true;
                (Terminator::Jump(*target), false, false)
            }
            Instruction::Branch { yes, no, .. } => {
                validate_target(*yes, code.len(), pc)?;
                validate_target(*no, code.len(), pc)?;
                leaders[*yes] = true;
                leaders[*no] = true;
                (Terminator::Branch { yes: *yes, no: *no }, false, false)
            }
            Instruction::Return { .. } => (Terminator::Return, false, false),
            Instruction::BinaryOp { .. } | Instruction::CompareOp { .. } => {
                (Terminator::Next, true, false)
            }
            Instruction::Call { .. } => (Terminator::Next, false, true),
            Instruction::ConstSmallInt { .. }
            | Instruction::ConstFloat { .. }
            | Instruction::ConstBool { .. }
            | Instruction::LoadConstant { .. }
            | Instruction::ConstI64 { .. }
            | Instruction::UnaryOp { .. }
            | Instruction::BooleanOp { .. }
            | Instruction::BuildTuple { .. }
            | Instruction::BuildList { .. }
            | Instruction::BuildDict { .. }
            | Instruction::GetItem { .. }
            | Instruction::SetItem { .. }
            | Instruction::Length { .. }
            | Instruction::LoadCurrentFunction { .. }
            | Instruction::AddI64 { .. }
            | Instruction::LtI64 { .. }
            | Instruction::Move { .. } => (Terminator::Next, false, false),
        };
        terminators.push(terminator);
        operation_prefix.push(operation_prefix[pc] + usize::from(is_operation));
        call_prefix.push(call_prefix[pc] + usize::from(is_call));
        if matches!(
            terminator,
            Terminator::Jump(_) | Terminator::Branch { .. } | Terminator::Return
        ) && pc + 1 < code.len()
        {
            leaders[pc + 1] = true;
        }
    }

    let (mut blocks, block_by_pc) = build_blocks(&leaders, code.len())?;
    populate_edges(&mut blocks, &block_by_pc, &terminators);
    populate_predecessors(&mut blocks);
    let (regions, region_by_entry_pc) = build_regions(
        builder.regions,
        RegionFacts {
            blocks: &blocks,
            operation_prefix: &operation_prefix,
            call_prefix: &call_prefix,
            code_len: code.len(),
        },
    )?;

    Ok(StructureMap {
        blocks,
        regions,
        operation_sites: builder.operation_sites,
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

fn build_blocks(
    leaders: &[bool],
    code_len: usize,
) -> Result<(Vec<BasicBlock>, Vec<BlockId>), String> {
    let starts: Vec<_> = leaders
        .iter()
        .enumerate()
        .filter_map(|(pc, is_leader)| is_leader.then_some(pc))
        .collect();
    let mut blocks = Vec::with_capacity(starts.len());
    let mut block_by_pc = vec![BlockId(0); code_len];
    for (index, start_pc) in starts.iter().copied().enumerate() {
        let id = BlockId(
            u32::try_from(index)
                .map_err(|_| "StructureMap contains too many blocks".to_string())?,
        );
        let end_pc = starts.get(index + 1).copied().unwrap_or(code_len);
        block_by_pc[start_pc..end_pc].fill(id);
        blocks.push(BasicBlock {
            id,
            start_pc,
            end_pc,
            successors: Vec::new(),
            predecessors: Vec::new(),
        });
    }
    Ok((blocks, block_by_pc))
}

fn populate_edges(blocks: &mut [BasicBlock], block_by_pc: &[BlockId], terms: &[Terminator]) {
    for block in blocks {
        let Some(last_pc) = block.end_pc.checked_sub(1) else {
            continue;
        };
        block.successors = match terms[last_pc] {
            Terminator::Jump(target) => vec![edge(block_by_pc[target], EdgeKind::Jump)],
            Terminator::Branch { yes, no } => vec![
                edge(block_by_pc[yes], EdgeKind::BranchTrue),
                edge(block_by_pc[no], EdgeKind::BranchFalse),
            ],
            Terminator::Return => Vec::new(),
            Terminator::Next if block.end_pc < terms.len() => {
                vec![edge(block_by_pc[block.end_pc], EdgeKind::Fallthrough)]
            }
            Terminator::Next => Vec::new(),
        };
    }
}

const fn edge(target: BlockId, kind: EdgeKind) -> BlockEdge {
    BlockEdge { target, kind }
}

fn populate_predecessors(blocks: &mut [BasicBlock]) {
    let successors: Vec<_> = blocks
        .iter()
        .map(|block| (block.id, block.successors.clone()))
        .collect();
    for (source, edges) in successors {
        for edge in edges {
            let predecessors = &mut blocks[edge.target.0 as usize].predecessors;
            if !predecessors.contains(&source) {
                predecessors.push(source);
            }
        }
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

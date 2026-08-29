use crate::bytecode::Instruction;

use super::super::{BasicBlock, BlockEdge, BlockId, EdgeKind};
use super::validate_target;

#[derive(Clone, Copy)]
enum Terminator {
    Next,
    Jump(usize),
    Branch { yes: usize, no: usize },
    Return,
}

pub(super) struct CfgFacts {
    pub blocks: Vec<BasicBlock>,
    pub block_by_pc: Vec<BlockId>,
    pub operation_prefix: Vec<usize>,
    pub call_prefix: Vec<usize>,
}

pub(super) fn build(code: &[Instruction], region_entries: &[usize]) -> Result<CfgFacts, String> {
    let mut leaders = vec![false; code.len()];
    if !code.is_empty() {
        leaders[0] = true;
    }
    for entry in region_entries {
        leaders[*entry] = true;
    }

    let mut terminators = Vec::with_capacity(code.len());
    let mut operation_prefix = vec![0usize];
    let mut call_prefix = vec![0usize];
    for (pc, instruction) in code.iter().enumerate() {
        let (terminator, is_operation, is_call) = classify(instruction, code.len(), pc)?;
        mark_targets(terminator, &mut leaders);
        terminators.push(terminator);
        operation_prefix.push(operation_prefix[pc] + usize::from(is_operation));
        call_prefix.push(call_prefix[pc] + usize::from(is_call));
        if is_terminator(terminator) && pc + 1 < code.len() {
            leaders[pc + 1] = true;
        }
    }

    let (mut blocks, block_by_pc) = build_blocks(&leaders, code.len())?;
    populate_edges(&mut blocks, &block_by_pc, &terminators);
    populate_predecessors(&mut blocks);
    Ok(CfgFacts {
        blocks,
        block_by_pc,
        operation_prefix,
        call_prefix,
    })
}

fn classify(
    instruction: &Instruction,
    code_len: usize,
    pc: usize,
) -> Result<(Terminator, bool, bool), String> {
    match instruction {
        Instruction::Jump { target } => {
            validate_target(*target, code_len, pc)?;
            Ok((Terminator::Jump(*target), false, false))
        }
        Instruction::Branch { yes, no, .. } => {
            validate_target(*yes, code_len, pc)?;
            validate_target(*no, code_len, pc)?;
            Ok((Terminator::Branch { yes: *yes, no: *no }, false, false))
        }
        Instruction::Return { .. } => Ok((Terminator::Return, false, false)),
        Instruction::BinaryOp { .. } | Instruction::CompareOp { .. } => {
            Ok((Terminator::Next, true, false))
        }
        Instruction::Call { .. } => Ok((Terminator::Next, false, true)),
        _ => Ok((Terminator::Next, false, false)),
    }
}

fn mark_targets(terminator: Terminator, leaders: &mut [bool]) {
    match terminator {
        Terminator::Jump(target) => leaders[target] = true,
        Terminator::Branch { yes, no } => {
            leaders[yes] = true;
            leaders[no] = true;
        }
        Terminator::Next | Terminator::Return => {}
    }
}

const fn is_terminator(terminator: Terminator) -> bool {
    matches!(
        terminator,
        Terminator::Jump(_) | Terminator::Branch { .. } | Terminator::Return
    )
}

fn build_blocks(
    leaders: &[bool],
    code_len: usize,
) -> Result<(Vec<BasicBlock>, Vec<BlockId>), String> {
    let starts = leaders
        .iter()
        .enumerate()
        .filter_map(|(pc, is_leader)| is_leader.then_some(pc))
        .collect::<Vec<_>>();
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
    let successors = blocks
        .iter()
        .map(|block| (block.id, block.successors.clone()))
        .collect::<Vec<_>>();
    for (source, edges) in successors {
        for edge in edges {
            let predecessors = &mut blocks[edge.target.0 as usize].predecessors;
            if !predecessors.contains(&source) {
                predecessors.push(source);
            }
        }
    }
}

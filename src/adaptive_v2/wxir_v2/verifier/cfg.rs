use std::collections::{BTreeMap, BTreeSet};

use super::super::SnapshotError;
use super::super::ir::{Block, BlockId, SnapshotBody, Terminator};

pub(crate) fn predecessors(
    body: &SnapshotBody,
    blocks: &BTreeMap<BlockId, &Block>,
) -> Result<BTreeMap<BlockId, BTreeSet<BlockId>>, SnapshotError> {
    let mut predecessors = body
        .blocks
        .iter()
        .map(|block| (block.id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for block in &body.blocks {
        for target in targets(&block.terminator) {
            if !blocks.contains_key(&target) {
                return Err(SnapshotError::InvalidCfg {
                    block: block.id.get(),
                });
            }
            if let Some(entries) = predecessors.get_mut(&target) {
                entries.insert(block.id);
            }
        }
    }
    Ok(predecessors)
}

pub(crate) fn dominators(
    entry: BlockId,
    blocks: &BTreeMap<BlockId, &Block>,
    predecessors: &BTreeMap<BlockId, BTreeSet<BlockId>>,
) -> BTreeMap<BlockId, BTreeSet<BlockId>> {
    let all = blocks.keys().copied().collect::<BTreeSet<_>>();
    let mut result = blocks
        .keys()
        .map(|id| {
            (
                *id,
                if *id == entry {
                    BTreeSet::from([entry])
                } else {
                    all.clone()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    loop {
        let previous = result.clone();
        for id in blocks.keys().copied().filter(|id| *id != entry) {
            let preds = &predecessors[&id];
            let mut set = preds
                .first()
                .map_or_else(BTreeSet::new, |first| previous[first].clone());
            for predecessor in preds.iter().skip(1) {
                set = set.intersection(&previous[predecessor]).copied().collect();
            }
            set.insert(id);
            result.insert(id, set);
        }
        if result == previous {
            return result;
        }
    }
}

fn targets(terminator: &Terminator) -> Vec<BlockId> {
    match terminator {
        Terminator::Jump { target, .. } => vec![*target],
        Terminator::Branch { yes, no, .. } => vec![*yes, *no],
        Terminator::Return { .. }
        | Terminator::SideExit { .. }
        | Terminator::Backedge { .. }
        | Terminator::IrreducibleBackedge => Vec::new(),
    }
}

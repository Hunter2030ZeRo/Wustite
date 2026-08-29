use std::collections::{HashMap, HashSet};

use super::{WxFunction, WxInst, WxInstKind, WxRuntimeInput, WxTerminator, WxValueId};

mod checked;
mod dce;
mod keys;

use keys::{ExpressionKey, folded_instruction};

#[cfg(test)]
mod tests;

pub(crate) fn optimize(function: &mut WxFunction) {
    let mut replacements = HashMap::new();
    for block in &mut function.blocks {
        let mut expressions: HashMap<ExpressionKey, Vec<WxValueId>> = HashMap::new();
        let mut guards = HashSet::new();
        let mut optimized = Vec::with_capacity(block.instructions.len());
        for mut instruction in block.instructions.drain(..) {
            rewrite_instruction(&mut instruction, &replacements);
            if let Some(folded) = folded_instruction(&instruction, &optimized) {
                instruction = folded;
            }
            if let WxInstKind::Guard {
                condition, mode, ..
            } = instruction.kind
            {
                let key = (condition, guard_mode_code(mode));
                if !guards.insert(key) {
                    continue;
                }
            }
            if let Some(key) = ExpressionKey::new(&instruction) {
                let results = instruction
                    .results
                    .iter()
                    .map(|result| result.id)
                    .collect::<Vec<_>>();
                if let Some(existing) = expressions.get(&key) {
                    for (result, existing) in results.iter().zip(existing) {
                        replacements.insert(*result, *existing);
                    }
                    continue;
                }
                expressions.insert(key, results);
            } else if is_barrier(&instruction.kind) {
                expressions.clear();
            }
            optimized.push(instruction);
        }
        block.instructions = optimized;
    }
    rewrite_function(function, &replacements);
    dce::remove_dead_instructions(function);
    remove_unreferenced_side_exits(function);
}

fn is_barrier(kind: &WxInstKind) -> bool {
    matches!(
        kind,
        WxInstKind::Call { .. }
            | WxInstKind::RuntimeCall { .. }
            | WxInstKind::Load { .. }
            | WxInstKind::Store { .. }
    )
}

const fn guard_mode_code(mode: super::WxGuardMode) -> u8 {
    match mode {
        super::WxGuardMode::ExitWhenTrue => 0,
        super::WxGuardMode::ExitWhenFalse => 1,
    }
}

fn remove_unreferenced_side_exits(function: &mut WxFunction) {
    let mut referenced = HashSet::new();
    for block in &function.blocks {
        for instruction in &block.instructions {
            if let WxInstKind::Guard { exit, .. } = instruction.kind {
                referenced.insert(exit.0);
            }
        }
        if let WxTerminator::SideExit { exit, .. } = block.terminator {
            referenced.insert(exit.0);
        }
    }
    function
        .side_exits
        .retain(|side_exit| referenced.contains(&side_exit.id.0));
}

fn canonical(replacements: &HashMap<WxValueId, WxValueId>, mut value: WxValueId) -> WxValueId {
    while let Some(replacement) = replacements.get(&value).copied() {
        value = replacement;
    }
    value
}

fn rewrite_function(function: &mut WxFunction, replacements: &HashMap<WxValueId, WxValueId>) {
    for state in &mut function.entry_state {
        state.value = canonical(replacements, state.value);
    }
    for side_exit in &mut function.side_exits {
        for state in &mut side_exit.state {
            state.value = canonical(replacements, state.value);
        }
    }
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            rewrite_instruction(instruction, replacements);
        }
        rewrite_terminator(&mut block.terminator, replacements);
    }
}

fn rewrite_instruction(instruction: &mut WxInst, replacements: &HashMap<WxValueId, WxValueId>) {
    let replace = |value: &mut WxValueId| *value = canonical(replacements, *value);
    match &mut instruction.kind {
        WxInstKind::Constant(_) => {}
        WxInstKind::Binary { lhs, rhs, .. }
        | WxInstKind::IntegerBinaryWithOverflow { lhs, rhs, .. }
        | WxInstKind::Compare { lhs, rhs, .. } => {
            replace(lhs);
            replace(rhs);
        }
        WxInstKind::Cast { value, .. }
        | WxInstKind::Splat { value }
        | WxInstKind::Load { address: value } => replace(value),
        WxInstKind::Store { address, value } => {
            replace(address);
            replace(value);
        }
        WxInstKind::PointerOffset { base, offset } => {
            replace(base);
            replace(offset);
        }
        WxInstKind::ExtractLane { vector, .. } => replace(vector),
        WxInstKind::InsertLane { vector, value, .. } => {
            replace(vector);
            replace(value);
        }
        WxInstKind::Shuffle { left, right, .. } => {
            replace(left);
            replace(right);
        }
        WxInstKind::Guard { condition, .. } => replace(condition),
        WxInstKind::GuardSequence { value, .. } | WxInstKind::MaterializeSequence { value, .. } => {
            replace(value)
        }
        WxInstKind::SequenceLength { inputs, .. }
        | WxInstKind::SequenceGet { inputs, .. }
        | WxInstKind::SequenceSet { inputs, .. }
        | WxInstKind::SequenceMutate { inputs, .. } => inputs
            .iter_mut()
            .for_each(|WxRuntimeInput { value, .. }| replace(value)),
        WxInstKind::Call { arguments, .. } => arguments.iter_mut().for_each(replace),
        WxInstKind::RuntimeCall { inputs, .. } => inputs
            .iter_mut()
            .for_each(|WxRuntimeInput { value, .. }| replace(value)),
    }
}

fn rewrite_terminator(terminator: &mut WxTerminator, replacements: &HashMap<WxValueId, WxValueId>) {
    let replace = |value: &mut WxValueId| *value = canonical(replacements, *value);
    match terminator {
        WxTerminator::Jump { arguments, .. }
        | WxTerminator::Return { values: arguments }
        | WxTerminator::SideExit {
            values: arguments, ..
        } => arguments.iter_mut().for_each(replace),
        WxTerminator::Branch { condition, yes, no } => {
            replace(condition);
            yes.arguments.iter_mut().for_each(replace);
            no.arguments.iter_mut().for_each(replace);
        }
    }
}

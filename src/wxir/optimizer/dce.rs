use std::collections::HashSet;

use crate::wxir::{WxFunction, WxInst, WxInstKind, WxTerminator, WxValueId};

pub(super) fn remove_dead_instructions(function: &mut WxFunction) {
    let mut live = function
        .entry_state
        .iter()
        .map(|state| state.value)
        .chain(
            function
                .side_exits
                .iter()
                .flat_map(|exit| exit.state.iter().map(|state| state.value)),
        )
        .collect::<HashSet<_>>();
    for block in &function.blocks {
        add_terminator_inputs(&block.terminator, &mut live);
    }
    for block in &mut function.blocks {
        let mut retained = Vec::with_capacity(block.instructions.len());
        for instruction in block.instructions.drain(..).rev() {
            let required = is_effectful(&instruction.kind)
                || instruction
                    .results
                    .iter()
                    .any(|result| live.contains(&result.id));
            if required {
                add_instruction_inputs(&instruction, &mut live);
                retained.push(instruction);
            }
        }
        retained.reverse();
        block.instructions = retained;
    }
}

fn is_effectful(kind: &WxInstKind) -> bool {
    matches!(
        kind,
        WxInstKind::Guard { .. }
            | WxInstKind::GuardSequence { .. }
            | WxInstKind::SequenceLength { .. }
            | WxInstKind::SequenceGet { .. }
            | WxInstKind::SequenceSet { .. }
            | WxInstKind::SequenceMutate { .. }
            | WxInstKind::MaterializeSequence { .. }
            | WxInstKind::Call { .. }
            | WxInstKind::RuntimeCall { .. }
            | WxInstKind::Load { .. }
            | WxInstKind::Store { .. }
    )
}

fn add_instruction_inputs(instruction: &WxInst, live: &mut HashSet<WxValueId>) {
    match &instruction.kind {
        WxInstKind::Constant(_) => {}
        WxInstKind::Binary { lhs, rhs, .. }
        | WxInstKind::IntegerBinaryWithOverflow { lhs, rhs, .. }
        | WxInstKind::Compare { lhs, rhs, .. } => {
            live.extend([*lhs, *rhs]);
        }
        WxInstKind::Cast { value, .. }
        | WxInstKind::Load { address: value }
        | WxInstKind::Splat { value } => {
            live.insert(*value);
        }
        WxInstKind::Store { address, value } => {
            live.extend([*address, *value]);
        }
        WxInstKind::PointerOffset { base, offset } => {
            live.extend([*base, *offset]);
        }
        WxInstKind::ExtractLane { vector, .. } => {
            live.insert(*vector);
        }
        WxInstKind::InsertLane { vector, value, .. } => {
            live.extend([*vector, *value]);
        }
        WxInstKind::Shuffle { left, right, .. } => {
            live.extend([*left, *right]);
        }
        WxInstKind::Guard { condition, .. } => {
            live.insert(*condition);
        }
        WxInstKind::GuardSequence { value, .. } | WxInstKind::MaterializeSequence { value, .. } => {
            live.insert(*value);
        }
        WxInstKind::SequenceLength { inputs, .. }
        | WxInstKind::SequenceGet { inputs, .. }
        | WxInstKind::SequenceSet { inputs, .. }
        | WxInstKind::SequenceMutate { inputs, .. } => {
            live.extend(inputs.iter().map(|input| input.value));
        }
        WxInstKind::Call { arguments, .. } => live.extend(arguments),
        WxInstKind::RuntimeCall { inputs, .. } => {
            live.extend(inputs.iter().map(|input| input.value));
        }
    }
}

fn add_terminator_inputs(terminator: &WxTerminator, live: &mut HashSet<WxValueId>) {
    match terminator {
        WxTerminator::Jump { arguments, .. }
        | WxTerminator::Return { values: arguments }
        | WxTerminator::SideExit {
            values: arguments, ..
        } => live.extend(arguments),
        WxTerminator::Branch { condition, yes, no } => {
            live.insert(*condition);
            live.extend(&yes.arguments);
            live.extend(&no.arguments);
        }
    }
}

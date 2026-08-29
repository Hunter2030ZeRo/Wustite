use std::collections::{HashMap, HashSet};

use crate::bytecode::{Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::planner::JitPlan;

pub(super) fn analyze(
    executable: &ExecutableFunction,
    plan: &JitPlan,
) -> HashMap<usize, HashSet<Register>> {
    let mut live = (plan.header..=plan.backedge)
        .map(|pc| (pc, HashSet::new()))
        .collect::<HashMap<_, _>>();
    let boundary = plan
        .live_slots
        .iter()
        .map(|slot| slot.register)
        .collect::<HashSet<_>>();

    loop {
        let mut changed = false;
        for pc in (plan.header..=plan.backedge).rev() {
            let instruction = &executable.bytecode().code[pc];
            let mut next = HashSet::new();
            for successor in successors(instruction, pc) {
                if let Some(successor_live) = live.get(&successor) {
                    next.extend(successor_live.iter().copied());
                } else {
                    next.extend(boundary.iter().copied());
                }
            }

            if let Some(fact) = executable.structure_map().instruction_fact(pc) {
                if let Some(output) = fact.output.and_then(|value| {
                    executable
                        .structure_map()
                        .value(value)
                        .map(|fact| fact.register)
                }) {
                    next.remove(&output);
                }
                next.extend(fact.inputs.iter().map(|input| input.register));
            }

            if live.get(&pc) != Some(&next) {
                live.insert(pc, next);
                changed = true;
            }
        }
        if !changed {
            return live;
        }
    }
}

fn successors(instruction: &Instruction, pc: usize) -> Vec<usize> {
    match instruction {
        Instruction::Jump { target } => vec![*target],
        Instruction::Branch { yes, no, .. } => vec![*yes, *no],
        Instruction::Return { .. } => Vec::new(),
        _ => vec![pc + 1],
    }
}

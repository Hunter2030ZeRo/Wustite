use crate::bytecode::Instruction;

use super::super::super::{
    BasicBlock, ConstantSeed, Fact, InstructionFact, OperationSite, ValueFact, ValueId, ValueUse,
};
use super::{effects, inputs, output};

type DefinitionState = Vec<Vec<ValueId>>;

pub(super) fn classify_and_refresh(
    values: &mut [ValueFact],
    instructions: &mut [InstructionFact],
    code: &[Instruction],
    blocks: &[BasicBlock],
    register_count: usize,
    operation_sites: &[OperationSite],
    constants: &[ConstantSeed],
) {
    let entry = entry_state(values, register_count);
    let block_inputs = solve(blocks, instructions, values, entry);
    rewrite_inputs(
        instructions,
        code,
        blocks,
        values,
        block_inputs,
        register_count,
    );
    refresh(values, instructions, code, operation_sites, constants);
}

fn entry_state(values: &[ValueFact], register_count: usize) -> DefinitionState {
    let mut state = vec![Vec::new(); register_count];
    for value in values.iter().filter(|value| value.defined_at.is_none()) {
        state[usize::from(value.register)] = vec![value.id];
    }
    state
}

fn solve(
    blocks: &[BasicBlock],
    instructions: &[InstructionFact],
    values: &[ValueFact],
    entry: DefinitionState,
) -> Vec<DefinitionState> {
    let empty = vec![Vec::new(); entry.len()];
    let mut inputs = vec![empty.clone(); blocks.len()];
    let mut outputs = vec![empty; blocks.len()];
    loop {
        let mut changed = false;
        for block in blocks {
            let mut incoming = if block.id.0 == 0 {
                entry.clone()
            } else {
                vec![Vec::new(); entry.len()]
            };
            for predecessor in &block.predecessors {
                merge(&mut incoming, &outputs[predecessor.0 as usize]);
            }
            let outgoing = transfer(incoming.clone(), block, instructions, values);
            let index = block.id.0 as usize;
            if inputs[index] != incoming || outputs[index] != outgoing {
                inputs[index] = incoming;
                outputs[index] = outgoing;
                changed = true;
            }
        }
        if !changed {
            return inputs;
        }
    }
}

fn merge(target: &mut DefinitionState, source: &DefinitionState) {
    for (target, source) in target.iter_mut().zip(source) {
        for id in source {
            if !target.contains(id) {
                target.push(*id);
            }
        }
    }
}

fn transfer(
    mut state: DefinitionState,
    block: &BasicBlock,
    instructions: &[InstructionFact],
    values: &[ValueFact],
) -> DefinitionState {
    for instruction in &instructions[block.start_pc..block.end_pc] {
        apply_output(&mut state, instruction.output, values);
    }
    state
}

fn rewrite_inputs(
    instructions: &mut [InstructionFact],
    code: &[Instruction],
    blocks: &[BasicBlock],
    values: &[ValueFact],
    block_inputs: Vec<DefinitionState>,
    register_count: usize,
) {
    for block in blocks {
        let mut state = block_inputs
            .get(block.id.0 as usize)
            .cloned()
            .unwrap_or_else(|| vec![Vec::new(); register_count]);
        for pc in block.start_pc..block.end_pc {
            instructions[pc].inputs = inputs::input_registers(&code[pc])
                .into_iter()
                .map(|register| ValueUse {
                    register,
                    value: unique(&state[usize::from(register)]),
                })
                .collect();
            apply_output(&mut state, instructions[pc].output, values);
        }
    }
}

fn apply_output(state: &mut DefinitionState, output: Option<ValueId>, values: &[ValueFact]) {
    let Some(value) = output.and_then(|id| values.get(id.0 as usize)) else {
        return;
    };
    state[usize::from(value.register)] = vec![value.id];
}

fn unique(definitions: &[ValueId]) -> Option<ValueId> {
    (definitions.len() == 1).then(|| definitions[0])
}

fn refresh(
    values: &mut [ValueFact],
    instructions: &mut [InstructionFact],
    code: &[Instruction],
    operation_sites: &[OperationSite],
    constants: &[ConstantSeed],
) {
    for (pc, instruction) in code.iter().enumerate() {
        let fact = &mut instructions[pc];
        let (instruction_effects, failures) =
            effects::effects_and_failures(instruction, &fact.inputs, values, operation_sites);
        fact.effects = Fact::Proven(instruction_effects);
        fact.mutated_values = Fact::Proven(inputs::mutated_values(instruction, &fact.inputs));
        fact.failures = failures;
        let Some(id) = fact.output else {
            continue;
        };
        let Some(derived) = output::output(
            pc,
            instruction,
            &fact.inputs,
            values,
            operation_sites,
            constants,
        ) else {
            continue;
        };
        let value = &mut values[id.0 as usize];
        value.origin = derived.origin;
        value.ty = derived.ty;
        value.identity = derived.identity;
        value.composition = derived.composition;
        value.escape = derived.escape;
        value.sequence = derived.sequence;
    }
}

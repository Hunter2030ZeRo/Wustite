use crate::bytecode::{Instruction, Register};

use super::super::{
    BasicBlock, Fact, InstructionFact, Region, StructureMapBuilder, TypeFact, ValueComposition,
    ValueFact, ValueId, ValueOrigin, ValueUse,
};

mod control;
mod effects;
mod escape;
mod inputs;
mod mutation;
mod output;
mod reaching;

pub(super) struct Analysis {
    pub values: Vec<ValueFact>,
    pub instructions: Vec<InstructionFact>,
}

pub(super) fn analyze(
    builder: &StructureMapBuilder,
    code: &[Instruction],
    register_count: usize,
    blocks: &[BasicBlock],
    block_by_pc: &[super::super::BlockId],
    regions: &mut [Region],
) -> Result<Analysis, String> {
    let mut values = Vec::new();
    let mut registers = vec![None; register_count];
    let mut parameters = builder.parameters.iter().collect::<Vec<_>>();
    parameters.sort_by_key(|parameter| parameter.index);
    for parameter in parameters {
        let register_index = usize::from(parameter.register);
        if register_index >= register_count {
            return Err(format!(
                "parameter {} has invalid register r{}",
                parameter.index, parameter.register
            ));
        }
        let id = next_value_id(&values)?;
        values.push(ValueFact {
            id,
            register: parameter.register,
            defined_at: None,
            origin: Fact::Proven(ValueOrigin::Parameter {
                index: parameter.index,
                name: parameter.name.clone(),
            }),
            ty: TypeFact::Proven(parameter.ty),
            identity: Fact::Unknown,
            composition: Fact::Proven(ValueComposition::None),
            escape: Fact::Proven(super::super::EscapeState::Local),
            sequence: super::super::SequenceFacts::unknown(),
        });
        registers[register_index] = Some(id);
    }

    let mut instructions = Vec::with_capacity(code.len());
    for (pc, instruction) in code.iter().enumerate() {
        let inputs = inputs::input_registers(instruction)
            .into_iter()
            .map(|register| ValueUse {
                register,
                value: registers.get(usize::from(register)).copied().flatten(),
            })
            .collect::<Vec<_>>();
        let (effects, failures) =
            effects::effects_and_failures(instruction, &inputs, &values, &builder.operation_sites);
        let mutated_values = inputs::mutated_values(instruction, &inputs);
        let output = output::output(
            pc,
            instruction,
            &inputs,
            &values,
            &builder.operation_sites,
            &builder.constants,
        )
        .map(|output| {
            let id = next_value_id(&values)?;
            let register_index = usize::from(output.register);
            if register_index >= register_count {
                return Err(format!(
                    "instruction at pc {pc} writes invalid register r{}",
                    output.register
                ));
            }
            values.push(ValueFact {
                id,
                register: output.register,
                defined_at: Some(pc),
                origin: output.origin,
                ty: output.ty,
                identity: output.identity,
                composition: output.composition,
                escape: output.escape,
                sequence: output.sequence,
            });
            registers[register_index] = Some(id);
            Ok(id)
        })
        .transpose()?;
        instructions.push(InstructionFact {
            pc,
            inputs,
            output,
            effects: Fact::Proven(effects),
            mutated_values: Fact::Proven(mutated_values),
            failures,
            mutations: Fact::Proven(Vec::new()),
            guard_placement: Fact::Unknown,
            control_dependencies: Vec::new(),
        });
    }

    reaching::classify_and_refresh(
        &mut values,
        &mut instructions,
        code,
        blocks,
        register_count,
        &builder.operation_sites,
        &builder.constants,
    );
    mutation::classify(&values, &mut instructions, code, block_by_pc);
    escape::classify(&mut values, &instructions, code, blocks, regions);
    control::classify(&values, &mut instructions, code, blocks, block_by_pc);
    summarize_regions(regions, blocks, &values, &instructions);
    Ok(Analysis {
        values,
        instructions,
    })
}

fn summarize_regions(
    regions: &mut [Region],
    blocks: &[BasicBlock],
    values: &[ValueFact],
    instructions: &[InstructionFact],
) {
    for region in regions {
        let contains_pc = |pc| {
            region.blocks.iter().any(|id| {
                blocks
                    .get(id.0 as usize)
                    .is_some_and(|block| block.start_pc <= pc && pc < block.end_pc)
            })
        };
        let mut effects = super::super::EffectSummary::default();
        let mut effects_fact = Fact::Proven(effects);
        let mut failure_site_count = 0;
        for instruction in instructions.iter().filter(|fact| contains_pc(fact.pc)) {
            match instruction.effects {
                Fact::Proven(instruction_effects) => effects.include(instruction_effects),
                Fact::Guardable(instruction_effects) => {
                    effects.include(instruction_effects);
                    effects_fact = Fact::Guardable(effects);
                }
                Fact::Unknown => effects_fact = Fact::Unknown,
            }
            if !matches!(instruction.failures, Fact::Proven(ref failures) if failures.is_empty()) {
                failure_site_count += 1;
            }
        }
        effects_fact = match effects_fact {
            Fact::Proven(_) => Fact::Proven(effects),
            Fact::Guardable(_) => Fact::Guardable(effects),
            Fact::Unknown => Fact::Unknown,
        };
        let allocations = values.iter().filter(|value| {
            value.defined_at.is_some_and(contains_pc)
                && matches!(value.origin, Fact::Proven(ValueOrigin::Allocation { .. }))
        });
        let mut escaping_allocation_count = 0;
        let mut virtualizable_allocation_count = 0;
        for allocation in allocations {
            if allocation.is_virtualizable() {
                virtualizable_allocation_count += 1;
            } else {
                escaping_allocation_count += 1;
            }
        }
        region.summary.effects = effects_fact;
        region.summary.escaping_allocation_count = escaping_allocation_count;
        region.summary.virtualizable_allocation_count = virtualizable_allocation_count;
        region.summary.failure_site_count = failure_site_count;
        region.summary.guardable_fact_count = values
            .iter()
            .filter(|value| {
                value.defined_at.is_some_and(contains_pc)
                    && matches!(value.ty, TypeFact::Guardable(_))
            })
            .count();
    }
}

fn next_value_id(values: &[ValueFact]) -> Result<ValueId, String> {
    u32::try_from(values.len())
        .map(ValueId)
        .map_err(|_| "StructureMap contains too many values".to_string())
}

fn value<'a>(values: &'a [ValueFact], value_use: &ValueUse) -> Option<&'a ValueFact> {
    values.get(value_use.value?.0 as usize)
}

fn type_of(values: &[ValueFact], value_use: &ValueUse) -> TypeFact {
    value(values, value_use).map_or(TypeFact::Unknown, |value| value.ty)
}

fn input_for(inputs: &[ValueUse], register: Register) -> Option<ValueUse> {
    inputs
        .iter()
        .copied()
        .find(|input| input.register == register)
}

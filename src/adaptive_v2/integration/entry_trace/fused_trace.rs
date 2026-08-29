use crate::adaptive_v2::profile::RecordPermit;
use crate::adaptive_v2::wxir_v2::ir::SnapshotDraft;
use crate::bytecode::Register;
use crate::executable::ExecutableFunction;
use crate::value::Value;

mod lower;
mod macro_loop;
mod scalar_cfg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::adaptive_v2::integration) enum FusedAccessFact {
    ListI64 { layout_epoch: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::adaptive_v2::integration) struct FusedTraceFacts {
    schema_epoch: u64,
    accesses: std::collections::BTreeMap<usize, FusedAccessFact>,
}

impl FusedTraceFacts {
    pub(super) fn from_proven_structure(
        executable: &ExecutableFunction,
        schema_epoch: u64,
    ) -> Self {
        let mut facts = Self {
            schema_epoch,
            accesses: std::collections::BTreeMap::new(),
        };
        for (pc, instruction) in executable.bytecode().code.iter().enumerate() {
            if !matches!(instruction, crate::bytecode::Instruction::GetItem { .. }) {
                continue;
            }
            let Some(instruction_fact) = executable.structure_map().instruction_fact(pc) else {
                continue;
            };
            let Some(receiver) = instruction_fact
                .inputs
                .first()
                .and_then(|input| input.value)
                .and_then(|value| executable.structure_map().value(value))
            else {
                continue;
            };
            let Some(output) = instruction_fact
                .output
                .and_then(|value| executable.structure_map().value(value))
            else {
                continue;
            };
            facts.include_sequence_access(
                pc,
                receiver.sequence.strategy,
                receiver.sequence.layout_stable,
                output.ty,
                executable.id().as_u64(),
            );
        }
        facts
    }

    #[cfg(test)]
    pub(super) const fn new(schema_epoch: u64) -> Self {
        Self {
            schema_epoch,
            accesses: std::collections::BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(in crate::adaptive_v2::integration) fn with_access(
        mut self,
        pc: usize,
        fact: FusedAccessFact,
    ) -> Self {
        self.accesses.insert(pc, fact);
        self
    }

    fn access(&self, pc: usize) -> Option<FusedAccessFact> {
        self.accesses.get(&pc).copied()
    }

    fn include_sequence_access(
        &mut self,
        pc: usize,
        strategy: crate::structure_map::Fact<crate::object::SequenceStrategy>,
        layout_stable: crate::structure_map::Fact<bool>,
        output: crate::structure_map::Fact<crate::structure_map::SlotType>,
        layout_epoch: u64,
    ) {
        use crate::object::SequenceStrategy;
        use crate::structure_map::{Fact, SlotType};

        if strategy == Fact::Proven(SequenceStrategy::I64)
            && layout_stable == Fact::Proven(true)
            && output == Fact::Proven(SlotType::SmallInt)
        {
            self.accesses
                .insert(pc, FusedAccessFact::ListI64 { layout_epoch });
        }
    }
}

pub(in crate::adaptive_v2::integration) struct FusedTraceRequest<'a> {
    pub(in crate::adaptive_v2::integration) executable: &'a ExecutableFunction,
    pub(in crate::adaptive_v2::integration) arguments: &'a [Value],
    pub(in crate::adaptive_v2::integration) permit: RecordPermit,
    pub(in crate::adaptive_v2::integration) facts: &'a FusedTraceFacts,
}

pub(super) fn is_macro_candidate(executable: &ExecutableFunction, arguments: &[Value]) -> bool {
    arguments.is_empty()
        && executable.parameters().is_empty()
        && (macro_loop::recognizes(executable) || scalar_cfg::recognizes(executable))
}

pub(in crate::adaptive_v2::integration) fn record(
    request: FusedTraceRequest<'_>,
) -> Result<Option<SnapshotDraft>, String> {
    if request.permit.schema_epoch() != request.facts.schema_epoch {
        return Ok(None);
    }
    if let Some(draft) = macro_loop::lower(&request)? {
        return Ok(Some(draft));
    }
    if let Some(draft) = scalar_cfg::lower(&request)? {
        return Ok(Some(draft));
    }
    lower::lower(request)
}

#[derive(Clone, Copy)]
enum RegisterValue {
    Ssa(crate::adaptive_v2::wxir_v2::ir::ValueDef),
    Callee(usize),
}

fn register_value(
    values: &std::collections::BTreeMap<Register, RegisterValue>,
    register: Register,
) -> Result<RegisterValue, String> {
    values
        .get(&register)
        .copied()
        .ok_or_else(|| format!("fused trace reads undefined r{register}"))
}

#[cfg(test)]
mod tests;

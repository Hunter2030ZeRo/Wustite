use crate::executable::ExecutableFunction;
use crate::structure_map::{Fact, RegionId, SlotType, ValueOrigin};
use crate::value::Value;

use crate::adaptive_v2::profile::{FactClass, LiveObservation, ProfileCase};

#[derive(Clone, Copy)]
pub(super) struct ClassifiedObservation {
    pub(super) live: LiveObservation,
    pub(super) static_facts: u64,
}

#[derive(Clone, Copy)]
pub(super) struct StaticClassification {
    certainty: Certainty,
    pub(super) static_facts: u64,
}

impl StaticClassification {
    pub(super) fn observe_case(self, runtime_case: u32) -> ClassifiedObservation {
        classified_case(runtime_case, self.certainty, self.static_facts)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Certainty {
    Proven,
    Guardable,
    Unknown,
}

pub(super) fn entry(executable: &ExecutableFunction, arguments: &[Value]) -> ClassifiedObservation {
    let mut certainty = Certainty::Proven;
    let mut static_facts = 0_u64;
    for (index, argument) in arguments.iter().enumerate() {
        let fact = executable.structure_map().values().iter().find(|value| {
            matches!(
                value.origin.as_ref(),
                Fact::Proven(ValueOrigin::Parameter { index: candidate, .. })
                    | Fact::Guardable(ValueOrigin::Parameter { index: candidate, .. })
                    if *candidate == index
            )
        });
        let Some(fact) = fact else {
            certainty = Certainty::Unknown;
            continue;
        };
        let origin = certainty_of(&fact.origin);
        let ty = match fact.ty.as_ref() {
            Fact::Proven(expected) if matches_type(argument, expected) => Certainty::Proven,
            Fact::Guardable(expected) if matches_type(argument, expected) => Certainty::Guardable,
            Fact::Proven(_) | Fact::Guardable(_) | Fact::Unknown => Certainty::Unknown,
        };
        let observed = origin.max(ty);
        if observed != Certainty::Unknown {
            static_facts = static_facts.saturating_add(1);
        }
        certainty = certainty.max(observed);
    }
    classified(arguments, certainty, static_facts)
}

pub(super) fn loop_header(
    executable: &ExecutableFunction,
    region_id: RegionId,
    inputs: &[Value],
    storage_cases: &[u32],
) -> ClassifiedObservation {
    let Some(region) = executable.structure_map().region(region_id) else {
        return classified_with_storage(inputs, storage_cases, Certainty::Unknown, 0);
    };
    let mut certainty = certainty_of(&region.summary.effects);
    let mut static_facts = u64::from(!matches!(region.summary.effects, Fact::Unknown));
    for (slot, input) in region.entry_summary.iter().zip(inputs) {
        if matches_type(input, &slot.ty) {
            static_facts = static_facts.saturating_add(1);
        } else {
            certainty = Certainty::Unknown;
        }
    }
    classified_with_storage(inputs, storage_cases, certainty, static_facts)
}

pub(super) fn instruction_classification(
    executable: &ExecutableFunction,
    pc: usize,
) -> StaticClassification {
    let map = executable.structure_map();
    let Some(instruction) = map.instruction_fact(pc) else {
        return StaticClassification {
            certainty: Certainty::Unknown,
            static_facts: 0,
        };
    };
    let mut certainty = certainty_of(&instruction.effects);
    let mut static_facts = u64::from(!matches!(instruction.effects, Fact::Unknown));
    for input in &instruction.inputs {
        let Some(value) = input.value.and_then(|id| map.value(id)) else {
            certainty = Certainty::Unknown;
            continue;
        };
        let observed = certainty_of(&value.origin).max(certainty_of(&value.ty));
        if observed != Certainty::Unknown {
            static_facts = static_facts.saturating_add(2);
        }
        certainty = certainty.max(observed);
    }
    if certainty == Certainty::Proven {
        certainty = Certainty::Guardable;
    }
    StaticClassification {
        certainty,
        static_facts,
    }
}

fn classified(values: &[Value], certainty: Certainty, static_facts: u64) -> ClassifiedObservation {
    classified_case(runtime_case(values), certainty, static_facts)
}

fn classified_with_storage(
    values: &[Value],
    storage_cases: &[u32],
    certainty: Certainty,
    static_facts: u64,
) -> ClassifiedObservation {
    let runtime_case = storage_cases
        .iter()
        .fold(runtime_case(values), |hash, value| {
            (hash ^ value).wrapping_mul(0x0100_0193)
        });
    classified_case(runtime_case, certainty, static_facts)
}

fn classified_case(
    runtime_case: u32,
    certainty: Certainty,
    static_facts: u64,
) -> ClassifiedObservation {
    let fact = match certainty {
        Certainty::Proven => FactClass::Proven,
        Certainty::Guardable => FactClass::Guardable {
            guard_emitted: true,
            live_confirmed: true,
        },
        Certainty::Unknown => FactClass::UnknownClassified,
    };
    ClassifiedObservation {
        live: LiveObservation::new(ProfileCase::new(runtime_case), fact),
        static_facts,
    }
}

fn certainty_of<T>(fact: &Fact<T>) -> Certainty {
    match fact {
        Fact::Proven(_) => Certainty::Proven,
        Fact::Guardable(_) => Certainty::Guardable,
        Fact::Unknown => Certainty::Unknown,
    }
}

fn matches_type(value: &Value, expected: &SlotType) -> bool {
    matches!(
        (value, expected),
        (_, SlotType::Any)
            | (Value::SmallInt(_), SlotType::SmallInt)
            | (Value::Float(_), SlotType::Float)
            | (Value::Bool(_), SlotType::Bool)
    )
}

fn runtime_case(values: &[Value]) -> u32 {
    values.iter().fold(0x811c_9dc5_u32, |hash, value| {
        let tag = match value {
            Value::SmallInt(_) => 1,
            Value::Float(_) => 2,
            Value::Bool(_) => 3,
            Value::None => 4,
            Value::Object(_) => 5,
            Value::Uninitialized => 6,
        };
        (hash ^ tag).wrapping_mul(0x0100_0193)
    })
}

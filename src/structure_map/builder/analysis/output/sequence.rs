use crate::object::{ObjectKind, SequenceStrategy};
use crate::structure_map::{
    Fact, SequenceFacts, SequenceKind, SequenceMutability, SlotType, TypeFact, ValueFact, ValueUse,
};

use super::super::type_of;

pub(super) fn sequence_facts(
    kind: ObjectKind,
    inputs: &[ValueUse],
    values: &[ValueFact],
) -> SequenceFacts {
    let element_type = inputs.first().map_or(TypeFact::Unknown, |first| {
        let first = type_of(values, first);
        if inputs
            .iter()
            .skip(1)
            .all(|input| type_of(values, input) == first)
        {
            first
        } else {
            TypeFact::Unknown
        }
    });
    let strategy = if inputs.is_empty() {
        Fact::Proven(SequenceStrategy::Empty)
    } else {
        match element_type {
            Fact::Proven(ty) => Fact::Proven(strategy_for_type(ty)),
            Fact::Guardable(ty) => Fact::Guardable(strategy_for_type(ty)),
            Fact::Unknown => Fact::Proven(SequenceStrategy::Object),
        }
    };
    SequenceFacts {
        kind: Fact::Proven(if kind == ObjectKind::List {
            SequenceKind::List
        } else {
            SequenceKind::Tuple
        }),
        strategy,
        element_type,
        exact_length: Fact::Proven(inputs.len()),
        mutability: Fact::Proven(if kind == ObjectKind::List {
            SequenceMutability::Mutable
        } else {
            SequenceMutability::Immutable
        }),
        layout_stable: Fact::Proven(true),
    }
}

const fn strategy_for_type(ty: SlotType) -> SequenceStrategy {
    match ty {
        SlotType::Bool => SequenceStrategy::Bool,
        SlotType::SmallInt => SequenceStrategy::I64,
        SlotType::Float => SequenceStrategy::F64,
        SlotType::Object(_) | SlotType::Any => SequenceStrategy::Object,
    }
}

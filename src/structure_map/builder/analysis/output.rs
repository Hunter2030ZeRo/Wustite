use crate::bytecode::{Instruction, Register, UnaryOperator};
use crate::object::ObjectKind;

use super::super::super::{
    ConstantSeed, EscapeState, Fact, IdentityFact, OperationSite, SequenceFacts, SlotType,
    TypeFact, ValueComposition, ValueFact, ValueOrigin, ValueUse,
};
use super::{input_for, type_of};

mod sequence;

use sequence::sequence_facts;

pub(super) struct Output {
    pub register: Register,
    pub origin: Fact<ValueOrigin>,
    pub ty: TypeFact,
    pub identity: Fact<IdentityFact>,
    pub composition: Fact<ValueComposition>,
    pub escape: Fact<EscapeState>,
    pub sequence: SequenceFacts,
}

pub(super) fn output(
    pc: usize,
    instruction: &Instruction,
    inputs: &[ValueUse],
    values: &[ValueFact],
    operation_sites: &[OperationSite],
    constants: &[ConstantSeed],
) -> Option<Output> {
    let fresh = |register, origin, ty, composition| Output {
        register,
        origin,
        ty,
        identity: Fact::Proven(IdentityFact::Fresh),
        composition,
        escape: Fact::Proven(EscapeState::Local),
        sequence: SequenceFacts::unknown(),
    };
    match instruction {
        Instruction::ConstSmallInt { dst, .. } | Instruction::ConstI64 { dst, .. } => Some(fresh(
            *dst,
            Fact::Proven(ValueOrigin::Immediate { pc }),
            TypeFact::Proven(SlotType::SmallInt),
            Fact::Proven(ValueComposition::None),
        )),
        Instruction::ConstFloat { dst, .. } => Some(fresh(
            *dst,
            Fact::Proven(ValueOrigin::Immediate { pc }),
            TypeFact::Proven(SlotType::Float),
            Fact::Proven(ValueComposition::None),
        )),
        Instruction::ConstBool { dst, .. } => Some(fresh(
            *dst,
            Fact::Proven(ValueOrigin::Immediate { pc }),
            TypeFact::Proven(SlotType::Bool),
            Fact::Proven(ValueComposition::None),
        )),
        Instruction::ConstNone { dst } => Some(fresh(
            *dst,
            Fact::Proven(ValueOrigin::Immediate { pc }),
            TypeFact::Unknown,
            Fact::Proven(ValueComposition::None),
        )),
        Instruction::LoadConstant { dst, constant } => {
            let kind = constants
                .iter()
                .find(|seed| seed.index == constant.0)
                .map(|seed| seed.kind);
            let ty = kind.map_or(TypeFact::Unknown, |kind| {
                TypeFact::Proven(SlotType::Object(kind))
            });
            Some(Output {
                register: *dst,
                origin: Fact::Proven(ValueOrigin::ConstantPool {
                    pc,
                    index: constant.0,
                    kind,
                }),
                ty,
                identity: Fact::Unknown,
                composition: Fact::Unknown,
                escape: Fact::Proven(EscapeState::Local),
                sequence: SequenceFacts::unknown(),
            })
        }
        Instruction::BuildTuple { dst, .. } | Instruction::BuildList { dst, .. } => {
            let kind = if matches!(instruction, Instruction::BuildTuple { .. }) {
                ObjectKind::Tuple
            } else {
                ObjectKind::List
            };
            let mut output = fresh(
                *dst,
                Fact::Proven(ValueOrigin::Allocation { pc, kind }),
                TypeFact::Proven(SlotType::Object(kind)),
                Fact::Proven(ValueComposition::Sequence(inputs.to_vec())),
            );
            output.sequence = sequence_facts(kind, inputs, values);
            Some(output)
        }
        Instruction::BuildDict { dst, .. } => Some(fresh(
            *dst,
            Fact::Proven(ValueOrigin::Allocation {
                pc,
                kind: ObjectKind::Dict,
            }),
            TypeFact::Proven(SlotType::Object(ObjectKind::Dict)),
            Fact::Proven(ValueComposition::Mapping(
                inputs
                    .chunks_exact(2)
                    .map(|pair| (pair[0], pair[1]))
                    .collect(),
            )),
        )),
        Instruction::Move { dst, src } => {
            let source = use_for(inputs, *src);
            Some(Output {
                register: *dst,
                origin: Fact::Proven(ValueOrigin::Alias { pc, source }),
                ty: type_of(values, &source),
                identity: source
                    .value
                    .map_or(Fact::Unknown, |id| Fact::Proven(IdentityFact::AliasOf(id))),
                composition: Fact::Unknown,
                escape: Fact::Proven(EscapeState::Local),
                sequence: source
                    .value
                    .and_then(|id| values.get(id.0 as usize))
                    .map_or_else(SequenceFacts::unknown, |value| value.sequence),
            })
        }
        Instruction::GetItem { dst, object, .. }
        | Instruction::GetAttr { dst, object, .. }
        | Instruction::ListPop {
            dst, list: object, ..
        } => {
            let aggregate = use_for(inputs, *object);
            Some(Output {
                register: *dst,
                origin: Fact::Proven(ValueOrigin::Projection { pc, aggregate }),
                ty: TypeFact::Unknown,
                identity: Fact::Unknown,
                composition: Fact::Unknown,
                escape: Fact::Unknown,
                sequence: SequenceFacts::unknown(),
            })
        }
        Instruction::Call { dst, callable, .. } => Some(Output {
            register: *dst,
            origin: Fact::Proven(ValueOrigin::Call {
                pc,
                callable: use_for(inputs, *callable),
            }),
            ty: TypeFact::Unknown,
            identity: Fact::Unknown,
            composition: Fact::Unknown,
            escape: Fact::Unknown,
            sequence: SequenceFacts::unknown(),
        }),
        Instruction::CallMethod { dst, receiver, .. } => Some(Output {
            register: *dst,
            origin: Fact::Proven(ValueOrigin::Call {
                pc,
                callable: use_for(inputs, *receiver),
            }),
            ty: TypeFact::Unknown,
            identity: Fact::Unknown,
            composition: Fact::Unknown,
            escape: Fact::Unknown,
            sequence: SequenceFacts::unknown(),
        }),
        Instruction::LoadCurrentFunction { dst } => Some(Output {
            register: *dst,
            origin: Fact::Proven(ValueOrigin::CurrentFunction { pc }),
            ty: TypeFact::Proven(SlotType::Object(ObjectKind::Function)),
            identity: Fact::Unknown,
            composition: Fact::Unknown,
            escape: Fact::Proven(EscapeState::Local),
            sequence: SequenceFacts::unknown(),
        }),
        Instruction::BinaryOp { dst, site, .. } | Instruction::CompareOp { dst, site, .. } => {
            Some(fresh(
                *dst,
                Fact::Proven(ValueOrigin::Operation { pc }),
                operation_sites
                    .get(site.0 as usize)
                    .map_or(TypeFact::Unknown, |site| site.result),
                Fact::Proven(ValueComposition::None),
            ))
        }
        Instruction::UnaryOp { dst, op, src } => Some(fresh(
            *dst,
            Fact::Proven(ValueOrigin::Operation { pc }),
            match op {
                UnaryOperator::Negate => input_for(inputs, *src)
                    .map_or(TypeFact::Unknown, |input| type_of(values, &input)),
                UnaryOperator::Not => TypeFact::Proven(SlotType::Bool),
            },
            Fact::Proven(ValueComposition::None),
        )),
        Instruction::BooleanOp { dst, .. } | Instruction::LtI64 { dst, .. } => Some(fresh(
            *dst,
            Fact::Proven(ValueOrigin::Operation { pc }),
            TypeFact::Proven(SlotType::Bool),
            Fact::Proven(ValueComposition::None),
        )),
        Instruction::Length { dst, .. } | Instruction::AddI64 { dst, .. } => Some(fresh(
            *dst,
            Fact::Proven(ValueOrigin::Operation { pc }),
            TypeFact::Proven(SlotType::SmallInt),
            Fact::Proven(ValueComposition::None),
        )),
        Instruction::GetSlice { dst, .. } => Some(Output {
            register: *dst,
            origin: Fact::Unknown,
            ty: TypeFact::Unknown,
            identity: Fact::Proven(IdentityFact::Fresh),
            composition: Fact::Unknown,
            escape: Fact::Proven(EscapeState::Local),
            sequence: SequenceFacts::unknown(),
        }),
        Instruction::SetItem { .. }
        | Instruction::SetAttr { .. }
        | Instruction::SetSlice { .. }
        | Instruction::ListAppend { .. }
        | Instruction::ListInsert { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => None,
    }
}

fn use_for(inputs: &[ValueUse], register: Register) -> ValueUse {
    input_for(inputs, register).unwrap_or(ValueUse {
        register,
        value: None,
    })
}

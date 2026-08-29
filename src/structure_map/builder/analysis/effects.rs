use crate::bytecode::{BinaryOperator, Instruction};
use crate::object::ObjectKind;

use super::super::super::{
    EffectSummary, Fact, FailureKind, OperationSite, OperationSiteId, SlotType, TypeFact,
    ValueFact, ValueUse,
};
use super::type_of;

pub(super) fn effects_and_failures(
    instruction: &Instruction,
    inputs: &[ValueUse],
    values: &[ValueFact],
    operation_sites: &[OperationSite],
) -> (EffectSummary, Fact<Vec<FailureKind>>) {
    let mutation = matches!(
        instruction,
        Instruction::SetItem { .. }
            | Instruction::SetAttr { .. }
            | Instruction::SetSlice { .. }
            | Instruction::ListAppend { .. }
            | Instruction::ListInsert { .. }
            | Instruction::ListPop { .. }
    );
    let allocation = matches!(
        instruction,
        Instruction::LoadConstant { .. }
            | Instruction::BuildTuple { .. }
            | Instruction::BuildList { .. }
            | Instruction::BuildDict { .. }
            | Instruction::GetSlice { .. }
    );
    let unknown_call = matches!(
        instruction,
        Instruction::Call { .. } | Instruction::CallMethod { .. }
    );
    let effects = EffectSummary {
        may_mutate: mutation || unknown_call,
        may_allocate: allocation || unknown_call,
        may_call_unknown: unknown_call,
        may_access_global_state: unknown_call,
    };
    let failures = match instruction {
        Instruction::BinaryOp { op, site, .. } => binary_failures(*op, *site, operation_sites),
        Instruction::CompareOp { site, .. } => compare_failures(*site, operation_sites),
        Instruction::UnaryOp { .. } | Instruction::BooleanOp { .. } => {
            Fact::Proven(vec![FailureKind::Type])
        }
        Instruction::BuildTuple { .. }
        | Instruction::BuildList { .. }
        | Instruction::BuildDict { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::GetSlice { .. } => Fact::Proven(vec![FailureKind::Allocation]),
        Instruction::GetItem { .. } => Fact::Proven(vec![
            FailureKind::Type,
            FailureKind::Index,
            FailureKind::Key,
        ]),
        Instruction::GetAttr { .. } | Instruction::SetAttr { .. } => {
            Fact::Proven(vec![FailureKind::Type])
        }
        Instruction::SetItem { .. } => Fact::Proven(vec![
            FailureKind::Type,
            FailureKind::Index,
            FailureKind::Key,
        ]),
        Instruction::SetSlice { .. } => Fact::Proven(vec![
            FailureKind::Type,
            FailureKind::Index,
            FailureKind::Value,
        ]),
        Instruction::ListAppend { .. } | Instruction::ListInsert { .. } => {
            Fact::Proven(vec![FailureKind::Type, FailureKind::Allocation])
        }
        Instruction::ListPop { .. } => Fact::Proven(vec![FailureKind::Type, FailureKind::Index]),
        Instruction::Length { .. } => length_failures(inputs, values),
        Instruction::Call { .. } | Instruction::CallMethod { .. } => Fact::Unknown,
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstFloat { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstNone { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::LoadCurrentFunction { .. }
        | Instruction::AddI64 { .. }
        | Instruction::LtI64 { .. }
        | Instruction::Move { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => Fact::Proven(Vec::new()),
    };
    (effects, failures)
}

fn binary_failures(
    op: BinaryOperator,
    site: OperationSiteId,
    operation_sites: &[OperationSite],
) -> Fact<Vec<FailureKind>> {
    let candidate = operation_sites.get(site.0 as usize);
    let type_failure = candidate.is_none_or(|site| {
        !matches!(
            (site.lhs, site.rhs),
            (TypeFact::Proven(_), TypeFact::Proven(_))
        )
    });
    let mut failures = Vec::new();
    if type_failure {
        failures.push(FailureKind::Type);
    }
    if matches!(op, BinaryOperator::Divide | BinaryOperator::FloorDivide) {
        failures.push(FailureKind::DivisionByZero);
    }
    if matches!(op, BinaryOperator::Power) {
        failures.push(FailureKind::Arithmetic);
    }
    Fact::Proven(failures)
}

fn compare_failures(
    site: OperationSiteId,
    operation_sites: &[OperationSite],
) -> Fact<Vec<FailureKind>> {
    match operation_sites.get(site.0 as usize) {
        Some(site)
            if matches!(site.lhs, TypeFact::Proven(_))
                && matches!(site.rhs, TypeFact::Proven(_)) =>
        {
            Fact::Proven(Vec::new())
        }
        Some(site)
            if matches!(site.lhs, TypeFact::Guardable(_))
                && matches!(site.rhs, TypeFact::Guardable(_)) =>
        {
            Fact::Guardable(Vec::new())
        }
        Some(_) => Fact::Proven(vec![FailureKind::Type]),
        None => Fact::Unknown,
    }
}

fn length_failures(inputs: &[ValueUse], values: &[ValueFact]) -> Fact<Vec<FailureKind>> {
    match inputs.first().map(|input| type_of(values, input)) {
        Some(TypeFact::Proven(SlotType::Object(
            ObjectKind::String | ObjectKind::Tuple | ObjectKind::List | ObjectKind::Dict,
        ))) => Fact::Proven(Vec::new()),
        Some(TypeFact::Guardable(SlotType::Object(
            ObjectKind::String | ObjectKind::Tuple | ObjectKind::List | ObjectKind::Dict,
        ))) => Fact::Guardable(Vec::new()),
        Some(TypeFact::Proven(_) | TypeFact::Guardable(_) | TypeFact::Unknown) | None => {
            Fact::Proven(vec![FailureKind::Type])
        }
    }
}

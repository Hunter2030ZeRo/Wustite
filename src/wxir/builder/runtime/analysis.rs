use super::*;

pub(in crate::wxir::builder) fn pointer_registers(
    executable: &ExecutableFunction,
    plan: &JitPlan,
    profile: Option<&Profile>,
) -> HashSet<Register> {
    let mut registers = plan
        .live_slots
        .iter()
        .filter_map(|slot| {
            let profiled_scalar = profile
                .and_then(|profile| profile.entry_tag(plan.region_id, slot.register))
                .and_then(profiled_type)
                .is_some();
            (matches!(slot.ty, SlotType::Object(_) | SlotType::Any) && !profiled_scalar)
                .then_some(slot.register)
        })
        .collect::<HashSet<_>>();
    for (pc, instruction) in executable.bytecode().code[plan.header..=plan.backedge]
        .iter()
        .enumerate()
        .map(|(offset, instruction)| (plan.header + offset, instruction))
    {
        let profiled_scalar = profile
            .and_then(|profile| profile.result_tag(pc))
            .and_then(profiled_type)
            .is_some();
        let output = match instruction {
            Instruction::LoadConstant { dst, .. }
            | Instruction::ConstNone { dst }
            | Instruction::BuildTuple { dst, .. }
            | Instruction::BuildList { dst, .. }
            | Instruction::BuildDict { dst, .. }
            | Instruction::GetItem { dst, .. }
            | Instruction::GetAttr { dst, .. }
            | Instruction::GetSlice { dst, .. }
            | Instruction::ListPop { dst, .. }
            | Instruction::LoadCurrentFunction { dst }
            | Instruction::Call { dst, .. } => (!profiled_scalar).then_some(*dst),
            Instruction::CallMethod { dst, .. } => (!profiled_scalar).then_some(*dst),
            Instruction::BinaryOp { dst, site, .. } => (executable
                .structure_map()
                .operation_site(*site)
                .is_none_or(|facts| {
                    !matches!(
                        facts.result,
                        TypeFact::Proven(SlotType::SmallInt | SlotType::Float | SlotType::Bool)
                    )
                })
                && !profiled_scalar)
                .then_some(*dst),
            Instruction::ConstSmallInt { .. }
            | Instruction::ConstFloat { .. }
            | Instruction::ConstBool { .. }
            | Instruction::ConstI64 { .. }
            | Instruction::CompareOp { .. }
            | Instruction::BooleanOp { .. }
            | Instruction::SetItem { .. }
            | Instruction::SetAttr { .. }
            | Instruction::SetSlice { .. }
            | Instruction::ListAppend { .. }
            | Instruction::ListInsert { .. }
            | Instruction::Length { .. }
            | Instruction::AddI64 { .. }
            | Instruction::LtI64 { .. }
            | Instruction::UnaryOp { .. }
            | Instruction::Move { .. }
            | Instruction::Jump { .. }
            | Instruction::Branch { .. }
            | Instruction::Return { .. } => None,
        };
        registers.extend(output);
    }
    loop {
        let before = registers.len();
        for instruction in &executable.bytecode().code[plan.header..=plan.backedge] {
            match instruction {
                Instruction::Move { dst, src } | Instruction::UnaryOp { dst, src, .. }
                    if registers.contains(src) =>
                {
                    registers.insert(*dst);
                }
                _ => {}
            }
        }
        if registers.len() == before {
            break;
        }
    }
    registers
}

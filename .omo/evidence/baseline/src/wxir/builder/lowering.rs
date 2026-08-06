use super::*;

impl RegionBuilder<'_> {
    pub(super) fn build_block(&mut self, start_pc: usize) -> Result<(), WxBuildError> {
        let spec =
            self.block_specs.get(&start_pc).cloned().ok_or_else(|| {
                WxBuildError::InvalidPlan(format!("missing block for pc {start_pc}"))
            })?;
        let mut environment: HashMap<Register, TypedValue> = spec
            .parameters
            .iter()
            .map(|(register, parameter)| {
                (
                    *register,
                    TypedValue {
                        id: parameter.id,
                        ty: parameter.ty,
                    },
                )
            })
            .collect();
        let mut instructions = Vec::new();
        let mut pc = spec.pc;

        let terminator = loop {
            if pc > self.plan.backedge {
                return Err(WxBuildError::InvalidPlan(format!(
                    "block starting at {start_pc} falls through the region"
                )));
            }
            if pc != start_pc && self.leaders.contains(&pc) {
                let target = self.internal_target(pc, &environment)?;
                break WxTerminator::Jump {
                    target: target.block,
                    arguments: target.arguments,
                };
            }

            match &self.executable.bytecode().code[pc] {
                Instruction::ConstSmallInt { dst, value }
                | Instruction::ConstI64 { dst, value } => {
                    self.emit_constant(
                        &mut instructions,
                        &mut environment,
                        *dst,
                        WxScalarType::I64,
                        super::super::ir::WxConstant::Int(*value),
                    )?;
                    pc += 1;
                }
                Instruction::ConstBool { dst, value } => {
                    self.emit_constant(
                        &mut instructions,
                        &mut environment,
                        *dst,
                        WxScalarType::I1,
                        super::super::ir::WxConstant::Bool(*value),
                    )?;
                    pc += 1;
                }
                Instruction::BinaryOp {
                    dst,
                    op: BinaryOperator::Add,
                    lhs,
                    rhs,
                    site,
                } => {
                    self.require_operation_facts(
                        pc,
                        *site,
                        TypeFact::Exact(SlotType::SmallInt),
                        TypeFact::Exact(SlotType::SmallInt),
                        TypeFact::Exact(SlotType::SmallInt),
                    )?;
                    self.emit_i64_add(&mut instructions, &mut environment, pc, *dst, *lhs, *rhs)?;
                    pc += 1;
                }
                Instruction::CompareOp {
                    dst,
                    op: CompareOperator::Lt,
                    lhs,
                    rhs,
                    site,
                } => {
                    self.require_operation_facts(
                        pc,
                        *site,
                        TypeFact::Exact(SlotType::SmallInt),
                        TypeFact::Exact(SlotType::SmallInt),
                        TypeFact::Exact(SlotType::Bool),
                    )?;
                    self.emit_i64_lt(&mut instructions, &mut environment, pc, *dst, *lhs, *rhs)?;
                    pc += 1;
                }
                Instruction::AddI64 { dst, lhs, rhs } => {
                    self.emit_i64_add(&mut instructions, &mut environment, pc, *dst, *lhs, *rhs)?;
                    pc += 1;
                }
                Instruction::LtI64 { dst, lhs, rhs } => {
                    self.emit_i64_lt(&mut instructions, &mut environment, pc, *dst, *lhs, *rhs)?;
                    pc += 1;
                }
                Instruction::Move { dst, src } => {
                    let value = environment
                        .get(src)
                        .copied()
                        .ok_or(WxBuildError::MissingRegister { pc, register: *src })?;
                    environment.insert(*dst, value);
                    pc += 1;
                }
                Instruction::Jump { target } => {
                    let target = self.control_target(*target, &environment)?;
                    break WxTerminator::Jump {
                        target: target.block,
                        arguments: target.arguments,
                    };
                }
                Instruction::Branch { cond, yes, no } => {
                    let condition =
                        self.read_register(&environment, pc, *cond, WxScalarType::I1)?;
                    let yes = self.control_target(*yes, &environment)?;
                    let no = self.control_target(*no, &environment)?;
                    break WxTerminator::Branch {
                        condition: condition.id,
                        yes,
                        no,
                    };
                }
                instruction @ (Instruction::ConstFloat { .. }
                | Instruction::LoadConstant { .. }
                | Instruction::BinaryOp { .. }
                | Instruction::CompareOp { .. }
                | Instruction::UnaryOp { .. }
                | Instruction::BooleanOp { .. }
                | Instruction::BuildTuple { .. }
                | Instruction::BuildList { .. }
                | Instruction::BuildDict { .. }
                | Instruction::GetItem { .. }
                | Instruction::SetItem { .. }
                | Instruction::Length { .. }
                | Instruction::LoadCurrentFunction { .. }
                | Instruction::Call { .. }
                | Instruction::Return { .. }) => {
                    return Err(WxBuildError::UnsupportedInstruction {
                        pc,
                        instruction: unsupported_instruction_name(instruction),
                    });
                }
            }
        };

        self.blocks.push(WxBlock {
            id: spec.id,
            parameters: spec
                .parameters
                .iter()
                .map(|(_, parameter)| *parameter)
                .collect(),
            instructions,
            terminator,
        });
        Ok(())
    }
}

fn unsupported_instruction_name(instruction: &Instruction) -> &'static str {
    match instruction {
        Instruction::ConstFloat { .. } => "ConstFloat",
        Instruction::LoadConstant { .. } => "LoadConstant",
        Instruction::BinaryOp { op, .. } => match op {
            BinaryOperator::Add => "BinaryOp::Add",
            BinaryOperator::Subtract => "BinaryOp::Subtract",
            BinaryOperator::Multiply => "BinaryOp::Multiply",
            BinaryOperator::Divide => "BinaryOp::Divide",
        },
        Instruction::CompareOp { op, .. } => match op {
            CompareOperator::Eq => "CompareOp::Eq",
            CompareOperator::NotEq => "CompareOp::NotEq",
            CompareOperator::Lt => "CompareOp::Lt",
            CompareOperator::Le => "CompareOp::Le",
            CompareOperator::Gt => "CompareOp::Gt",
            CompareOperator::Ge => "CompareOp::Ge",
        },
        Instruction::UnaryOp { .. } => "UnaryOp",
        Instruction::BooleanOp { .. } => "BooleanOp",
        Instruction::BuildTuple { .. } => "BuildTuple",
        Instruction::BuildList { .. } => "BuildList",
        Instruction::BuildDict { .. } => "BuildDict",
        Instruction::GetItem { .. } => "GetItem",
        Instruction::SetItem { .. } => "SetItem",
        Instruction::Length { .. } => "Length",
        Instruction::LoadCurrentFunction { .. } => "LoadCurrentFunction",
        Instruction::Call { .. } => "Call",
        Instruction::Return { .. } => "Return",
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::AddI64 { .. }
        | Instruction::LtI64 { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Move { .. } => "supported instruction",
    }
}

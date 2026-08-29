use super::*;

mod scalars;

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
                instruction @ Instruction::BinaryOp { .. } => {
                    self.lower_binary_operation(
                        &mut instructions,
                        &mut environment,
                        pc,
                        instruction,
                    )?;
                    pc += 1;
                }
                instruction @ Instruction::CompareOp { .. } => {
                    self.lower_compare_operation(
                        &mut instructions,
                        &mut environment,
                        pc,
                        instruction,
                    )?;
                    pc += 1;
                }
                Instruction::AddI64 { dst, lhs, rhs } => {
                    if self.i64_operation_requires_runtime(&environment, *dst, *lhs, *rhs) {
                        self.emit_runtime_call(
                            &mut instructions,
                            &mut environment,
                            pc,
                            &self.executable.bytecode().code[pc],
                        )?;
                    } else {
                        self.emit_i64_checked(
                            &mut instructions,
                            &mut environment,
                            WxIntOverflowOp::Add,
                            (pc, [*dst, *lhs, *rhs]),
                        )?;
                    }
                    pc += 1;
                }
                Instruction::LtI64 { dst, lhs, rhs } => {
                    if self.i64_operation_requires_runtime(&environment, *dst, *lhs, *rhs) {
                        self.emit_runtime_call(
                            &mut instructions,
                            &mut environment,
                            pc,
                            &self.executable.bytecode().code[pc],
                        )?;
                    } else {
                        self.emit_i64_compare(
                            &mut instructions,
                            &mut environment,
                            WxIntCompareOp::SignedLt,
                            (pc, [*dst, *lhs, *rhs]),
                        )?;
                    }
                    pc += 1;
                }
                Instruction::Move { dst, src } => {
                    let value = environment
                        .get(src)
                        .copied()
                        .ok_or(WxBuildError::MissingRegister { pc, register: *src })?;
                    if self.move_requires_runtime(*dst, value) {
                        self.emit_runtime_call(
                            &mut instructions,
                            &mut environment,
                            pc,
                            &self.executable.bytecode().code[pc],
                        )?;
                    } else {
                        environment.insert(*dst, value);
                    }
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
                Instruction::ConstFloat { dst, value } => {
                    self.emit_constant(
                        &mut instructions,
                        &mut environment,
                        *dst,
                        WxScalarType::F64,
                        super::super::ir::WxConstant::F64(*value),
                    )?;
                    pc += 1;
                }
                instruction @ Instruction::LoadConstant { .. } => {
                    if let Some(resume_pc) =
                        self.try_inline_numeric_leaf(&mut instructions, &mut environment, pc)?
                    {
                        pc = resume_pc;
                    } else {
                        self.emit_runtime_call(
                            &mut instructions,
                            &mut environment,
                            pc,
                            instruction,
                        )?;
                        pc += 1;
                    }
                }
                instruction @ Instruction::UnaryOp { .. } => {
                    self.lower_unary_operation(
                        &mut instructions,
                        &mut environment,
                        pc,
                        instruction,
                    )?;
                    pc += 1;
                }
                instruction @ Instruction::BooleanOp { .. } => {
                    self.lower_boolean_operation(
                        &mut instructions,
                        &mut environment,
                        pc,
                        instruction,
                    )?;
                    pc += 1;
                }
                instruction @ (Instruction::BuildTuple { .. } | Instruction::BuildList { .. }) => {
                    if let Some(resume_pc) = self.try_virtualize_container_access(
                        &mut instructions,
                        &mut environment,
                        pc,
                    )? {
                        pc = resume_pc;
                    } else {
                        self.emit_runtime_call(
                            &mut instructions,
                            &mut environment,
                            pc,
                            instruction,
                        )?;
                        pc += 1;
                    }
                }
                instruction @ (Instruction::GetItem { .. }
                | Instruction::GetSlice { .. }
                | Instruction::SetItem { .. }
                | Instruction::SetSlice { .. }
                | Instruction::ListAppend { .. }
                | Instruction::ListInsert { .. }
                | Instruction::ListPop { .. }
                | Instruction::Length { .. }) => {
                    self.emit_sequence_call(&mut instructions, &mut environment, pc, instruction)?;
                    pc += 1;
                }
                instruction @ (Instruction::ConstNone { .. }
                | Instruction::BuildDict { .. }
                | Instruction::GetAttr { .. }
                | Instruction::SetAttr { .. }
                | Instruction::LoadCurrentFunction { .. }
                | Instruction::Call { .. }
                | Instruction::CallMethod { .. }) => {
                    self.emit_runtime_call(&mut instructions, &mut environment, pc, instruction)?;
                    pc += 1;
                }
                Instruction::Return { .. } => {
                    let target = self.return_target(pc, &environment)?;
                    break WxTerminator::Jump {
                        target: target.block,
                        arguments: target.arguments,
                    };
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

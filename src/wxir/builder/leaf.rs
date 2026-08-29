use crate::executable::ExecutableConstant;

use super::*;

impl RegionBuilder<'_> {
    pub(super) fn try_inline_numeric_leaf(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
    ) -> Result<Option<usize>, WxBuildError> {
        let instruction_len = instructions.len();
        let exit_len = self.synthetic_exits.len();
        let next_value = self.next_value;
        let next_exit = self.next_exit;
        let result = self.try_inline_numeric_leaf_inner(instructions, environment, pc);
        if !matches!(result, Ok(Some(_))) {
            instructions.truncate(instruction_len);
            self.synthetic_exits.truncate(exit_len);
            self.next_value = next_value;
            self.next_exit = next_exit;
        }
        result
    }

    fn try_inline_numeric_leaf_inner(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
    ) -> Result<Option<usize>, WxBuildError> {
        let Instruction::LoadConstant {
            dst: callable_register,
            constant,
        } = &self.executable.bytecode().code[pc]
        else {
            return Ok(None);
        };
        let call_pc = pc.saturating_add(1);
        if call_pc > self.plan.backedge || self.leaders.contains(&call_pc) {
            return Ok(None);
        }
        let Some(Instruction::Call {
            dst,
            callable,
            args,
        }) = self.executable.bytecode().code.get(call_pc)
        else {
            return Ok(None);
        };
        if callable != callable_register {
            return Ok(None);
        }
        if self
            .plan
            .live_slots
            .iter()
            .any(|slot| slot.register == *callable_register)
            || self
                .executable
                .bytecode()
                .code
                .get(call_pc.saturating_add(1)..=self.plan.backedge)
                .into_iter()
                .flatten()
                .any(|instruction| instruction_reads(instruction, *callable_register))
        {
            return Ok(None);
        }
        let Some(ExecutableConstant::Function(callee)) =
            self.executable.constants().get(constant.0)
        else {
            return Ok(None);
        };
        if callee.parameters().len() != args.len() || callee.bytecode().register_count > 32 {
            return Ok(None);
        }

        let mut leaf_environment = HashMap::new();
        for (parameter, argument) in callee.parameters().iter().zip(args) {
            let value =
                environment
                    .get(argument)
                    .copied()
                    .ok_or(WxBuildError::MissingRegister {
                        pc: call_pc,
                        register: *argument,
                    })?;
            let Some(parameter_ty) = leaf_slot_type(parameter.ty) else {
                return Ok(None);
            };
            if value.ty != parameter_ty {
                return Ok(None);
            }
            leaf_environment.insert(parameter.register, value);
        }

        let caller_state = environment.clone();
        let mut returned = None;
        for leaf_instruction in &callee.bytecode().code {
            match leaf_instruction {
                Instruction::ConstSmallInt { dst, value }
                | Instruction::ConstI64 { dst, value } => self.emit_constant(
                    instructions,
                    &mut leaf_environment,
                    *dst,
                    WxScalarType::I64,
                    super::super::ir::WxConstant::Int(*value),
                )?,
                Instruction::ConstFloat { dst, value } => self.emit_constant(
                    instructions,
                    &mut leaf_environment,
                    *dst,
                    WxScalarType::F64,
                    super::super::ir::WxConstant::F64(*value),
                )?,
                Instruction::ConstBool { dst, value } => self.emit_constant(
                    instructions,
                    &mut leaf_environment,
                    *dst,
                    WxScalarType::I1,
                    super::super::ir::WxConstant::Bool(*value),
                )?,
                Instruction::BinaryOp {
                    dst, op, lhs, rhs, ..
                } => {
                    if !self.emit_inlined_binary(
                        instructions,
                        &mut leaf_environment,
                        &caller_state,
                        pc,
                        *dst,
                        *op,
                        *lhs,
                        *rhs,
                    )? {
                        return Ok(None);
                    }
                }
                Instruction::Move { dst, src } => {
                    let value = leaf_environment.get(src).copied().ok_or(
                        WxBuildError::MissingRegister {
                            pc: call_pc,
                            register: *src,
                        },
                    )?;
                    leaf_environment.insert(*dst, value);
                }
                Instruction::Return { src } => {
                    returned = leaf_environment.get(src).copied();
                    break;
                }
                _ => return Ok(None),
            }
        }
        let Some(result) = returned else {
            return Ok(None);
        };
        environment.insert(*dst, result);
        Ok(Some(call_pc.saturating_add(1)))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "leaf inlining preserves explicit caller and callee register identities"
    )]
    fn emit_inlined_binary(
        &mut self,
        instructions: &mut Vec<WxInst>,
        leaf: &mut HashMap<Register, TypedValue>,
        caller: &HashMap<Register, TypedValue>,
        call_pc: usize,
        dst: Register,
        op: BinaryOperator,
        lhs: Register,
        rhs: Register,
    ) -> Result<bool, WxBuildError> {
        let lhs_value = leaf
            .get(&lhs)
            .copied()
            .ok_or(WxBuildError::MissingRegister {
                pc: call_pc,
                register: lhs,
            })?;
        let rhs_value = leaf
            .get(&rhs)
            .copied()
            .ok_or(WxBuildError::MissingRegister {
                pc: call_pc,
                register: rhs,
            })?;
        let integer = WxType::Scalar(WxScalarType::I64);
        let float = WxType::Scalar(WxScalarType::F64);
        if lhs_value.ty == integer && rhs_value.ty == integer {
            if matches!(
                op,
                BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply
            ) {
                let overflow_op = match op {
                    BinaryOperator::Add => WxIntOverflowOp::Add,
                    BinaryOperator::Subtract => WxIntOverflowOp::Sub,
                    BinaryOperator::Multiply => WxIntOverflowOp::Mul,
                    _ => unreachable!(),
                };
                let result = self.allocate_value()?;
                let overflow = self.allocate_value()?;
                instructions.push(WxInst {
                    results: vec![
                        WxInstResult {
                            id: result,
                            ty: integer,
                        },
                        WxInstResult {
                            id: overflow,
                            ty: WxType::Scalar(WxScalarType::I1),
                        },
                    ],
                    kind: WxInstKind::IntegerBinaryWithOverflow {
                        op: overflow_op,
                        lhs: lhs_value.id,
                        rhs: rhs_value.id,
                    },
                });
                self.emit_replay_guard(instructions, caller, call_pc, overflow)?;
                leaf.insert(
                    dst,
                    TypedValue {
                        id: result,
                        ty: integer,
                    },
                );
                return Ok(true);
            }
            if op == BinaryOperator::FloorDivide {
                let zero = self.emit_leaf_constant(instructions, integer, 0)?;
                let divisor_is_zero =
                    self.emit_leaf_compare(instructions, WxIntCompareOp::Eq, rhs_value.id, zero)?;
                self.emit_replay_guard(instructions, caller, call_pc, divisor_is_zero)?;
                let minimum = self.emit_leaf_constant(instructions, integer, i64::MIN)?;
                let negative_one = self.emit_leaf_constant(instructions, integer, -1)?;
                let lhs_is_minimum = self.emit_leaf_compare(
                    instructions,
                    WxIntCompareOp::Eq,
                    lhs_value.id,
                    minimum,
                )?;
                let rhs_is_negative_one = self.emit_leaf_compare(
                    instructions,
                    WxIntCompareOp::Eq,
                    rhs_value.id,
                    negative_one,
                )?;
                let overflow = self.emit_leaf_boolean(
                    instructions,
                    WxIntBinaryOp::And,
                    lhs_is_minimum,
                    rhs_is_negative_one,
                )?;
                self.emit_replay_guard(instructions, caller, call_pc, overflow)?;
                let result = self.allocate_value()?;
                instructions.push(WxInst {
                    results: vec![WxInstResult {
                        id: result,
                        ty: integer,
                    }],
                    kind: WxInstKind::Binary {
                        op: WxBinaryOp::Integer(WxIntBinaryOp::FloorDiv),
                        lhs: lhs_value.id,
                        rhs: rhs_value.id,
                    },
                });
                leaf.insert(
                    dst,
                    TypedValue {
                        id: result,
                        ty: integer,
                    },
                );
                return Ok(true);
            }
        }
        if op == BinaryOperator::Power {
            return Ok(false);
        }
        let lhs = self.cast_leaf_float(instructions, lhs_value)?;
        let rhs = self.cast_leaf_float(instructions, rhs_value)?;
        if matches!(op, BinaryOperator::Divide | BinaryOperator::FloorDivide) {
            let zero = self.emit_leaf_float_constant(instructions, 0.0)?;
            let divisor_is_zero =
                self.emit_leaf_float_compare(instructions, WxFloatCompareOp::Eq, rhs, zero)?;
            self.emit_replay_guard(instructions, caller, call_pc, divisor_is_zero)?;
        }
        let float_op = match op {
            BinaryOperator::Add => WxFloatBinaryOp::Add,
            BinaryOperator::Subtract => WxFloatBinaryOp::Sub,
            BinaryOperator::Multiply => WxFloatBinaryOp::Mul,
            BinaryOperator::Divide => WxFloatBinaryOp::Div,
            BinaryOperator::FloorDivide | BinaryOperator::Power => return Ok(false),
        };
        let result = self.allocate_value()?;
        instructions.push(WxInst {
            results: vec![WxInstResult {
                id: result,
                ty: float,
            }],
            kind: WxInstKind::Binary {
                op: WxBinaryOp::Float(float_op),
                lhs,
                rhs,
            },
        });
        leaf.insert(
            dst,
            TypedValue {
                id: result,
                ty: float,
            },
        );
        Ok(true)
    }

    fn cast_leaf_float(
        &mut self,
        instructions: &mut Vec<WxInst>,
        value: TypedValue,
    ) -> Result<WxValueId, WxBuildError> {
        if value.ty == WxType::Scalar(WxScalarType::F64) {
            return Ok(value.id);
        }
        if value.ty != WxType::Scalar(WxScalarType::I64) {
            return Err(WxBuildError::UnsupportedSpecialization {
                pc: 0,
                reason: "numeric leaf operand is not an integer or float".to_string(),
            });
        }
        let result = self.allocate_value()?;
        instructions.push(WxInst {
            results: vec![WxInstResult {
                id: result,
                ty: WxType::Scalar(WxScalarType::F64),
            }],
            kind: WxInstKind::Cast {
                op: WxCastOp::IntToFloat { signed: true },
                value: value.id,
            },
        });
        Ok(result)
    }

    fn emit_leaf_constant(
        &mut self,
        instructions: &mut Vec<WxInst>,
        ty: WxType,
        value: i64,
    ) -> Result<WxValueId, WxBuildError> {
        let result = self.allocate_value()?;
        instructions.push(WxInst {
            results: vec![WxInstResult { id: result, ty }],
            kind: WxInstKind::Constant(super::super::ir::WxConstant::Int(value)),
        });
        Ok(result)
    }

    fn emit_leaf_float_constant(
        &mut self,
        instructions: &mut Vec<WxInst>,
        value: f64,
    ) -> Result<WxValueId, WxBuildError> {
        let result = self.allocate_value()?;
        instructions.push(WxInst {
            results: vec![WxInstResult {
                id: result,
                ty: WxType::Scalar(WxScalarType::F64),
            }],
            kind: WxInstKind::Constant(super::super::ir::WxConstant::F64(value)),
        });
        Ok(result)
    }

    fn emit_leaf_compare(
        &mut self,
        instructions: &mut Vec<WxInst>,
        op: WxIntCompareOp,
        lhs: WxValueId,
        rhs: WxValueId,
    ) -> Result<WxValueId, WxBuildError> {
        let result = self.allocate_value()?;
        instructions.push(WxInst {
            results: vec![WxInstResult {
                id: result,
                ty: WxType::Scalar(WxScalarType::I1),
            }],
            kind: WxInstKind::Compare {
                op: WxCompareOp::Integer(op),
                lhs,
                rhs,
            },
        });
        Ok(result)
    }

    fn emit_leaf_float_compare(
        &mut self,
        instructions: &mut Vec<WxInst>,
        op: WxFloatCompareOp,
        lhs: WxValueId,
        rhs: WxValueId,
    ) -> Result<WxValueId, WxBuildError> {
        let result = self.allocate_value()?;
        instructions.push(WxInst {
            results: vec![WxInstResult {
                id: result,
                ty: WxType::Scalar(WxScalarType::I1),
            }],
            kind: WxInstKind::Compare {
                op: WxCompareOp::Float(op),
                lhs,
                rhs,
            },
        });
        Ok(result)
    }

    fn emit_leaf_boolean(
        &mut self,
        instructions: &mut Vec<WxInst>,
        op: WxIntBinaryOp,
        lhs: WxValueId,
        rhs: WxValueId,
    ) -> Result<WxValueId, WxBuildError> {
        let result = self.allocate_value()?;
        let ty = WxType::Scalar(WxScalarType::I1);
        instructions.push(WxInst {
            results: vec![WxInstResult { id: result, ty }],
            kind: WxInstKind::Binary {
                op: WxBinaryOp::Integer(op),
                lhs,
                rhs,
            },
        });
        Ok(result)
    }

    pub(in crate::wxir::builder) fn emit_replay_guard(
        &mut self,
        instructions: &mut Vec<WxInst>,
        caller: &HashMap<Register, TypedValue>,
        pc: usize,
        condition: WxValueId,
    ) -> Result<(), WxBuildError> {
        let (exit, _) = self.create_replay_exit(pc, caller)?;
        instructions.push(WxInst {
            results: Vec::new(),
            kind: WxInstKind::Guard {
                condition,
                exit,
                mode: WxGuardMode::ExitWhenTrue,
            },
        });
        Ok(())
    }
}

const fn leaf_slot_type(ty: SlotType) -> Option<WxType> {
    match ty {
        SlotType::SmallInt => Some(WxType::Scalar(WxScalarType::I64)),
        SlotType::Float => Some(WxType::Scalar(WxScalarType::F64)),
        SlotType::Bool => Some(WxType::Scalar(WxScalarType::I1)),
        SlotType::Object(_) | SlotType::Any => None,
    }
}

fn instruction_reads(instruction: &Instruction, register: Register) -> bool {
    let contains = |registers: &[Register]| registers.contains(&register);
    match instruction {
        Instruction::BinaryOp { lhs, rhs, .. }
        | Instruction::CompareOp { lhs, rhs, .. }
        | Instruction::BooleanOp { lhs, rhs, .. }
        | Instruction::AddI64 { lhs, rhs, .. }
        | Instruction::LtI64 { lhs, rhs, .. } => [*lhs, *rhs].contains(&register),
        Instruction::UnaryOp { src, .. } | Instruction::Move { src, .. } => *src == register,
        Instruction::BuildTuple { items, .. } | Instruction::BuildList { items, .. } => {
            contains(items)
        }
        Instruction::BuildDict { entries, .. } => entries
            .iter()
            .any(|(key, value)| *key == register || *value == register),
        Instruction::GetItem { object, key, .. } => [*object, *key].contains(&register),
        Instruction::GetAttr { object, .. } => *object == register,
        Instruction::GetSlice {
            object,
            start,
            stop,
            step,
            ..
        }
        | Instruction::SetSlice {
            object,
            start,
            stop,
            step,
            ..
        } => {
            *object == register
                || [*start, *stop, *step]
                    .into_iter()
                    .flatten()
                    .any(|candidate| candidate == register)
                || matches!(instruction, Instruction::SetSlice { value, .. } if *value == register)
        }
        Instruction::SetItem {
            object, key, value, ..
        }
        | Instruction::ListInsert {
            list: object,
            index: key,
            value,
        } => [*object, *key, *value].contains(&register),
        Instruction::SetAttr { object, value, .. } => [*object, *value].contains(&register),
        Instruction::ListAppend { list, value } => [*list, *value].contains(&register),
        Instruction::ListPop { list, index, .. } => [*list, *index].contains(&register),
        Instruction::Length { object, .. } => *object == register,
        Instruction::Call { callable, args, .. } => *callable == register || contains(args),
        Instruction::CallMethod { receiver, args, .. } => *receiver == register || contains(args),
        Instruction::Branch { cond, .. } => *cond == register,
        Instruction::Return { src } => *src == register,
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstFloat { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstNone { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::LoadCurrentFunction { .. }
        | Instruction::Jump { .. } => false,
    }
}

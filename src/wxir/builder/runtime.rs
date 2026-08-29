use super::control::slot_type;
use super::*;

mod analysis;

pub(super) use analysis::pointer_registers;

impl RegionBuilder<'_> {
    pub(super) fn emit_runtime_call(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        instruction: &Instruction,
    ) -> Result<(), WxBuildError> {
        let mut input_registers = runtime_inputs(instruction)?;
        input_registers.sort_unstable();
        input_registers.dedup();
        let inputs = input_registers
            .into_iter()
            .map(|register| {
                let value = environment
                    .get(&register)
                    .copied()
                    .ok_or(WxBuildError::MissingRegister { pc, register })?;
                Ok(WxRuntimeInput {
                    register,
                    value: value.id,
                    ty: value.ty,
                })
            })
            .collect::<Result<Vec<_>, WxBuildError>>()?;
        let output = self.runtime_output(environment, pc, instruction)?;
        let results = match output {
            Some((_, value)) => vec![WxInstResult {
                id: value.id,
                ty: value.ty,
            }],
            None => Vec::new(),
        };
        instructions.push(WxInst {
            results,
            kind: WxInstKind::RuntimeCall {
                pc: u32::try_from(pc).map_err(|_| WxBuildError::IdSpaceExhausted("bytecode pc"))?,
                inputs,
                output: output.map(|(register, _)| register),
                effects: runtime_effects(instruction),
            },
        });
        if let Some((register, value)) = output {
            environment.insert(register, value);
        }
        Ok(())
    }

    pub(super) fn emit_sequence_call(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        instruction: &Instruction,
    ) -> Result<(), WxBuildError> {
        let mut registers = runtime_inputs(instruction)?;
        registers.sort_unstable();
        registers.dedup();
        let inputs = registers
            .into_iter()
            .map(|register| {
                let value = environment
                    .get(&register)
                    .copied()
                    .ok_or(WxBuildError::MissingRegister { pc, register })?;
                Ok(WxRuntimeInput {
                    register,
                    value: value.id,
                    ty: value.ty,
                })
            })
            .collect::<Result<Vec<_>, WxBuildError>>()?;
        let output = self.runtime_output(environment, pc, instruction)?;
        let results = output.map_or_else(Vec::new, |(_, value)| {
            vec![WxInstResult {
                id: value.id,
                ty: value.ty,
            }]
        });
        let pc = u32::try_from(pc).map_err(|_| WxBuildError::IdSpaceExhausted("bytecode pc"))?;
        let (strategy, profiled) = self.sequence_strategy(pc as usize, instruction);
        let kind = match instruction {
            Instruction::Length { object, dst } => WxInstKind::SequenceLength {
                pc,
                object: *object,
                inputs,
                output: *dst,
                strategy,
                profiled,
            },
            Instruction::GetItem { object, dst, .. } => WxInstKind::SequenceGet {
                pc,
                object: *object,
                inputs,
                output: *dst,
                strategy,
                profiled,
            },
            Instruction::SetItem { object, .. } => WxInstKind::SequenceSet {
                pc,
                object: *object,
                inputs,
                strategy,
                profiled,
            },
            Instruction::GetSlice { object, dst, .. } => WxInstKind::SequenceMutate {
                pc,
                object: *object,
                operation: WxSequenceMutation::GetSlice,
                inputs,
                output: Some(*dst),
                strategy,
                profiled,
            },
            Instruction::SetSlice { object, .. } => WxInstKind::SequenceMutate {
                pc,
                object: *object,
                operation: WxSequenceMutation::SetSlice,
                inputs,
                output: None,
                strategy,
                profiled,
            },
            Instruction::ListAppend { list, .. } => WxInstKind::SequenceMutate {
                pc,
                object: *list,
                operation: WxSequenceMutation::Append,
                inputs,
                output: None,
                strategy,
                profiled,
            },
            Instruction::ListInsert { list, .. } => WxInstKind::SequenceMutate {
                pc,
                object: *list,
                operation: WxSequenceMutation::Insert,
                inputs,
                output: None,
                strategy,
                profiled,
            },
            Instruction::ListPop { list, dst, .. } => WxInstKind::SequenceMutate {
                pc,
                object: *list,
                operation: WxSequenceMutation::Pop,
                inputs,
                output: Some(*dst),
                strategy,
                profiled,
            },
            _ => {
                return Err(WxBuildError::UnsupportedInstruction {
                    pc: usize::try_from(pc).unwrap_or(usize::MAX),
                    instruction: "sequence lowering",
                });
            }
        };
        instructions.push(WxInst { results, kind });
        if let Some((register, value)) = output {
            environment.insert(register, value);
        }
        Ok(())
    }

    fn sequence_strategy(
        &self,
        pc: usize,
        instruction: &Instruction,
    ) -> (Option<SequenceStrategy>, bool) {
        if let Some(SequenceSpecialization::Monomorphic(case)) = self
            .profile
            .map(|profile| profile.sequence_specialization(pc))
        {
            return (Some(case.strategy), true);
        }
        let object = match instruction {
            Instruction::GetItem { object, .. }
            | Instruction::GetSlice { object, .. }
            | Instruction::SetItem { object, .. }
            | Instruction::SetSlice { object, .. }
            | Instruction::Length { object, .. } => *object,
            Instruction::ListAppend { list, .. }
            | Instruction::ListInsert { list, .. }
            | Instruction::ListPop { list, .. } => *list,
            _ => return (None, false),
        };
        if let Some(SequenceSpecialization::Monomorphic(case)) = self
            .profile
            .map(|profile| profile.entry_sequence_specialization(self.plan.region_id, object))
        {
            return (Some(case.strategy), true);
        }
        let Some(fact) = self.executable.structure_map().instruction_fact(pc) else {
            return (None, false);
        };
        let strategy = fact
            .inputs
            .iter()
            .find(|input| input.register == object)
            .and_then(|input| input.value)
            .and_then(|id| self.executable.structure_map().value(id))
            .and_then(|value| value.sequence.strategy.proven().copied());
        (strategy, false)
    }

    pub(super) fn move_requires_runtime(&self, dst: Register, source: TypedValue) -> bool {
        source.ty == WxType::Scalar(WxScalarType::RuntimeHandle)
            || self.pointer_registers.contains(&dst)
    }

    pub(super) fn i64_operation_requires_runtime(
        &self,
        environment: &HashMap<Register, TypedValue>,
        dst: Register,
        lhs: Register,
        rhs: Register,
    ) -> bool {
        let i64_type = WxType::Scalar(WxScalarType::I64);
        environment
            .get(&lhs)
            .is_none_or(|value| value.ty != i64_type)
            || environment
                .get(&rhs)
                .is_none_or(|value| value.ty != i64_type)
            || self.pointer_registers.contains(&dst)
    }

    fn runtime_output(
        &mut self,
        environment: &HashMap<Register, TypedValue>,
        pc: usize,
        instruction: &Instruction,
    ) -> Result<Option<(Register, TypedValue)>, WxBuildError> {
        let (register, default_ty) = match instruction {
            Instruction::ConstFloat { dst, .. } => (*dst, WxType::Scalar(WxScalarType::F64)),
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
            | Instruction::Call { dst, .. } => (*dst, WxType::Scalar(WxScalarType::RuntimeHandle)),
            Instruction::CallMethod { dst, .. } => {
                (*dst, WxType::Scalar(WxScalarType::RuntimeHandle))
            }
            Instruction::BinaryOp { dst, site, .. } => {
                let facts = self
                    .executable
                    .structure_map()
                    .operation_site(*site)
                    .ok_or_else(|| WxBuildError::UnsupportedSpecialization {
                        pc,
                        reason: format!("missing operation site {}", site.0),
                    })?;
                let ty = match facts.result {
                    TypeFact::Proven(ty) => slot_type(ty),
                    TypeFact::Guardable(_) | TypeFact::Unknown => {
                        WxType::Scalar(WxScalarType::RuntimeHandle)
                    }
                };
                (*dst, ty)
            }
            Instruction::CompareOp { dst, .. } | Instruction::BooleanOp { dst, .. } => {
                (*dst, WxType::Scalar(WxScalarType::I1))
            }
            Instruction::UnaryOp { dst, src, .. } => {
                let source = environment
                    .get(src)
                    .copied()
                    .ok_or(WxBuildError::MissingRegister { pc, register: *src })?;
                (*dst, source.ty)
            }
            Instruction::Length { dst, .. } => (*dst, WxType::Scalar(WxScalarType::I64)),
            Instruction::AddI64 { dst, .. } => (*dst, WxType::Scalar(WxScalarType::I64)),
            Instruction::LtI64 { dst, .. } => (*dst, WxType::Scalar(WxScalarType::I1)),
            Instruction::Move { dst, .. } => (*dst, WxType::Scalar(WxScalarType::RuntimeHandle)),
            Instruction::SetItem { .. }
            | Instruction::SetAttr { .. }
            | Instruction::SetSlice { .. }
            | Instruction::ListAppend { .. }
            | Instruction::ListInsert { .. } => return Ok(None),
            Instruction::ConstSmallInt { .. }
            | Instruction::ConstBool { .. }
            | Instruction::ConstI64 { .. }
            | Instruction::Jump { .. }
            | Instruction::Branch { .. }
            | Instruction::Return { .. } => {
                return Err(WxBuildError::UnsupportedInstruction {
                    pc,
                    instruction: "native runtime call",
                });
            }
        };
        let ty = self
            .profile
            .and_then(|profile| profile.result_tag(pc))
            .and_then(profiled_type)
            .unwrap_or(default_ty);
        let ty = self
            .plan
            .live_slots
            .iter()
            .find(|slot| slot.register == register)
            .map(|slot| self.state_type(register, slot.ty))
            .unwrap_or(ty);
        let value = TypedValue {
            id: self.allocate_value()?,
            ty,
        };
        Ok(Some((register, value)))
    }
}

fn runtime_effects(instruction: &Instruction) -> crate::structure_map::EffectSummary {
    let unknown_call = matches!(
        instruction,
        Instruction::Call { .. } | Instruction::CallMethod { .. }
    );
    crate::structure_map::EffectSummary {
        may_mutate: unknown_call
            || matches!(
                instruction,
                Instruction::SetItem { .. }
                    | Instruction::SetAttr { .. }
                    | Instruction::SetSlice { .. }
                    | Instruction::ListAppend { .. }
                    | Instruction::ListInsert { .. }
                    | Instruction::ListPop { .. }
            ),
        may_allocate: unknown_call
            || matches!(
                instruction,
                Instruction::LoadConstant { .. }
                    | Instruction::BuildTuple { .. }
                    | Instruction::BuildList { .. }
                    | Instruction::BuildDict { .. }
                    | Instruction::GetSlice { .. }
            ),
        may_call_unknown: unknown_call,
        may_access_global_state: unknown_call,
    }
}

fn runtime_inputs(instruction: &Instruction) -> Result<Vec<Register>, WxBuildError> {
    let registers = match instruction {
        Instruction::ConstFloat { .. }
        | Instruction::ConstNone { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::LoadCurrentFunction { .. } => Vec::new(),
        Instruction::BinaryOp { lhs, rhs, .. }
        | Instruction::CompareOp { lhs, rhs, .. }
        | Instruction::BooleanOp { lhs, rhs, .. } => vec![*lhs, *rhs],
        Instruction::UnaryOp { src, .. } | Instruction::Move { src, .. } => vec![*src],
        Instruction::BuildTuple { items, .. } | Instruction::BuildList { items, .. } => {
            items.clone()
        }
        Instruction::BuildDict { entries, .. } => entries
            .iter()
            .flat_map(|(key, value)| [*key, *value])
            .collect(),
        Instruction::GetItem { object, key, .. } => vec![*object, *key],
        Instruction::GetAttr { object, .. } => vec![*object],
        Instruction::GetSlice {
            object,
            start,
            stop,
            step,
            ..
        } => std::iter::once(*object)
            .chain([*start, *stop, *step].into_iter().flatten())
            .collect(),
        Instruction::SetItem {
            object, key, value, ..
        } => vec![*object, *key, *value],
        Instruction::SetAttr { object, value, .. } => vec![*object, *value],
        Instruction::SetSlice {
            object,
            start,
            stop,
            step,
            value,
        } => std::iter::once(*object)
            .chain([*start, *stop, *step].into_iter().flatten())
            .chain(std::iter::once(*value))
            .collect(),
        Instruction::ListAppend { list, value } => vec![*list, *value],
        Instruction::ListInsert { list, index, value } => vec![*list, *index, *value],
        Instruction::ListPop { list, index, .. } => vec![*list, *index],
        Instruction::Length { object, .. } => vec![*object],
        Instruction::Call { callable, args, .. } => std::iter::once(*callable)
            .chain(args.iter().copied())
            .collect(),
        Instruction::CallMethod { receiver, args, .. } => std::iter::once(*receiver)
            .chain(args.iter().copied())
            .collect(),
        Instruction::AddI64 { lhs, rhs, .. } | Instruction::LtI64 { lhs, rhs, .. } => {
            vec![*lhs, *rhs]
        }
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => {
            return Err(WxBuildError::UnsupportedInstruction {
                pc: 0,
                instruction: "native runtime input",
            });
        }
    };
    Ok(registers)
}

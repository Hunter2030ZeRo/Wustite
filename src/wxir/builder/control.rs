use super::*;

mod exits;

impl RegionBuilder<'_> {
    pub(super) fn control_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        if self.leaders.contains(&target) {
            self.internal_target(target, environment)
        } else {
            self.exit_target(target, environment)
        }
    }

    pub(super) fn internal_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        if !self.leaders.contains(&target) {
            return Err(WxBuildError::InvalidPlan(format!(
                "pc {target} is not a region block leader"
            )));
        }

        if !self.block_specs.contains_key(&target) {
            let id = self.allocate_block()?;
            let registers = self.registers_for_environment(target, environment);
            let mut parameters = Vec::with_capacity(registers.len());
            for register in registers {
                let value =
                    environment
                        .get(&register)
                        .copied()
                        .ok_or(WxBuildError::MissingRegister {
                            pc: target,
                            register,
                        })?;
                parameters.push((
                    register,
                    WxBlockParam {
                        id: self.allocate_value()?,
                        ty: value.ty,
                    },
                ));
            }
            self.block_specs.insert(
                target,
                BlockSpec {
                    id,
                    pc: target,
                    parameters,
                },
            );
            self.queue.push_back(target);
        }

        let spec =
            self.block_specs.get(&target).cloned().ok_or_else(|| {
                WxBuildError::InvalidPlan(format!("missing block for pc {target}"))
            })?;
        let arguments = self.arguments_for(&spec.parameters, environment, target)?;
        Ok(WxBlockTarget {
            block: spec.id,
            arguments,
        })
    }

    pub(super) fn registers_for_environment(
        &self,
        pc: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Vec<Register> {
        let mut registers = environment
            .keys()
            .filter(|register| {
                self.live_registers
                    .get(&pc)
                    .is_none_or(|live| live.contains(register))
            })
            .copied()
            .collect::<Vec<_>>();
        registers.sort_unstable();
        registers
    }

    pub(super) fn parameters_for_slots(
        &mut self,
        slots: &[StateSlot],
        _pc: usize,
    ) -> Result<Vec<(Register, WxBlockParam)>, WxBuildError> {
        slots
            .iter()
            .map(|slot| {
                Ok((
                    slot.register,
                    WxBlockParam {
                        id: self.allocate_value()?,
                        ty: self.state_type(slot.register, slot.ty),
                    },
                ))
            })
            .collect()
    }

    fn arguments_for(
        &self,
        parameters: &[(Register, WxBlockParam)],
        environment: &HashMap<Register, TypedValue>,
        pc: usize,
    ) -> Result<Vec<WxValueId>, WxBuildError> {
        parameters
            .iter()
            .map(|(register, parameter)| {
                let value =
                    environment
                        .get(register)
                        .copied()
                        .ok_or(WxBuildError::MissingRegister {
                            pc,
                            register: *register,
                        })?;
                if value.ty != parameter.ty {
                    return Err(WxBuildError::TypeMismatch {
                        pc,
                        register: *register,
                        expected: parameter.ty,
                        actual: value.ty,
                    });
                }
                Ok(value.id)
            })
            .collect()
    }

    pub(super) fn read_register(
        &self,
        environment: &HashMap<Register, TypedValue>,
        pc: usize,
        register: Register,
        expected: WxScalarType,
    ) -> Result<TypedValue, WxBuildError> {
        let value = environment
            .get(&register)
            .copied()
            .ok_or(WxBuildError::MissingRegister { pc, register })?;
        let expected = WxType::Scalar(expected);
        if value.ty == expected {
            Ok(value)
        } else {
            Err(WxBuildError::TypeMismatch {
                pc,
                register,
                expected,
                actual: value.ty,
            })
        }
    }

    pub(super) fn state_type(&self, register: Register, slot: SlotType) -> WxType {
        if self.pointer_registers.contains(&register) {
            WxType::Scalar(WxScalarType::RuntimeHandle)
        } else if let Some(ty) = self
            .profile
            .and_then(|profile| profile.entry_tag(self.plan.region_id, register))
            .and_then(profiled_type)
        {
            ty
        } else {
            slot_type(slot)
        }
    }
}

pub(super) const fn slot_type(slot_type: SlotType) -> WxType {
    match slot_type {
        SlotType::SmallInt => WxType::Scalar(WxScalarType::I64),
        SlotType::Float => WxType::Scalar(WxScalarType::F64),
        SlotType::Bool => WxType::Scalar(WxScalarType::I1),
        SlotType::Object(_) | SlotType::Any => WxType::Scalar(WxScalarType::RuntimeHandle),
    }
}

use super::*;

impl RegionBuilder<'_> {
    pub(super) fn control_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        if (self.plan.header..=self.plan.backedge).contains(&target) {
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
            let mut registers: Vec<_> = environment.keys().copied().collect();
            registers.sort_unstable();
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

    fn exit_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        let exit_index = self.exit_by_pc.get(&target).copied().ok_or_else(|| {
            WxBuildError::InvalidPlan(format!("region edge to pc {target} has no JitPlan exit"))
        })?;

        if !self.exit_specs.contains_key(&target) {
            let block_id = self.allocate_block()?;
            let exit_id = WxExitId(
                u32::try_from(exit_index)
                    .map_err(|_| WxBuildError::IdSpaceExhausted("side-exit"))?,
            );
            let parameters = self.parameters_for_slots(&self.plan.live_slots, target)?;
            self.exit_specs.insert(
                target,
                ExitBlockSpec {
                    id: block_id,
                    exit: exit_id,
                    resume_pc: target,
                    parameters,
                },
            );
        }

        let spec =
            self.exit_specs.get(&target).cloned().ok_or_else(|| {
                WxBuildError::InvalidPlan(format!("missing exit for pc {target}"))
            })?;
        let arguments = self.arguments_for(&spec.parameters, environment, target)?;
        Ok(WxBlockTarget {
            block: spec.id,
            arguments,
        })
    }

    pub(super) fn parameters_for_slots(
        &mut self,
        slots: &[LiveSlot],
        pc: usize,
    ) -> Result<Vec<(Register, WxBlockParam)>, WxBuildError> {
        slots
            .iter()
            .map(|slot| {
                Ok((
                    slot.register,
                    WxBlockParam {
                        id: self.allocate_value()?,
                        ty: slot_type(slot.ty, pc)?,
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
}

fn slot_type(slot_type: SlotType, pc: usize) -> Result<WxType, WxBuildError> {
    match slot_type {
        SlotType::SmallInt => Ok(WxType::Scalar(WxScalarType::I64)),
        SlotType::Bool => Ok(WxType::Scalar(WxScalarType::I1)),
        SlotType::Float | SlotType::Object(_) | SlotType::Any => {
            Err(WxBuildError::UnsupportedSpecialization {
                pc,
                reason: "live slot type is not supported by WXIR".to_string(),
            })
        }
    }
}

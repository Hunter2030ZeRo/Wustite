use super::*;

impl RegionBuilder<'_> {
    pub(in crate::wxir::builder) fn return_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        if self.exit_by_pc.contains_key(&target) {
            self.dynamic_exit_target(target, environment)
        } else {
            self.replay_target(target, environment)
        }
    }

    pub(in crate::wxir::builder) fn exit_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        let exit_index = self.exit_index(target)?;
        if !self.exit_specs.contains_key(&target) {
            let block_id = self.allocate_block()?;
            let parameters = self.parameters_for_slots(&self.plan.live_slots, target)?;
            self.exit_specs.insert(
                target,
                ExitBlockSpec {
                    id: block_id,
                    exit: exit_id(exit_index)?,
                    resume_pc: target,
                    parameters,
                },
            );
        }
        self.existing_exit_target(target, environment)
    }

    fn dynamic_exit_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        let exit_index = self.exit_index(target)?;
        if !self.exit_specs.contains_key(&target) {
            let block_id = self.allocate_block()?;
            let parameters = self.parameters_for_environment(target, environment)?;
            self.exit_specs.insert(
                target,
                ExitBlockSpec {
                    id: block_id,
                    exit: exit_id(exit_index)?,
                    resume_pc: target,
                    parameters,
                },
            );
        }
        self.existing_exit_target(target, environment)
    }

    fn existing_exit_target(
        &self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
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

    fn replay_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        let block = self.allocate_block()?;
        let parameters = self.parameters_for_environment(target, environment)?;
        let parameter_environment = parameters
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
        let (exit, values) = self.create_replay_exit(target, &parameter_environment)?;
        let arguments = self.arguments_for(&parameters, environment, target)?;
        self.blocks.push(WxBlock {
            id: block,
            parameters: parameters.iter().map(|(_, parameter)| *parameter).collect(),
            instructions: Vec::new(),
            terminator: WxTerminator::SideExit { exit, values },
        });
        Ok(WxBlockTarget { block, arguments })
    }

    fn parameters_for_environment(
        &mut self,
        pc: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<Vec<(Register, WxBlockParam)>, WxBuildError> {
        let registers = self.registers_for_environment(pc, environment);
        registers
            .into_iter()
            .map(|register| {
                let value = environment
                    .get(&register)
                    .copied()
                    .ok_or(WxBuildError::MissingRegister { pc, register })?;
                Ok((
                    register,
                    WxBlockParam {
                        id: self.allocate_value()?,
                        ty: value.ty,
                    },
                ))
            })
            .collect()
    }

    fn exit_index(&self, target: usize) -> Result<usize, WxBuildError> {
        self.exit_by_pc.get(&target).copied().ok_or_else(|| {
            WxBuildError::InvalidPlan(format!("region edge to pc {target} has no JitPlan exit"))
        })
    }
}

fn exit_id(index: usize) -> Result<WxExitId, WxBuildError> {
    u32::try_from(index)
        .map(WxExitId)
        .map_err(|_| WxBuildError::IdSpaceExhausted("side-exit"))
}

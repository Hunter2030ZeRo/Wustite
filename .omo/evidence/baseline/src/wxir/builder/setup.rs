use super::*;

impl<'a> RegionBuilder<'a> {
    pub(super) fn new(
        executable: &'a ExecutableFunction,
        plan: &'a JitPlan,
    ) -> Result<Self, WxBuildError> {
        let mut leaders = HashSet::from([plan.header]);
        let mut exit_by_pc = HashMap::new();

        for (index, exit) in plan.exits.iter().enumerate() {
            if exit_by_pc.insert(exit.target, index).is_some() {
                return Err(WxBuildError::InvalidPlan(format!(
                    "multiple exits resume at bytecode pc {}",
                    exit.target
                )));
            }
        }

        for pc in plan.header..=plan.backedge {
            match &executable.bytecode().code[pc] {
                Instruction::Jump { target } => {
                    if (plan.header..=plan.backedge).contains(target) {
                        leaders.insert(*target);
                    }
                }
                Instruction::Branch { yes, no, .. } => {
                    if (plan.header..=plan.backedge).contains(yes) {
                        leaders.insert(*yes);
                    }
                    if (plan.header..=plan.backedge).contains(no) {
                        leaders.insert(*no);
                    }
                }
                Instruction::ConstSmallInt { .. }
                | Instruction::ConstFloat { .. }
                | Instruction::ConstBool { .. }
                | Instruction::LoadConstant { .. }
                | Instruction::ConstI64 { .. }
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
                | Instruction::AddI64 { .. }
                | Instruction::LtI64 { .. }
                | Instruction::Return { .. }
                | Instruction::Move { .. } => {}
            }
        }

        let mut builder = Self {
            executable,
            plan,
            leaders,
            exit_by_pc,
            block_specs: HashMap::new(),
            exit_specs: HashMap::new(),
            synthetic_exits: Vec::new(),
            queue: VecDeque::new(),
            built: HashSet::new(),
            blocks: Vec::new(),
            next_value: 0,
            next_block: 0,
            next_exit: u32::try_from(plan.exits.len())
                .map_err(|_| WxBuildError::IdSpaceExhausted("side-exit"))?,
        };

        let entry_id = builder.allocate_block()?;
        let parameters = builder.parameters_for_slots(&plan.live_slots, plan.header)?;
        builder.block_specs.insert(
            plan.header,
            BlockSpec {
                id: entry_id,
                pc: plan.header,
                parameters,
            },
        );
        builder.queue.push_back(plan.header);
        Ok(builder)
    }

    pub(super) fn build(&mut self) -> Result<WxFunction, WxBuildError> {
        while let Some(pc) = self.queue.pop_front() {
            if self.built.insert(pc) {
                self.build_block(pc)?;
            }
        }

        if self.exit_specs.len() != self.plan.exits.len() {
            let missing = self
                .plan
                .exits
                .iter()
                .find(|exit| !self.exit_specs.contains_key(&exit.target))
                .map(|exit| exit.target)
                .ok_or_else(|| {
                    WxBuildError::InvalidPlan(
                        "exit block count does not match the JIT plan".to_string(),
                    )
                })?;
            return Err(WxBuildError::InvalidPlan(format!(
                "exit at bytecode pc {missing} is not reachable from the region"
            )));
        }

        let mut exit_specs: Vec<_> = self.exit_specs.values().cloned().collect();
        exit_specs.sort_by_key(|spec| spec.exit.0);
        let mut side_exits = Vec::with_capacity(exit_specs.len());

        for spec in exit_specs {
            let values = spec
                .parameters
                .iter()
                .map(|(_, parameter)| parameter.id)
                .collect();
            let state = spec
                .parameters
                .iter()
                .map(|(register, parameter)| WxStateValue {
                    register: *register,
                    value: parameter.id,
                    ty: parameter.ty,
                })
                .collect();

            self.blocks.push(WxBlock {
                id: spec.id,
                parameters: spec
                    .parameters
                    .iter()
                    .map(|(_, parameter)| *parameter)
                    .collect(),
                instructions: Vec::new(),
                terminator: WxTerminator::SideExit {
                    exit: spec.exit,
                    values,
                },
            });
            side_exits.push(WxSideExit {
                id: spec.exit,
                kind: WxExitKind::RegionExit,
                resume_pc: spec.resume_pc,
                state,
            });
        }
        side_exits.append(&mut self.synthetic_exits);
        side_exits.sort_by_key(|side_exit| side_exit.id.0);

        let entry_spec = self
            .block_specs
            .get(&self.plan.header)
            .ok_or_else(|| WxBuildError::InvalidPlan("missing entry block".to_string()))?;
        let entry_state = entry_spec
            .parameters
            .iter()
            .map(|(register, parameter)| WxStateValue {
                register: *register,
                value: parameter.id,
                ty: parameter.ty,
            })
            .collect();

        Ok(WxFunction {
            origin: WxRegionOrigin {
                region_id: self.plan.region_id,
                bytecode_header: self.plan.header,
                bytecode_backedge: self.plan.backedge,
            },
            entry: entry_spec.id,
            entry_state,
            blocks: std::mem::take(&mut self.blocks),
            returns: Vec::new(),
            side_exits,
        })
    }
}

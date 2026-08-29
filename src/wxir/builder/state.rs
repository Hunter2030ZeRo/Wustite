use super::*;

impl RegionBuilder<'_> {
    pub(super) fn create_replay_exit(
        &mut self,
        pc: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<(WxExitId, Vec<WxValueId>), WxBuildError> {
        let exit = WxExitId(self.next_exit);
        self.next_exit = self
            .next_exit
            .checked_add(1)
            .ok_or(WxBuildError::IdSpaceExhausted("side-exit"))?;

        let registers = self.registers_for_environment(pc, environment);
        let state: Vec<WxStateValue> = registers
            .into_iter()
            .map(|register| {
                let value = environment
                    .get(&register)
                    .copied()
                    .ok_or(WxBuildError::MissingRegister { pc, register })?;
                Ok(WxStateValue {
                    register,
                    value: value.id,
                    ty: value.ty,
                })
            })
            .collect::<Result<_, WxBuildError>>()?;
        let values = state.iter().map(|slot| slot.value).collect();
        self.synthetic_exits.push(WxSideExit {
            id: exit,
            kind: WxExitKind::ReplayInstruction,
            resume_pc: pc,
            state,
        });
        Ok((exit, values))
    }

    pub(super) fn allocate_value(&mut self) -> Result<WxValueId, WxBuildError> {
        let id = WxValueId(self.next_value);
        self.next_value = self
            .next_value
            .checked_add(1)
            .ok_or(WxBuildError::IdSpaceExhausted("value"))?;
        Ok(id)
    }

    pub(super) fn allocate_block(&mut self) -> Result<WxBlockId, WxBuildError> {
        let id = WxBlockId(self.next_block);
        self.next_block = self
            .next_block
            .checked_add(1)
            .ok_or(WxBuildError::IdSpaceExhausted("block"))?;
        Ok(id)
    }
}

use super::*;

impl RegionBuilder<'_> {
    pub(super) fn create_overflow_exit(
        &mut self,
        pc: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxExitId, WxBuildError> {
        let exit = WxExitId(self.next_exit);
        self.next_exit = self
            .next_exit
            .checked_add(1)
            .ok_or(WxBuildError::IdSpaceExhausted("side-exit"))?;

        let mut registers: Vec<_> = environment.keys().copied().collect();
        registers.sort_unstable();
        let state = registers
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
        self.synthetic_exits.push(WxSideExit {
            id: exit,
            kind: WxExitKind::ReplayInstruction,
            resume_pc: pc,
            state,
        });
        Ok(exit)
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

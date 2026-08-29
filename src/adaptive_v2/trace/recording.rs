use super::*;

impl TraceRecorder {
    pub(crate) fn try_start(permit: RecordPermit, start: TraceStart) -> Result<Self, TraceError> {
        Self::with_limit(permit, start, DEFAULT_TRACE_LIMIT)
    }

    pub(crate) fn with_limit(
        permit: RecordPermit,
        start: TraceStart,
        limit: usize,
    ) -> Result<Self, TraceError> {
        let valid = match start.entry {
            EntryKind::FunctionEntry => start.start_pc == 0,
            EntryKind::LoopHeader { header_pc, .. } => start.start_pc == header_pc,
        };
        if !valid {
            return Err(TraceError::ArbitraryPc { pc: start.start_pc });
        }
        Ok(Self {
            start,
            schema_epoch: permit.schema_epoch(),
            limit,
            instructions: Vec::new(),
            terminated: false,
            registers: BTreeMap::new(),
            parameters: Vec::new(),
            next_value: 0,
        })
    }

    pub(crate) fn record(&mut self, event: TraceEvent) -> Result<(), TraceError> {
        if self.terminated {
            return Err(TraceError::Terminated);
        }
        if self.instructions.len() >= self.limit {
            return Err(TraceError::TraceLimit { limit: self.limit });
        }
        if event.effect.is_barrier() && event.safepoint.is_none() {
            return Err(TraceError::MissingSafepoint { pc: event.pc });
        }
        if matches!(
            event.op,
            TraceOp::Fact {
                lowering: FactLowering::ElidedProven
            }
        ) {
            return Ok(());
        }
        let kind = lower_op(event.pc, event.op.clone());
        let inputs = event
            .inputs
            .into_iter()
            .map(|register| {
                self.registers
                    .get(&register)
                    .copied()
                    .ok_or(TraceError::UndefinedRegister { register })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let output = event.output.map(|(register, ty)| {
            let value = ValueId::new(self.next_value);
            self.next_value = self.next_value.saturating_add(1);
            self.registers.insert(register, value);
            ValueDef::new(value, ty)
        });
        let sequence = u32::try_from(self.instructions.len()).unwrap_or(u32::MAX);
        let mut instruction = match event.safepoint {
            Some(point) => Instruction::safepoint(kind, inputs, output, event.effect, point),
            None => Instruction::new(kind, inputs, output, event.effect),
        };
        if event.effect.is_ordered() {
            instruction = instruction.ordered(sequence);
        }
        self.instructions.push(instruction);
        if matches!(event.op, TraceOp::NestedLoopHeader { .. }) {
            self.terminated = true;
        }
        Ok(())
    }

    pub(crate) fn finish(self, terminator: Terminator) -> Result<RecordedTrace, TraceError> {
        if let EntryKind::LoopHeader { header_pc, .. } = self.start.entry {
            match terminator {
                Terminator::Backedge { target_pc, .. } if target_pc == header_pc => {}
                Terminator::Backedge { target_pc, .. } => {
                    return Err(TraceError::MismatchedBackedge {
                        expected: header_pc,
                        actual: target_pc,
                    });
                }
                Terminator::IrreducibleBackedge => return Err(TraceError::IrreducibleBackedge),
                Terminator::Jump { .. }
                | Terminator::Branch { .. }
                | Terminator::Return { .. }
                | Terminator::SideExit { .. } => return Err(TraceError::MissingBackedge),
            }
        }
        Ok(RecordedTrace {
            executable: self.start.executable,
            entry: self.start.entry,
            schema_epoch: self.schema_epoch,
            instructions: self.instructions,
            parameters: self.parameters,
            terminator,
        })
    }
}

fn lower_op(pc: u32, op: TraceOp) -> InstructionKind {
    match op {
        TraceOp::Constant(value) => InstructionKind::Constant(value),
        TraceOp::Copy => InstructionKind::Copy,
        TraceOp::IntegerAdd => InstructionKind::IntegerAdd,
        TraceOp::IntegerSubtract => InstructionKind::IntegerSubtract,
        TraceOp::IntegerMultiply => InstructionKind::IntegerMultiply,
        TraceOp::IntegerCompare { comparison } => InstructionKind::IntegerCompare { comparison },
        TraceOp::FloatAdd => InstructionKind::FloatAdd,
        TraceOp::FloatSubtract => InstructionKind::FloatSubtract,
        TraceOp::FloatMultiply => InstructionKind::FloatMultiply,
        TraceOp::FloatDivide => InstructionKind::FloatDivide,
        TraceOp::FloatCompare { comparison } => InstructionKind::FloatCompare { comparison },
        TraceOp::IntegerNegate => InstructionKind::IntegerNegate,
        TraceOp::FloatNegate => InstructionKind::FloatNegate,
        TraceOp::BooleanNot => InstructionKind::BooleanNot,
        TraceOp::BooleanAnd => InstructionKind::BooleanAnd,
        TraceOp::BooleanOr => InstructionKind::BooleanOr,
        TraceOp::ObjectGet => InstructionKind::ObjectGet,
        TraceOp::ObjectSet => InstructionKind::ObjectSet,
        TraceOp::ListGet => InstructionKind::ListGet,
        TraceOp::ListSet => InstructionKind::ListSet,
        TraceOp::ListAppend => InstructionKind::ListAppend,
        TraceOp::Call { callee } => InstructionKind::Call { callee },
        TraceOp::Guard { guard } => InstructionKind::Guard { guard },
        TraceOp::Allocate => InstructionKind::Allocate,
        TraceOp::Helper { helper } => InstructionKind::Helper { helper },
        TraceOp::Branch { taken, side_exit } => InstructionKind::BranchGuard { taken, side_exit },
        TraceOp::NestedLoopHeader { header_pc } => InstructionKind::NestedLoopExit { header_pc },
        TraceOp::BorrowView => InstructionKind::BorrowView,
        TraceOp::ResolveHandle => InstructionKind::ResolveHandle,
        TraceOp::Fact { lowering } => match lowering {
            FactLowering::ElidedProven => InstructionKind::LiveProbe,
            FactLowering::GuardedStatic { guard } => InstructionKind::Guard { guard },
            FactLowering::LiveProbe => InstructionKind::LiveProbe,
        },
    }
    .at_pc(pc)
}

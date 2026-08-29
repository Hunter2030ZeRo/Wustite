use super::*;

mod integer;

impl RegionBuilder<'_> {
    pub(in crate::wxir::builder) fn emit_scalar_constant(
        &mut self,
        instructions: &mut Vec<WxInst>,
        scalar: WxScalarType,
        constant: WxConstant,
    ) -> Result<TypedValue, WxBuildError> {
        let id = self.allocate_value()?;
        let ty = WxType::Scalar(scalar);
        instructions.push(WxInst {
            results: vec![WxInstResult { id, ty }],
            kind: WxInstKind::Constant(constant),
        });
        Ok(TypedValue { id, ty })
    }

    pub(in crate::wxir::builder) fn emit_binary_value(
        &mut self,
        instructions: &mut Vec<WxInst>,
        ty: WxType,
        op: WxBinaryOp,
        lhs: WxValueId,
        rhs: WxValueId,
    ) -> Result<TypedValue, WxBuildError> {
        let id = self.allocate_value()?;
        instructions.push(WxInst {
            results: vec![WxInstResult { id, ty }],
            kind: WxInstKind::Binary { op, lhs, rhs },
        });
        Ok(TypedValue { id, ty })
    }

    pub(in crate::wxir::builder) fn emit_compare_value(
        &mut self,
        instructions: &mut Vec<WxInst>,
        op: WxCompareOp,
        lhs: WxValueId,
        rhs: WxValueId,
    ) -> Result<TypedValue, WxBuildError> {
        let id = self.allocate_value()?;
        let ty = WxType::Scalar(WxScalarType::I1);
        instructions.push(WxInst {
            results: vec![WxInstResult { id, ty }],
            kind: WxInstKind::Compare { op, lhs, rhs },
        });
        Ok(TypedValue { id, ty })
    }

    pub(super) fn emit_constant(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        dst: Register,
        scalar: WxScalarType,
        constant: super::super::ir::WxConstant,
    ) -> Result<(), WxBuildError> {
        let result = self.allocate_value()?;
        let ty = WxType::Scalar(scalar);
        instructions.push(WxInst {
            results: vec![WxInstResult { id: result, ty }],
            kind: WxInstKind::Constant(constant),
        });
        environment.insert(dst, TypedValue { id: result, ty });
        Ok(())
    }

    pub(super) fn operation_facts_allow(
        &self,
        pc: usize,
        site: OperationSiteId,
        expected: [SlotType; 3],
        guarded_inputs: bool,
    ) -> bool {
        let Some(facts) = self.executable.structure_map().operation_site(site) else {
            return false;
        };
        if facts.pc != pc {
            return false;
        }
        if guarded_inputs {
            return true;
        }
        let actual = [&facts.lhs, &facts.rhs, &facts.result];
        let mut needs_guard = false;
        for (fact, expected) in actual.into_iter().zip(expected) {
            match fact {
                Fact::Proven(actual) if *actual == expected => {}
                Fact::Guardable(actual) if *actual == expected => needs_guard = true,
                Fact::Proven(_) | Fact::Guardable(_) | Fact::Unknown => return false,
            }
        }
        !needs_guard || guarded_inputs
    }

    pub(super) fn emit_f64_binary(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        op: WxFloatBinaryOp,
        registers: [Register; 3],
    ) -> Result<(), WxBuildError> {
        let [dst, lhs, rhs] = registers;
        let lhs = self.read_f64_operand(instructions, environment, pc, lhs)?;
        let rhs = self.read_f64_operand(instructions, environment, pc, rhs)?;
        if op == WxFloatBinaryOp::Div {
            let zero = self.emit_scalar_constant(
                instructions,
                WxScalarType::F64,
                super::super::ir::WxConstant::F64(0.0),
            )?;
            let invalid = self.emit_compare_value(
                instructions,
                WxCompareOp::Float(WxFloatCompareOp::Eq),
                rhs.id,
                zero.id,
            )?;
            self.emit_replay_guard(instructions, environment, pc, invalid.id)?;
        }
        let result = self.allocate_value()?;
        let ty = WxType::Scalar(WxScalarType::F64);
        instructions.push(WxInst {
            results: vec![WxInstResult { id: result, ty }],
            kind: WxInstKind::Binary {
                op: WxBinaryOp::Float(op),
                lhs: lhs.id,
                rhs: rhs.id,
            },
        });
        environment.insert(dst, TypedValue { id: result, ty });
        Ok(())
    }

    pub(super) fn emit_f64_compare(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        op: WxFloatCompareOp,
        registers: [Register; 3],
    ) -> Result<(), WxBuildError> {
        let [dst, lhs, rhs] = registers;
        let lhs = self.read_f64_operand(instructions, environment, pc, lhs)?;
        let rhs = self.read_f64_operand(instructions, environment, pc, rhs)?;
        if !matches!(op, WxFloatCompareOp::Eq | WxFloatCompareOp::Ne) {
            let lhs_nan = self.emit_compare_value(
                instructions,
                WxCompareOp::Float(WxFloatCompareOp::Ne),
                lhs.id,
                lhs.id,
            )?;
            let rhs_nan = self.emit_compare_value(
                instructions,
                WxCompareOp::Float(WxFloatCompareOp::Ne),
                rhs.id,
                rhs.id,
            )?;
            let invalid = self.emit_binary_value(
                instructions,
                WxType::Scalar(WxScalarType::I1),
                WxBinaryOp::Integer(WxIntBinaryOp::Or),
                lhs_nan.id,
                rhs_nan.id,
            )?;
            self.emit_replay_guard(instructions, environment, pc, invalid.id)?;
        }
        let result = self.allocate_value()?;
        let ty = WxType::Scalar(WxScalarType::I1);
        instructions.push(WxInst {
            results: vec![WxInstResult { id: result, ty }],
            kind: WxInstKind::Compare {
                op: WxCompareOp::Float(op),
                lhs: lhs.id,
                rhs: rhs.id,
            },
        });
        environment.insert(dst, TypedValue { id: result, ty });
        Ok(())
    }

    pub(super) fn read_f64_operand(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &HashMap<Register, TypedValue>,
        pc: usize,
        register: Register,
    ) -> Result<TypedValue, WxBuildError> {
        let value = environment
            .get(&register)
            .copied()
            .ok_or(WxBuildError::MissingRegister { pc, register })?;
        match value.ty {
            WxType::Scalar(WxScalarType::F64) => Ok(value),
            WxType::Scalar(WxScalarType::I64) => {
                let result = self.allocate_value()?;
                let ty = WxType::Scalar(WxScalarType::F64);
                instructions.push(WxInst {
                    results: vec![WxInstResult { id: result, ty }],
                    kind: WxInstKind::Cast {
                        op: WxCastOp::IntToFloat { signed: true },
                        value: value.id,
                    },
                });
                Ok(TypedValue { id: result, ty })
            }
            actual => Err(WxBuildError::TypeMismatch {
                pc,
                register,
                expected: WxType::Scalar(WxScalarType::F64),
                actual,
            }),
        }
    }
}

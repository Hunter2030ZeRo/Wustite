use super::*;

impl RegionBuilder<'_> {
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

    pub(super) fn require_operation_facts(
        &self,
        pc: usize,
        site: OperationSiteId,
        lhs: TypeFact,
        rhs: TypeFact,
        result: TypeFact,
    ) -> Result<(), WxBuildError> {
        let facts = self
            .executable
            .structure_map()
            .operation_site(site)
            .ok_or_else(|| WxBuildError::UnsupportedSpecialization {
                pc,
                reason: format!("missing operation site {}", site.0),
            })?;
        if facts.pc != pc || facts.lhs != lhs || facts.rhs != rhs || facts.result != result {
            return Err(WxBuildError::UnsupportedSpecialization {
                pc,
                reason: format!(
                    "operation site {} lacks the exact facts required by the typed WXIR path",
                    site.0
                ),
            });
        }
        Ok(())
    }

    pub(super) fn emit_i64_add(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        dst: Register,
        lhs: Register,
        rhs: Register,
    ) -> Result<(), WxBuildError> {
        let lhs = self.read_register(environment, pc, lhs, WxScalarType::I64)?;
        let rhs = self.read_register(environment, pc, rhs, WxScalarType::I64)?;
        let result = self.allocate_value()?;
        let overflow = self.allocate_value()?;
        let ty = WxType::Scalar(WxScalarType::I64);
        instructions.push(WxInst {
            results: vec![
                WxInstResult { id: result, ty },
                WxInstResult {
                    id: overflow,
                    ty: WxType::Scalar(WxScalarType::I1),
                },
            ],
            kind: WxInstKind::IntegerBinaryWithOverflow {
                op: WxIntOverflowOp::Add,
                lhs: lhs.id,
                rhs: rhs.id,
            },
        });
        let exit = self.create_overflow_exit(pc, environment)?;
        instructions.push(WxInst {
            results: Vec::new(),
            kind: WxInstKind::Guard {
                condition: overflow,
                exit,
                mode: WxGuardMode::ExitWhenTrue,
            },
        });
        environment.insert(dst, TypedValue { id: result, ty });
        Ok(())
    }

    pub(super) fn emit_i64_lt(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        dst: Register,
        lhs: Register,
        rhs: Register,
    ) -> Result<(), WxBuildError> {
        let lhs = self.read_register(environment, pc, lhs, WxScalarType::I64)?;
        let rhs = self.read_register(environment, pc, rhs, WxScalarType::I64)?;
        let result = self.allocate_value()?;
        let ty = WxType::Scalar(WxScalarType::I1);
        instructions.push(WxInst {
            results: vec![WxInstResult { id: result, ty }],
            kind: WxInstKind::Compare {
                op: WxCompareOp::Integer(WxIntCompareOp::SignedLt),
                lhs: lhs.id,
                rhs: rhs.id,
            },
        });
        environment.insert(dst, TypedValue { id: result, ty });
        Ok(())
    }
}

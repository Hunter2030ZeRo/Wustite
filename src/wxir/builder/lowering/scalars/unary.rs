use super::super::super::*;

impl RegionBuilder<'_> {
    pub(in crate::wxir::builder) fn lower_unary_operation(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        instruction: &Instruction,
    ) -> Result<(), WxBuildError> {
        let Instruction::UnaryOp { dst, op, src } = instruction else {
            return Err(WxBuildError::UnsupportedInstruction {
                pc,
                instruction: "unary scalar lowering",
            });
        };
        match (op, environment.get(src).map(|value| value.ty)) {
            (UnaryOperator::Not, Some(WxType::Scalar(WxScalarType::I1))) => {
                self.emit_bool_not(instructions, environment, pc, *dst, *src)
            }
            (UnaryOperator::Negate, Some(WxType::Scalar(WxScalarType::I64))) => {
                self.emit_i64_negate(instructions, environment, pc, *dst, *src)
            }
            (UnaryOperator::Negate, Some(WxType::Scalar(WxScalarType::F64))) => {
                self.emit_f64_negate(instructions, environment, pc, *dst, *src)
            }
            (UnaryOperator::Negate | UnaryOperator::Not, _) => {
                self.emit_runtime_call(instructions, environment, pc, instruction)
            }
        }
    }

    pub(in crate::wxir::builder) fn lower_boolean_operation(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        instruction: &Instruction,
    ) -> Result<(), WxBuildError> {
        let Instruction::BooleanOp { dst, op, lhs, rhs } = instruction else {
            return Err(WxBuildError::UnsupportedInstruction {
                pc,
                instruction: "boolean scalar lowering",
            });
        };
        let boolean = WxType::Scalar(WxScalarType::I1);
        let direct = [*lhs, *rhs].into_iter().all(|register| {
            environment
                .get(&register)
                .is_some_and(|value| value.ty == boolean)
        });
        if direct {
            let op = match op {
                BooleanOperator::And => WxIntBinaryOp::And,
                BooleanOperator::Or => WxIntBinaryOp::Or,
            };
            self.emit_bool_binary(instructions, environment, op, (pc, [*dst, *lhs, *rhs]))
        } else {
            self.emit_runtime_call(instructions, environment, pc, instruction)
        }
    }
}

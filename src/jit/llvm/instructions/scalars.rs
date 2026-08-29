use super::*;

impl InstructionContext<'_, '_> {
    pub(super) fn lower_binary(
        &mut self,
        instruction: &WxInst,
        op: WxBinaryOp,
        lhs: WxValueId,
        rhs: WxValueId,
    ) -> Result<(), CompileError> {
        let result = one_result(instruction)?;
        let value = match (result.ty, op) {
            (WxType::Scalar(WxScalarType::I64), WxBinaryOp::Integer(WxIntBinaryOp::Add)) => self
                .builder
                .build_int_add(
                    int_value_for(self.values, lhs)?,
                    int_value_for(self.values, rhs)?,
                    "add",
                )
                .map(BasicValueEnum::from)
                .map_err(llvm_error)?,
            (WxType::Scalar(WxScalarType::I64), WxBinaryOp::Integer(op)) => {
                let lhs = int_value_for(self.values, lhs)?;
                let rhs = int_value_for(self.values, rhs)?;
                let value = match op {
                    WxIntBinaryOp::Sub => self.builder.build_int_sub(lhs, rhs, "sub"),
                    WxIntBinaryOp::Mul => self.builder.build_int_mul(lhs, rhs, "mul"),
                    WxIntBinaryOp::FloorDiv => {
                        let quotient = self
                            .builder
                            .build_int_signed_div(lhs, rhs, "floor_quotient")
                            .map_err(llvm_error)?;
                        let remainder = self
                            .builder
                            .build_int_signed_rem(lhs, rhs, "floor_remainder")
                            .map_err(llvm_error)?;
                        let zero = lhs.get_type().const_zero();
                        let has_remainder = self
                            .builder
                            .build_int_compare(IntPredicate::NE, remainder, zero, "has_remainder")
                            .map_err(llvm_error)?;
                        let lhs_negative = self
                            .builder
                            .build_int_compare(IntPredicate::SLT, lhs, zero, "lhs_negative")
                            .map_err(llvm_error)?;
                        let rhs_negative = self
                            .builder
                            .build_int_compare(IntPredicate::SLT, rhs, zero, "rhs_negative")
                            .map_err(llvm_error)?;
                        let signs_differ = self
                            .builder
                            .build_xor(lhs_negative, rhs_negative, "signs_differ")
                            .map_err(llvm_error)?;
                        let adjust = self
                            .builder
                            .build_and(has_remainder, signs_differ, "floor_adjust")
                            .map_err(llvm_error)?;
                        let minus_one = lhs.get_type().const_int(u64::MAX, true);
                        let correction = self
                            .builder
                            .build_select(adjust, minus_one, zero, "floor_correction")
                            .map_err(llvm_error)?
                            .into_int_value();
                        self.builder
                            .build_int_add(quotient, correction, "floor_div")
                    }
                    WxIntBinaryOp::Add
                    | WxIntBinaryOp::And
                    | WxIntBinaryOp::Or
                    | WxIntBinaryOp::Xor => {
                        return Err(CompileError::UnsupportedInstruction("Binary"));
                    }
                };
                value.map(BasicValueEnum::from).map_err(llvm_error)?
            }
            (WxType::Scalar(WxScalarType::I1), WxBinaryOp::Integer(WxIntBinaryOp::And)) => self
                .builder
                .build_and(
                    int_value_for(self.values, lhs)?,
                    int_value_for(self.values, rhs)?,
                    "and",
                )
                .map(BasicValueEnum::from)
                .map_err(llvm_error)?,
            (WxType::Scalar(WxScalarType::I1), WxBinaryOp::Integer(WxIntBinaryOp::Or)) => self
                .builder
                .build_or(
                    int_value_for(self.values, lhs)?,
                    int_value_for(self.values, rhs)?,
                    "or",
                )
                .map(BasicValueEnum::from)
                .map_err(llvm_error)?,
            (WxType::Scalar(WxScalarType::F64), WxBinaryOp::Float(op)) => {
                let lhs = float_value_for(self.values, lhs)?;
                let rhs = float_value_for(self.values, rhs)?;
                let value = match op {
                    WxFloatBinaryOp::Add => self.builder.build_float_add(lhs, rhs, "fadd"),
                    WxFloatBinaryOp::Sub => self.builder.build_float_sub(lhs, rhs, "fsub"),
                    WxFloatBinaryOp::Mul => self.builder.build_float_mul(lhs, rhs, "fmul"),
                    WxFloatBinaryOp::Div => self.builder.build_float_div(lhs, rhs, "fdiv"),
                };
                value.map(BasicValueEnum::from).map_err(llvm_error)?
            }
            (_, _) => return Err(CompileError::UnsupportedInstruction("Binary")),
        };
        self.values.insert(result.id, value);
        Ok(())
    }

    pub(super) fn lower_compare(
        &mut self,
        instruction: &WxInst,
        op: WxCompareOp,
        lhs: WxValueId,
        rhs: WxValueId,
    ) -> Result<(), CompileError> {
        let result = one_result(instruction)?;
        if result.ty != WxType::Scalar(WxScalarType::I1) {
            return Err(CompileError::UnsupportedType(result.ty));
        }
        let value = match op {
            WxCompareOp::Integer(WxIntCompareOp::Eq) => self
                .builder
                .build_int_compare(
                    IntPredicate::EQ,
                    int_value_for(self.values, lhs)?,
                    int_value_for(self.values, rhs)?,
                    "compare",
                )
                .map_err(llvm_error)?,
            WxCompareOp::Integer(WxIntCompareOp::SignedLt) => self
                .builder
                .build_int_compare(
                    IntPredicate::SLT,
                    int_value_for(self.values, lhs)?,
                    int_value_for(self.values, rhs)?,
                    "compare",
                )
                .map_err(llvm_error)?,
            WxCompareOp::Integer(WxIntCompareOp::Ne) => self
                .builder
                .build_int_compare(
                    IntPredicate::NE,
                    int_value_for(self.values, lhs)?,
                    int_value_for(self.values, rhs)?,
                    "compare",
                )
                .map_err(llvm_error)?,
            WxCompareOp::Integer(WxIntCompareOp::SignedLe) => self
                .builder
                .build_int_compare(
                    IntPredicate::SLE,
                    int_value_for(self.values, lhs)?,
                    int_value_for(self.values, rhs)?,
                    "compare",
                )
                .map_err(llvm_error)?,
            WxCompareOp::Float(op) => self
                .builder
                .build_float_compare(
                    float_predicate(op),
                    float_value_for(self.values, lhs)?,
                    float_value_for(self.values, rhs)?,
                    "fcompare",
                )
                .map_err(llvm_error)?,
            _ => return Err(CompileError::UnsupportedInstruction("Compare")),
        };
        self.values.insert(result.id, value.into());
        Ok(())
    }
}

impl InstructionContext<'_, '_> {
    pub(super) fn lower_cast(
        &mut self,
        instruction: &WxInst,
        op: WxCastOp,
        value: WxValueId,
    ) -> Result<(), CompileError> {
        let result = one_result(instruction)?;
        let value = match (op, result.ty) {
            (WxCastOp::IntToFloat { signed: true }, WxType::Scalar(WxScalarType::F64)) => self
                .builder
                .build_signed_int_to_float(
                    int_value_for(self.values, value)?,
                    self.module.get_context().f64_type(),
                    "int_to_float",
                )
                .map(BasicValueEnum::from)
                .map_err(llvm_error)?,
            _ => return Err(CompileError::UnsupportedInstruction("Cast")),
        };
        self.values.insert(result.id, value);
        Ok(())
    }
}

fn float_predicate(op: WxFloatCompareOp) -> FloatPredicate {
    match op {
        WxFloatCompareOp::Eq => FloatPredicate::OEQ,
        WxFloatCompareOp::Ne => FloatPredicate::UNE,
        WxFloatCompareOp::Lt => FloatPredicate::OLT,
        WxFloatCompareOp::Le => FloatPredicate::OLE,
        WxFloatCompareOp::Gt => FloatPredicate::OGT,
        WxFloatCompareOp::Ge => FloatPredicate::OGE,
    }
}

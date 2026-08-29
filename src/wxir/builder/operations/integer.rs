use super::super::*;

impl RegionBuilder<'_> {
    pub(in crate::wxir::builder) fn emit_i64_negate(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        dst: Register,
        src: Register,
    ) -> Result<(), WxBuildError> {
        let src = self.read_register(environment, pc, src, WxScalarType::I64)?;
        let zero =
            self.emit_scalar_constant(instructions, WxScalarType::I64, WxConstant::Int(0))?;
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
                op: WxIntOverflowOp::Sub,
                lhs: zero.id,
                rhs: src.id,
            },
        });
        self.emit_replay_guard(instructions, environment, pc, overflow)?;
        environment.insert(dst, TypedValue { id: result, ty });
        Ok(())
    }

    pub(in crate::wxir::builder) fn emit_bool_not(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        dst: Register,
        src: Register,
    ) -> Result<(), WxBuildError> {
        let src = self.read_register(environment, pc, src, WxScalarType::I1)?;
        let false_value =
            self.emit_scalar_constant(instructions, WxScalarType::I1, WxConstant::Bool(false))?;
        let result = self.emit_compare_value(
            instructions,
            WxCompareOp::Integer(WxIntCompareOp::Eq),
            src.id,
            false_value.id,
        )?;
        environment.insert(dst, result);
        Ok(())
    }

    pub(in crate::wxir::builder) fn emit_f64_negate(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        dst: Register,
        src: Register,
    ) -> Result<(), WxBuildError> {
        let src = self.read_register(environment, pc, src, WxScalarType::F64)?;
        let zero =
            self.emit_scalar_constant(instructions, WxScalarType::F64, WxConstant::F64(0.0))?;
        let result = self.emit_binary_value(
            instructions,
            WxType::Scalar(WxScalarType::F64),
            WxBinaryOp::Float(WxFloatBinaryOp::Sub),
            zero.id,
            src.id,
        )?;
        environment.insert(dst, result);
        Ok(())
    }

    pub(in crate::wxir::builder) fn emit_i64_checked(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        op: WxIntOverflowOp,
        operation: (usize, [Register; 3]),
    ) -> Result<(), WxBuildError> {
        let (pc, [dst, lhs, rhs]) = operation;
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
                op,
                lhs: lhs.id,
                rhs: rhs.id,
            },
        });
        self.emit_replay_guard(instructions, environment, pc, overflow)?;
        environment.insert(dst, TypedValue { id: result, ty });
        Ok(())
    }

    pub(in crate::wxir::builder) fn emit_i64_floor_div(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        operation: (usize, [Register; 3]),
    ) -> Result<(), WxBuildError> {
        let (pc, [dst, lhs, rhs]) = operation;
        let lhs = self.read_register(environment, pc, lhs, WxScalarType::I64)?;
        let rhs = self.read_register(environment, pc, rhs, WxScalarType::I64)?;
        let zero =
            self.emit_scalar_constant(instructions, WxScalarType::I64, WxConstant::Int(0))?;
        let minimum =
            self.emit_scalar_constant(instructions, WxScalarType::I64, WxConstant::Int(i64::MIN))?;
        let negative_one =
            self.emit_scalar_constant(instructions, WxScalarType::I64, WxConstant::Int(-1))?;
        let division_by_zero = self.emit_compare_value(
            instructions,
            WxCompareOp::Integer(WxIntCompareOp::Eq),
            rhs.id,
            zero.id,
        )?;
        let minimum_lhs = self.emit_compare_value(
            instructions,
            WxCompareOp::Integer(WxIntCompareOp::Eq),
            lhs.id,
            minimum.id,
        )?;
        let negative_rhs = self.emit_compare_value(
            instructions,
            WxCompareOp::Integer(WxIntCompareOp::Eq),
            rhs.id,
            negative_one.id,
        )?;
        let overflow = self.emit_binary_value(
            instructions,
            WxType::Scalar(WxScalarType::I1),
            WxBinaryOp::Integer(WxIntBinaryOp::And),
            minimum_lhs.id,
            negative_rhs.id,
        )?;
        let invalid = self.emit_binary_value(
            instructions,
            WxType::Scalar(WxScalarType::I1),
            WxBinaryOp::Integer(WxIntBinaryOp::Or),
            division_by_zero.id,
            overflow.id,
        )?;
        self.emit_replay_guard(instructions, environment, pc, invalid.id)?;
        let result = self.emit_binary_value(
            instructions,
            WxType::Scalar(WxScalarType::I64),
            WxBinaryOp::Integer(WxIntBinaryOp::FloorDiv),
            lhs.id,
            rhs.id,
        )?;
        environment.insert(dst, result);
        Ok(())
    }

    pub(in crate::wxir::builder) fn emit_i64_compare(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        op: WxIntCompareOp,
        operation: (usize, [Register; 3]),
    ) -> Result<(), WxBuildError> {
        let (pc, [dst, lhs, rhs]) = operation;
        let lhs = self.read_register(environment, pc, lhs, WxScalarType::I64)?;
        let rhs = self.read_register(environment, pc, rhs, WxScalarType::I64)?;
        let result =
            self.emit_compare_value(instructions, WxCompareOp::Integer(op), lhs.id, rhs.id)?;
        environment.insert(dst, result);
        Ok(())
    }

    pub(in crate::wxir::builder) fn emit_bool_binary(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        op: WxIntBinaryOp,
        operation: (usize, [Register; 3]),
    ) -> Result<(), WxBuildError> {
        let (pc, [dst, lhs, rhs]) = operation;
        let lhs = self.read_register(environment, pc, lhs, WxScalarType::I1)?;
        let rhs = self.read_register(environment, pc, rhs, WxScalarType::I1)?;
        let result = self.emit_binary_value(
            instructions,
            WxType::Scalar(WxScalarType::I1),
            WxBinaryOp::Integer(op),
            lhs.id,
            rhs.id,
        )?;
        environment.insert(dst, result);
        Ok(())
    }
}

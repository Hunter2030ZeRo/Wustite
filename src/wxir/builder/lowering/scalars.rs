use super::*;

mod unary;

impl RegionBuilder<'_> {
    pub(in crate::wxir::builder) fn lower_binary_operation(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        instruction: &Instruction,
    ) -> Result<(), WxBuildError> {
        let Instruction::BinaryOp {
            dst,
            op,
            lhs,
            rhs,
            site,
        } = instruction
        else {
            return Err(WxBuildError::UnsupportedInstruction {
                pc,
                instruction: "binary scalar lowering",
            });
        };
        let integer_inputs = operands_are(
            environment,
            [*lhs, *rhs],
            [
                WxType::Scalar(WxScalarType::I64),
                WxType::Scalar(WxScalarType::I64),
            ],
        );
        let integer = self.operation_facts_allow(
            pc,
            *site,
            [SlotType::SmallInt, SlotType::SmallInt, SlotType::SmallInt],
            integer_inputs,
        );
        let operation = (pc, [*dst, *lhs, *rhs]);
        match op {
            BinaryOperator::Add if integer => {
                self.emit_i64_checked(instructions, environment, WxIntOverflowOp::Add, operation)
            }
            BinaryOperator::Subtract if integer => {
                self.emit_i64_checked(instructions, environment, WxIntOverflowOp::Sub, operation)
            }
            BinaryOperator::Multiply if integer => {
                self.emit_i64_checked(instructions, environment, WxIntOverflowOp::Mul, operation)
            }
            BinaryOperator::FloorDivide if integer => {
                self.emit_i64_floor_div(instructions, environment, operation)
            }
            op if self.float_operation_allowed(pc, *site, environment, [*lhs, *rhs]) => {
                match float_binary(*op) {
                    Some(float_op) => self.emit_f64_binary(
                        instructions,
                        environment,
                        pc,
                        float_op,
                        [*dst, *lhs, *rhs],
                    ),
                    None => self.emit_runtime_call(instructions, environment, pc, instruction),
                }
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::FloorDivide
            | BinaryOperator::Power => {
                self.emit_runtime_call(instructions, environment, pc, instruction)
            }
        }
    }

    pub(in crate::wxir::builder) fn lower_compare_operation(
        &mut self,
        instructions: &mut Vec<WxInst>,
        environment: &mut HashMap<Register, TypedValue>,
        pc: usize,
        instruction: &Instruction,
    ) -> Result<(), WxBuildError> {
        let Instruction::CompareOp {
            dst,
            op,
            lhs,
            rhs,
            site,
        } = instruction
        else {
            return Err(WxBuildError::UnsupportedInstruction {
                pc,
                instruction: "compare scalar lowering",
            });
        };
        let integer_inputs = operands_are(
            environment,
            [*lhs, *rhs],
            [
                WxType::Scalar(WxScalarType::I64),
                WxType::Scalar(WxScalarType::I64),
            ],
        );
        let integer = self.operation_facts_allow(
            pc,
            *site,
            [SlotType::SmallInt, SlotType::SmallInt, SlotType::Bool],
            integer_inputs,
        );
        if integer {
            let (op, registers) = integer_compare(*op, [*dst, *lhs, *rhs]);
            self.emit_i64_compare(instructions, environment, op, (pc, registers))
        } else if self.float_compare_allowed(pc, *site, environment, [*lhs, *rhs]) {
            self.emit_f64_compare(
                instructions,
                environment,
                pc,
                float_compare(*op),
                [*dst, *lhs, *rhs],
            )
        } else {
            self.emit_runtime_call(instructions, environment, pc, instruction)
        }
    }

    fn float_operation_allowed(
        &self,
        pc: usize,
        site: OperationSiteId,
        environment: &HashMap<Register, TypedValue>,
        registers: [Register; 2],
    ) -> bool {
        self.numeric_float_facts_allow(pc, site, environment, registers, SlotType::Float)
    }

    fn float_compare_allowed(
        &self,
        pc: usize,
        site: OperationSiteId,
        environment: &HashMap<Register, TypedValue>,
        registers: [Register; 2],
    ) -> bool {
        self.operation_facts_allow(
            pc,
            site,
            [SlotType::Float, SlotType::Float, SlotType::Bool],
            operands_are(
                environment,
                registers,
                [
                    WxType::Scalar(WxScalarType::F64),
                    WxType::Scalar(WxScalarType::F64),
                ],
            ),
        )
    }

    fn numeric_float_facts_allow(
        &self,
        pc: usize,
        site: OperationSiteId,
        environment: &HashMap<Register, TypedValue>,
        registers: [Register; 2],
        result: SlotType,
    ) -> bool {
        [
            [SlotType::Float, SlotType::Float],
            [SlotType::SmallInt, SlotType::Float],
            [SlotType::Float, SlotType::SmallInt],
            [SlotType::SmallInt, SlotType::SmallInt],
        ]
        .into_iter()
        .any(|inputs| {
            let expected = inputs.map(slot_type);
            self.operation_facts_allow(
                pc,
                site,
                [inputs[0], inputs[1], result],
                operands_are(environment, registers, expected),
            )
        })
    }
}

fn operands_are(
    environment: &HashMap<Register, TypedValue>,
    registers: [Register; 2],
    expected: [WxType; 2],
) -> bool {
    registers
        .into_iter()
        .zip(expected)
        .all(|(register, expected)| {
            environment
                .get(&register)
                .is_some_and(|value| value.ty == expected)
        })
}

fn slot_type(ty: SlotType) -> WxType {
    match ty {
        SlotType::SmallInt => WxType::Scalar(WxScalarType::I64),
        SlotType::Float => WxType::Scalar(WxScalarType::F64),
        SlotType::Bool => WxType::Scalar(WxScalarType::I1),
        SlotType::Object(_) | SlotType::Any => WxType::Scalar(WxScalarType::RuntimeHandle),
    }
}

fn integer_compare(
    op: CompareOperator,
    [dst, lhs, rhs]: [Register; 3],
) -> (WxIntCompareOp, [Register; 3]) {
    match op {
        CompareOperator::Eq => (WxIntCompareOp::Eq, [dst, lhs, rhs]),
        CompareOperator::NotEq => (WxIntCompareOp::Ne, [dst, lhs, rhs]),
        CompareOperator::Lt => (WxIntCompareOp::SignedLt, [dst, lhs, rhs]),
        CompareOperator::Le => (WxIntCompareOp::SignedLe, [dst, lhs, rhs]),
        CompareOperator::Gt => (WxIntCompareOp::SignedLt, [dst, rhs, lhs]),
        CompareOperator::Ge => (WxIntCompareOp::SignedLe, [dst, rhs, lhs]),
    }
}

fn float_binary(op: BinaryOperator) -> Option<WxFloatBinaryOp> {
    match op {
        BinaryOperator::Add => Some(WxFloatBinaryOp::Add),
        BinaryOperator::Subtract => Some(WxFloatBinaryOp::Sub),
        BinaryOperator::Multiply => Some(WxFloatBinaryOp::Mul),
        BinaryOperator::Divide => Some(WxFloatBinaryOp::Div),
        BinaryOperator::FloorDivide | BinaryOperator::Power => None,
    }
}

fn float_compare(op: CompareOperator) -> WxFloatCompareOp {
    match op {
        CompareOperator::Eq => WxFloatCompareOp::Eq,
        CompareOperator::NotEq => WxFloatCompareOp::Ne,
        CompareOperator::Lt => WxFloatCompareOp::Lt,
        CompareOperator::Le => WxFloatCompareOp::Le,
        CompareOperator::Gt => WxFloatCompareOp::Gt,
        CompareOperator::Ge => WxFloatCompareOp::Ge,
    }
}

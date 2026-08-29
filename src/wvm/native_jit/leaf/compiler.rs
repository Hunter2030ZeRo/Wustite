use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use cranelift_codegen::ir::{
    AbiParam, InstBuilder, MemFlagsData, UserFuncName, condcodes::FloatCC, condcodes::IntCC, types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};

use super::{LeafOperation, LeafScalarType, NumericLeafPlan};
use crate::bytecode::{BinaryOperator, Register};

type NativeLeafEntry = unsafe extern "C" fn(*const u64, *mut u64) -> u8;

static NEXT_SYMBOL: AtomicU64 = AtomicU64::new(0);

pub(super) struct CompiledNumericLeaf {
    entry: NativeLeafEntry,
    _module: JITModule,
}

impl fmt::Debug for CompiledNumericLeaf {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompiledNumericLeaf")
            .field("entry", &(self.entry as usize))
            .finish_non_exhaustive()
    }
}

impl CompiledNumericLeaf {
    pub(super) fn compile(plan: &NumericLeafPlan) -> Result<Self, String> {
        if plan.operations.iter().any(|operation| {
            matches!(
                operation,
                LeafOperation::FloatBinary {
                    op: BinaryOperator::Power,
                    ..
                }
            )
        }) {
            return Err("native numeric leaf power is unsupported".to_string());
        }

        let mut module = JITModule::new(
            JITBuilder::new(default_libcall_names()).map_err(|error| error.to_string())?,
        );
        let pointer = module.target_config().pointer_type();
        let mut signature = module.make_signature();
        signature.params.push(AbiParam::new(pointer));
        signature.params.push(AbiParam::new(pointer));
        signature.returns.push(AbiParam::new(types::I8));
        let symbol = format!(
            "wustite_numeric_leaf_{}",
            NEXT_SYMBOL.fetch_add(1, Ordering::Relaxed)
        );
        let function_id = module
            .declare_function(&symbol, Linkage::Local, &signature)
            .map_err(|error| error.to_string())?;
        let mut context = module.make_context();
        context.func.signature = signature;
        context.func.name = UserFuncName::user(1, function_id.as_u32());
        let mut builder_context = FunctionBuilderContext::new();

        {
            let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let entry = builder.create_block();
            let failure = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let arguments = builder.block_params(entry)[0];
            let result = builder.block_params(entry)[1];
            let mut values = HashMap::new();
            for (index, (register, _)) in plan.parameters.iter().enumerate() {
                let offset = i32::try_from(index * size_of::<u64>())
                    .map_err(|_| "numeric leaf argument offset exceeds i32".to_string())?;
                let value = builder
                    .ins()
                    .load(types::I64, MemFlagsData::new(), arguments, offset);
                values.insert(*register, value);
            }

            let mut returned = false;
            for operation in &plan.operations {
                match *operation {
                    LeafOperation::Constant { dst, bits } => {
                        values.insert(dst, builder.ins().iconst(types::I64, bits as i64));
                    }
                    LeafOperation::IntegerBinary { dst, op, lhs, rhs } => {
                        let lhs = value(&values, lhs)?;
                        let rhs = value(&values, rhs)?;
                        let computed = match op {
                            BinaryOperator::Add => {
                                let (value, overflow) = builder.ins().sadd_overflow(lhs, rhs);
                                guard(&mut builder, overflow, failure);
                                value
                            }
                            BinaryOperator::Subtract => {
                                let (value, overflow) = builder.ins().ssub_overflow(lhs, rhs);
                                guard(&mut builder, overflow, failure);
                                value
                            }
                            BinaryOperator::Multiply => {
                                let (value, overflow) = builder.ins().smul_overflow(lhs, rhs);
                                guard(&mut builder, overflow, failure);
                                value
                            }
                            BinaryOperator::FloorDivide => {
                                let zero = builder.ins().icmp_imm_s(IntCC::Equal, rhs, 0);
                                guard(&mut builder, zero, failure);
                                let minimum = builder.ins().iconst(types::I64, i64::MIN);
                                let lhs_minimum = builder.ins().icmp(IntCC::Equal, lhs, minimum);
                                let rhs_negative_one =
                                    builder.ins().icmp_imm_s(IntCC::Equal, rhs, -1);
                                let overflow = builder.ins().band(lhs_minimum, rhs_negative_one);
                                guard(&mut builder, overflow, failure);
                                let quotient = builder.ins().sdiv(lhs, rhs);
                                let remainder = builder.ins().srem(lhs, rhs);
                                let has_remainder =
                                    builder.ins().icmp_imm_s(IntCC::NotEqual, remainder, 0);
                                let lhs_negative =
                                    builder.ins().icmp_imm_s(IntCC::SignedLessThan, lhs, 0);
                                let rhs_negative =
                                    builder.ins().icmp_imm_s(IntCC::SignedLessThan, rhs, 0);
                                let signs_differ = builder.ins().bxor(lhs_negative, rhs_negative);
                                let adjust = builder.ins().band(has_remainder, signs_differ);
                                let minus_one = builder.ins().iconst(types::I64, -1);
                                let zero = builder.ins().iconst(types::I64, 0);
                                let correction = builder.ins().select(adjust, minus_one, zero);
                                builder.ins().iadd(quotient, correction)
                            }
                            BinaryOperator::Divide | BinaryOperator::Power => {
                                return Err("invalid integer leaf operation".to_string());
                            }
                        };
                        values.insert(dst, computed);
                    }
                    LeafOperation::FloatBinary {
                        dst,
                        op,
                        lhs,
                        lhs_ty,
                        rhs,
                        rhs_ty,
                    } => {
                        let lhs = float_value(&mut builder, value(&values, lhs)?, lhs_ty);
                        let rhs = float_value(&mut builder, value(&values, rhs)?, rhs_ty);
                        if matches!(op, BinaryOperator::Divide | BinaryOperator::FloorDivide) {
                            let zero = builder.ins().f64const(0.0);
                            let division_by_zero = builder.ins().fcmp(FloatCC::Equal, rhs, zero);
                            guard(&mut builder, division_by_zero, failure);
                        }
                        let computed = match op {
                            BinaryOperator::Add => builder.ins().fadd(lhs, rhs),
                            BinaryOperator::Subtract => builder.ins().fsub(lhs, rhs),
                            BinaryOperator::Multiply => builder.ins().fmul(lhs, rhs),
                            BinaryOperator::Divide => builder.ins().fdiv(lhs, rhs),
                            BinaryOperator::FloorDivide => {
                                let quotient = builder.ins().fdiv(lhs, rhs);
                                builder.ins().floor(quotient)
                            }
                            BinaryOperator::Power => unreachable!(),
                        };
                        values.insert(dst, computed);
                    }
                    LeafOperation::Move { dst, src } => {
                        values.insert(dst, value(&values, src)?);
                    }
                    LeafOperation::Return { src, ty } => {
                        let value = value(&values, src)?;
                        let bits = match ty {
                            LeafScalarType::Integer | LeafScalarType::Bool => value,
                            LeafScalarType::Float => {
                                builder
                                    .ins()
                                    .bitcast(types::I64, MemFlagsData::new(), value)
                            }
                        };
                        builder.ins().store(MemFlagsData::new(), bits, result, 0);
                        let success = builder.ins().iconst(types::I8, 1);
                        builder.ins().return_(&[success]);
                        returned = true;
                    }
                }
            }
            if !returned {
                return Err("numeric leaf ended without Return".to_string());
            }
            builder.switch_to_block(failure);
            let failed = builder.ins().iconst(types::I8, 0);
            builder.ins().return_(&[failed]);
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }

        module
            .define_function(function_id, &mut context)
            .map_err(|error| format!("{error:#?}"))?;
        module.clear_context(&mut context);
        module
            .finalize_definitions()
            .map_err(|error| error.to_string())?;
        let code = module.get_finalized_function(function_id);
        // SAFETY: the function was declared and finalized with NativeLeafEntry's exact C ABI,
        // and the owning JITModule is retained in CompiledNumericLeaf.
        let entry = unsafe { std::mem::transmute::<*const u8, NativeLeafEntry>(code) };
        Ok(Self {
            entry,
            _module: module,
        })
    }

    pub(super) fn execute(&self, arguments: &[u64], result: &mut u64) -> bool {
        // SAFETY: the compiled entry only reads the fixed-signature argument slots and writes one
        // u64 result. Both buffers remain live for the duration of the call.
        unsafe { (self.entry)(arguments.as_ptr(), result) != 0 }
    }
}

fn value(
    values: &HashMap<Register, cranelift_codegen::ir::Value>,
    register: Register,
) -> Result<cranelift_codegen::ir::Value, String> {
    values
        .get(&register)
        .copied()
        .ok_or_else(|| format!("numeric leaf missing r{register}"))
}

fn float_value(
    builder: &mut FunctionBuilder<'_>,
    value: cranelift_codegen::ir::Value,
    ty: LeafScalarType,
) -> cranelift_codegen::ir::Value {
    match ty {
        LeafScalarType::Integer => builder.ins().fcvt_from_sint(types::F64, value),
        LeafScalarType::Float => builder
            .ins()
            .bitcast(types::F64, MemFlagsData::new(), value),
        LeafScalarType::Bool => unreachable!(),
    }
}

fn guard(
    builder: &mut FunctionBuilder<'_>,
    condition: cranelift_codegen::ir::Value,
    failure: cranelift_codegen::ir::Block,
) {
    let continuation = builder.create_block();
    builder
        .ins()
        .brif(condition, failure, &[], continuation, &[]);
    builder.switch_to_block(continuation);
}

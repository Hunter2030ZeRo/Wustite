use num_bigint::BigInt;
use wustite::bytecode::{BinaryOperator, CompareOperator, Function, Instruction};
use wustite::executable::{ConstantId, ExecutableConstant, ExecutableFunction};
use wustite::structure_map::{OperationSiteId, StructureMapBuilder, TypeFact};
use wustite::value::Value;
use wustite::wvm::Vm;

#[path = "numeric_semantics/cycles.rs"]
mod numeric_semantics_cycles;

fn executable(
    register_count: usize,
    code: Vec<Instruction>,
    constants: Vec<ExecutableConstant>,
) -> ExecutableFunction {
    let function = Function {
        code,
        register_count,
    };
    let mut builder = StructureMapBuilder::new();
    for (pc, instruction) in function.code.iter().enumerate() {
        if matches!(
            instruction,
            Instruction::BinaryOp { .. } | Instruction::CompareOp { .. }
        ) {
            builder
                .record_operation(pc, TypeFact::Unknown, TypeFact::Unknown, TypeFact::Unknown)
                .expect("operation site fixture should be representable");
        }
    }
    let structure_map = builder
        .finish(&function.code, function.register_count)
        .expect("structure map fixture should be representable");
    ExecutableFunction::new_with_abi(function, structure_map, Vec::new(), constants)
}

fn compare_to_float(
    op: CompareOperator,
    integer: Instruction,
    constants: Vec<ExecutableConstant>,
) -> Value {
    let function = executable(
        3,
        vec![
            integer,
            Instruction::ConstFloat {
                dst: 1,
                value: 9_007_199_254_740_992.0,
            },
            Instruction::CompareOp {
                dst: 2,
                op,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::Return { src: 2 },
        ],
        constants,
    );
    Vm::new().execute(&function).unwrap().value
}

#[test]
fn mixed_bigint_and_float_equality_is_exact_beyond_f64_integer_precision() {
    // Given: adjacent integers that collapse to the same f64 at 2^53.
    // When: the VM compares the exact BigInt to the rounded float.
    let value = compare_to_float(
        CompareOperator::Eq,
        Instruction::LoadConstant {
            dst: 0,
            constant: ConstantId(0),
        },
        vec![ExecutableConstant::BigInt(BigInt::from(
            9_007_199_254_740_993_u64,
        ))],
    );
    // Then: the numerically distinct values are not equal.
    assert_eq!(value, Value::Bool(false));
}

#[test]
fn mixed_bigint_and_float_ordering_is_exact_beyond_f64_integer_precision() {
    // Given: a BigInt one greater than an exactly represented f64.
    // When: the VM orders the BigInt against the float.
    let value = compare_to_float(
        CompareOperator::Gt,
        Instruction::LoadConstant {
            dst: 0,
            constant: ConstantId(0),
        },
        vec![ExecutableConstant::BigInt(BigInt::from(
            9_007_199_254_740_993_u64,
        ))],
    );
    // Then: the exact integer is greater.
    assert_eq!(value, Value::Bool(true));
}

#[test]
fn mixed_smallint_and_float_equality_is_exact_beyond_f64_integer_precision() {
    // Given: a SmallInt one greater than the rounded float at 2^53.
    // When: the VM compares them for equality.
    let value = compare_to_float(
        CompareOperator::Eq,
        Instruction::ConstSmallInt {
            dst: 0,
            value: 9_007_199_254_740_993_i64,
        },
        Vec::new(),
    );
    // Then: the numerically distinct values are not equal.
    assert_eq!(value, Value::Bool(false));
}

#[test]
fn nan_is_not_equal_to_itself() {
    // Given: two NaN float operands.
    // When: the VM compares them for equality.
    let value = compare_nan(CompareOperator::Eq).unwrap();
    // Then: IEEE equality remains false.
    assert_eq!(value, Value::Bool(false));
}

#[test]
fn nan_ordering_returns_a_controlled_error() {
    // Given: two NaN float operands.
    // When: the VM attempts to order them.
    let error = compare_nan(CompareOperator::Lt).unwrap_err();
    // Then: unordered values produce an explicit error.
    assert!(error.contains("NaN is not orderable"));
}

fn compare_nan(op: CompareOperator) -> Result<Value, String> {
    let function = executable(
        3,
        vec![
            Instruction::ConstFloat {
                dst: 0,
                value: f64::NAN,
            },
            Instruction::ConstFloat {
                dst: 1,
                value: f64::NAN,
            },
            Instruction::CompareOp {
                dst: 2,
                op,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::Return { src: 2 },
        ],
        Vec::new(),
    );
    Vm::new().execute(&function).map(|result| result.value)
}

#[test]
fn numerically_distinct_bigint_and_float_remain_distinct_dict_keys() {
    // Given: a BigInt and float that lossy conversion would conflate.
    let function = executable(
        5,
        vec![
            Instruction::LoadConstant {
                dst: 0,
                constant: ConstantId(0),
            },
            Instruction::ConstFloat {
                dst: 1,
                value: 9_007_199_254_740_992.0,
            },
            Instruction::ConstSmallInt { dst: 2, value: 10 },
            Instruction::ConstSmallInt { dst: 3, value: 20 },
            Instruction::BuildDict {
                dst: 4,
                entries: vec![(0, 2), (1, 3)],
            },
            Instruction::Length { dst: 0, object: 4 },
            Instruction::Return { src: 0 },
        ],
        vec![ExecutableConstant::BigInt(BigInt::from(
            9_007_199_254_740_993_u64,
        ))],
    );

    // When: both values are inserted through public dictionary bytecode.
    let value = Vm::new().execute(&function).unwrap().value;
    // Then: the dictionary retains two keys.
    assert_eq!(value, Value::SmallInt(2));
}

#[test]
fn huge_bigint_plus_float_returns_a_controlled_error() {
    // Given: a 400-digit BigInt that cannot become a finite f64.
    let function = binary_with_huge_bigint(BinaryOperator::Add, false);
    // When: the VM adds a float to it.
    let Err(error) = Vm::new().execute(&function) else {
        panic!("expected mixed arithmetic to fail");
    };
    // Then: execution reports conversion overflow instead of returning infinity.
    assert!(error.contains("BigInt cannot be represented as a finite float"));
}

#[test]
fn huge_bigint_divided_by_itself_is_one() {
    // Given: the same 400-digit BigInt as dividend and divisor.
    let function = binary_with_huge_bigint(BinaryOperator::Divide, true);
    // When: the VM divides the exact integers before float conversion.
    let value = Vm::new().execute(&function).unwrap().value;
    // Then: the finite ratio is preserved.
    assert_eq!(value, Value::Float(1.0));
}

#[test]
fn huge_bigint_divided_by_one_returns_a_controlled_error() {
    // Given: a 400-digit BigInt divided by one.
    let function = binary_with_huge_bigint(BinaryOperator::Divide, false);
    // When: the quotient cannot fit in a finite f64.
    let Err(error) = Vm::new().execute(&function) else {
        panic!("expected integer quotient conversion to fail");
    };
    // Then: execution reports overflow instead of returning infinity.
    assert!(error.contains("integer quotient cannot be represented as a finite float"));
}

fn binary_with_huge_bigint(op: BinaryOperator, rhs_is_bigint: bool) -> ExecutableFunction {
    let rhs = if rhs_is_bigint {
        Instruction::LoadConstant {
            dst: 1,
            constant: ConstantId(0),
        }
    } else if op == BinaryOperator::Divide {
        Instruction::ConstSmallInt { dst: 1, value: 1 }
    } else {
        Instruction::ConstFloat { dst: 1, value: 1.0 }
    };
    executable(
        3,
        vec![
            Instruction::LoadConstant {
                dst: 0,
                constant: ConstantId(0),
            },
            rhs,
            Instruction::BinaryOp {
                dst: 2,
                op,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::Return { src: 2 },
        ],
        vec![ExecutableConstant::BigInt(BigInt::from(10_u8).pow(399))],
    )
}

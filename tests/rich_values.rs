use wustite::bytecode::{
    BinaryOperator, BooleanOperator, CompareOperator, Function, Instruction, UnaryOperator,
};
use wustite::executable::{ConstantId, ExecutableConstant, ExecutableFunction};
use wustite::structure_map::{OperationSite, OperationSiteId, StructureMap, TypeFact};
use wustite::value::{Object, Value};
use wustite::wvm::Vm;

fn executable(
    register_count: usize,
    code: Vec<Instruction>,
    constants: Vec<ExecutableConstant>,
) -> ExecutableFunction {
    let operation_sites = code
        .iter()
        .enumerate()
        .filter_map(|(pc, instruction)| match instruction {
            Instruction::BinaryOp { .. } | Instruction::CompareOp { .. } => Some(OperationSite {
                pc,
                lhs: TypeFact::Unknown,
                rhs: TypeFact::Unknown,
                result: TypeFact::Unknown,
            }),
            _ => None,
        })
        .collect();

    ExecutableFunction::new_with_abi(
        Function {
            code,
            register_count,
        },
        StructureMap {
            regions: Vec::new(),
            operation_sites,
        },
        Vec::new(),
        constants,
    )
}

#[test]
fn float_arithmetic_and_negation_preserve_float_values() {
    // Given: float operands for every source-level arithmetic operation.
    let function = executable(
        9,
        vec![
            Instruction::ConstFloat { dst: 0, value: 9.0 },
            Instruction::ConstFloat { dst: 1, value: 3.0 },
            Instruction::ConstFloat { dst: 2, value: 2.0 },
            Instruction::ConstFloat { dst: 3, value: 5.0 },
            Instruction::BinaryOp {
                dst: 4,
                op: BinaryOperator::Divide,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::BinaryOp {
                dst: 5,
                op: BinaryOperator::Multiply,
                lhs: 4,
                rhs: 2,
                site: OperationSiteId(1),
            },
            Instruction::BinaryOp {
                dst: 6,
                op: BinaryOperator::Subtract,
                lhs: 5,
                rhs: 3,
                site: OperationSiteId(2),
            },
            Instruction::BinaryOp {
                dst: 7,
                op: BinaryOperator::Add,
                lhs: 6,
                rhs: 1,
                site: OperationSiteId(3),
            },
            Instruction::UnaryOp {
                dst: 8,
                op: UnaryOperator::Negate,
                src: 7,
            },
            Instruction::Return { src: 8 },
        ],
        Vec::new(),
    );

    // When: the VM executes division, multiplication, subtraction, addition, and negation.
    let value = Vm::new().execute(&function).unwrap().value;

    // Then: arithmetic remains in the float representation.
    assert_eq!(value, Value::Float(-4.0));
}

#[test]
fn boolean_and_comparison_operations_produce_boolean_values() {
    // Given: integer comparison operands and Boolean operands.
    let function = executable(
        17,
        vec![
            Instruction::ConstSmallInt { dst: 0, value: 3 },
            Instruction::ConstSmallInt { dst: 1, value: 5 },
            Instruction::ConstBool {
                dst: 2,
                value: true,
            },
            Instruction::ConstBool {
                dst: 3,
                value: false,
            },
            Instruction::CompareOp {
                dst: 4,
                op: CompareOperator::Eq,
                lhs: 0,
                rhs: 0,
                site: OperationSiteId(0),
            },
            Instruction::CompareOp {
                dst: 5,
                op: CompareOperator::NotEq,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(1),
            },
            Instruction::CompareOp {
                dst: 6,
                op: CompareOperator::Lt,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(2),
            },
            Instruction::CompareOp {
                dst: 7,
                op: CompareOperator::Le,
                lhs: 0,
                rhs: 0,
                site: OperationSiteId(3),
            },
            Instruction::CompareOp {
                dst: 8,
                op: CompareOperator::Gt,
                lhs: 1,
                rhs: 0,
                site: OperationSiteId(4),
            },
            Instruction::CompareOp {
                dst: 9,
                op: CompareOperator::Ge,
                lhs: 1,
                rhs: 1,
                site: OperationSiteId(5),
            },
            Instruction::UnaryOp {
                dst: 10,
                op: UnaryOperator::Not,
                src: 3,
            },
            Instruction::BooleanOp {
                dst: 11,
                op: BooleanOperator::And,
                lhs: 4,
                rhs: 5,
            },
            Instruction::BooleanOp {
                dst: 12,
                op: BooleanOperator::And,
                lhs: 6,
                rhs: 7,
            },
            Instruction::BooleanOp {
                dst: 13,
                op: BooleanOperator::And,
                lhs: 8,
                rhs: 9,
            },
            Instruction::BooleanOp {
                dst: 14,
                op: BooleanOperator::And,
                lhs: 11,
                rhs: 12,
            },
            Instruction::BooleanOp {
                dst: 15,
                op: BooleanOperator::And,
                lhs: 13,
                rhs: 10,
            },
            Instruction::BooleanOp {
                dst: 16,
                op: BooleanOperator::Or,
                lhs: 14,
                rhs: 15,
            },
            Instruction::Return { src: 16 },
        ],
        Vec::new(),
    );

    // When: the VM evaluates the complete comparison and Boolean operator set.
    let value = Vm::new().execute(&function).unwrap().value;

    // Then: the combined truth value is represented as a Boolean.
    assert_eq!(value, Value::Bool(true));
}

#[test]
fn string_constants_are_heap_objects_with_a_length() {
    // Given: a string constant in an executable constant pool.
    let function = executable(
        2,
        vec![
            Instruction::LoadConstant {
                dst: 0,
                constant: ConstantId(0),
            },
            Instruction::Length { dst: 1, object: 0 },
            Instruction::Return { src: 1 },
        ],
        vec![ExecutableConstant::String("wustite".into())],
    );
    let mut vm = Vm::new();

    // When: the VM loads the string constant and computes its length.
    let value = vm.execute(&function).unwrap().value;

    // Then: the object reports its character length as a SmallInt.
    assert_eq!(value, Value::SmallInt(7));
}

#[test]
fn tuple_construction_and_indexing_return_the_selected_item() {
    // Given: three SmallInt values and an index for a new tuple.
    let function = executable(
        6,
        vec![
            Instruction::ConstSmallInt { dst: 0, value: 10 },
            Instruction::ConstSmallInt { dst: 1, value: 20 },
            Instruction::ConstSmallInt { dst: 2, value: 30 },
            Instruction::ConstSmallInt { dst: 3, value: 1 },
            Instruction::BuildTuple {
                dst: 4,
                items: vec![0, 1, 2],
            },
            Instruction::GetItem {
                dst: 5,
                object: 4,
                key: 3,
            },
            Instruction::Return { src: 5 },
        ],
        Vec::new(),
    );

    // When: the VM constructs the tuple and reads its second item.
    let value = Vm::new().execute(&function).unwrap().value;

    // Then: indexing exposes the selected SmallInt value.
    assert_eq!(value, Value::SmallInt(20));
}

#[test]
fn list_mutation_and_length_are_visible_through_the_same_heap_object() {
    // Given: three SmallInt values for a new list and a replacement value.
    let function = executable(
        8,
        vec![
            Instruction::ConstSmallInt { dst: 0, value: 1 },
            Instruction::ConstSmallInt { dst: 1, value: 2 },
            Instruction::ConstSmallInt { dst: 2, value: 3 },
            Instruction::ConstSmallInt { dst: 3, value: 10 },
            Instruction::BuildList {
                dst: 4,
                items: vec![0, 1, 2],
            },
            Instruction::SetItem {
                object: 4,
                key: 1,
                value: 3,
            },
            Instruction::GetItem {
                dst: 5,
                object: 4,
                key: 1,
            },
            Instruction::Length { dst: 6, object: 4 },
            Instruction::BinaryOp {
                dst: 7,
                op: BinaryOperator::Add,
                lhs: 5,
                rhs: 6,
                site: OperationSiteId(0),
            },
            Instruction::Return { src: 7 },
        ],
        Vec::new(),
    );

    // When: the VM mutates an item, reads it, and measures the list.
    let value = Vm::new().execute(&function).unwrap().value;

    // Then: the replacement and list length both participate in the result.
    assert_eq!(value, Value::SmallInt(13));
}

#[test]
fn dict_mutation_indexing_and_length_share_one_dictionary() {
    // Given: string keys, two initial values, and a replacement dictionary value.
    let function = executable(
        9,
        vec![
            Instruction::LoadConstant {
                dst: 0,
                constant: ConstantId(0),
            },
            Instruction::LoadConstant {
                dst: 1,
                constant: ConstantId(1),
            },
            Instruction::ConstSmallInt { dst: 2, value: 10 },
            Instruction::ConstSmallInt { dst: 3, value: 20 },
            Instruction::ConstSmallInt { dst: 4, value: 11 },
            Instruction::BuildDict {
                dst: 5,
                entries: vec![(0, 2), (1, 3)],
            },
            Instruction::SetItem {
                object: 5,
                key: 0,
                value: 4,
            },
            Instruction::GetItem {
                dst: 6,
                object: 5,
                key: 0,
            },
            Instruction::Length { dst: 7, object: 5 },
            Instruction::BinaryOp {
                dst: 8,
                op: BinaryOperator::Add,
                lhs: 6,
                rhs: 7,
                site: OperationSiteId(0),
            },
            Instruction::Return { src: 8 },
        ],
        vec![
            ExecutableConstant::String("first".into()),
            ExecutableConstant::String("second".into()),
        ],
    );

    // When: the VM overwrites one key, reads another, and measures the dictionary.
    let value = Vm::new().execute(&function).unwrap().value;

    // Then: the replacement value and length report the two-entry dictionary after mutation.
    assert_eq!(value, Value::SmallInt(13));
}

#[test]
fn smallint_overflow_promotes_to_bigint_for_following_arithmetic() {
    // Given: a SmallInt addition that exceeds the i64 range.
    let function = executable(
        6,
        vec![
            Instruction::ConstSmallInt {
                dst: 0,
                value: i64::MAX,
            },
            Instruction::ConstSmallInt { dst: 1, value: 1 },
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Add,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::BinaryOp {
                dst: 3,
                op: BinaryOperator::Multiply,
                lhs: 2,
                rhs: 1,
                site: OperationSiteId(1),
            },
            Instruction::BinaryOp {
                dst: 4,
                op: BinaryOperator::Add,
                lhs: 3,
                rhs: 1,
                site: OperationSiteId(2),
            },
            Instruction::BinaryOp {
                dst: 5,
                op: BinaryOperator::Subtract,
                lhs: 4,
                rhs: 1,
                site: OperationSiteId(3),
            },
            Instruction::Return { src: 5 },
        ],
        Vec::new(),
    );
    let mut vm = Vm::new();

    // When: overflowed arithmetic is followed by more arithmetic instructions.
    let value = vm.execute(&function).unwrap().value;

    // Then: the result remains a heap-backed BigInt with the exact promoted value.
    let Value::Object(reference) = value else {
        panic!("expected a BigInt object reference");
    };
    assert!(
        matches!(vm.object(reference).unwrap(), Object::BigInt(value) if value.to_string() == "9223372036854775808")
    );
}

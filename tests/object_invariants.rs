use num_bigint::BigInt;
use wustite::bytecode::{Function, Instruction};
use wustite::executable::{ConstantId, ExecutableConstant, ExecutableFunction};
use wustite::object::{Object, ObjectError, ObjectHeap};
use wustite::structure_map::StructureMap;
use wustite::value::Value;
use wustite::wvm::Vm;

fn function(code: Vec<Instruction>, constants: Vec<ExecutableConstant>) -> ExecutableFunction {
    ExecutableFunction::new_with_abi(
        Function {
            register_count: 5,
            code,
        },
        StructureMap::default(),
        Vec::new(),
        constants,
    )
}

#[test]
fn containers_reject_foreign_stale_nested_refs() {
    // Given: a reference from another heap and a reused local slot.
    let mut owner = ObjectHeap::new();
    let foreign = owner.allocate(Object::String("foreign".into())).unwrap();
    let mut heap = ObjectHeap::new();
    let stale = heap.allocate(Object::String("stale".into())).unwrap();
    heap.remove(stale).unwrap();
    heap.allocate(Object::String("replacement".into())).unwrap();

    // When: containers attempt to retain those invalid nested references.
    let foreign_result = heap.allocate(Object::list(vec![Value::Object(foreign)]));
    let stale_result = heap.allocate(Object::tuple(vec![Value::Object(stale)]));

    // Then: allocation rejects the invalid handle at the heap boundary.
    assert!(matches!(foreign_result, Err(ObjectError::WrongHeap { .. })));
    assert!(matches!(
        stale_result,
        Err(ObjectError::StaleGeneration { .. })
    ));
}

#[test]
fn runtime_rejects_unnormalized_host_dicts() {
    // Given: a host-created dictionary with a duplicate key.
    let mut vm = Vm::new();
    let dictionary = Object::Dict(vec![
        (Value::SmallInt(1), Value::Bool(true)),
        (Value::SmallInt(1), Value::Bool(false)),
    ]);

    // When: the host asks the runtime to allocate it directly.
    let result = vm.allocate_object(dictionary);

    // Then: allocation preserves the one-entry-per-equivalent-key invariant.
    assert!(matches!(result, Err(ObjectError::DuplicateDictionaryKey)));
}

#[test]
fn host_dict_rejects_equivalent_string_object_keys() {
    // Given: distinct heap objects containing the same hashable string key.
    let mut heap = ObjectHeap::new();
    let first = heap.allocate(Object::String("same".into())).unwrap();
    let second = heap.allocate(Object::String("same".into())).unwrap();
    let dictionary = Object::Dict(vec![
        (Value::Object(first), Value::Bool(true)),
        (Value::Object(second), Value::Bool(false)),
    ]);

    // When: a host constructs a dictionary with the equivalent keys.
    let result = heap.allocate(dictionary);

    // Then: the heap rejects the duplicate even though the handles differ.
    assert!(matches!(result, Err(ObjectError::DuplicateDictionaryKey)));
}

#[test]
fn host_dict_handles_exact_mixed_numeric_keys() {
    // Given: one equivalent mixed numeric pair and one distinct large numeric pair.
    let mut heap = ObjectHeap::new();
    let equivalent = Object::Dict(vec![
        (Value::SmallInt(1), Value::Bool(true)),
        (Value::Float(1.0), Value::Bool(false)),
    ]);
    let bigint = heap
        .allocate(Object::BigInt(BigInt::from(9_007_199_254_740_993_i64)))
        .unwrap();
    let distinct = Object::Dict(vec![
        (Value::Object(bigint), Value::Bool(true)),
        (Value::Float(9_007_199_254_740_992.0), Value::Bool(false)),
    ]);

    // When: the host allocates both dictionaries through the lower heap boundary.
    let equivalent_result = heap.allocate(equivalent);
    let distinct_result = heap.allocate(distinct);

    // Then: exact equivalence is rejected and values beyond f64 precision remain distinct.
    assert!(matches!(
        equivalent_result,
        Err(ObjectError::DuplicateDictionaryKey)
    ));
    assert!(distinct_result.is_ok());
}

#[test]
fn containers_reject_uninitialized_values_unhashable_dict_keys() {
    // Given: direct host-created containers with invalid nested values.
    let mut heap = ObjectHeap::new();
    let uninitialized = Object::tuple(vec![Value::Uninitialized]);
    let unhashable_key = Object::Dict(vec![(
        Value::Object(heap.allocate(Object::list(Vec::new())).unwrap()),
        Value::Bool(true),
    )]);

    // When: the heap validates them at allocation.
    let uninitialized_result = heap.allocate(uninitialized);
    let unhashable_result = heap.allocate(unhashable_key);

    // Then: neither invalid graph can enter the heap.
    assert!(matches!(
        uninitialized_result,
        Err(ObjectError::UninitializedValue)
    ));
    assert!(matches!(
        unhashable_result,
        Err(ObjectError::UnhashableDictionaryKey)
    ));
}

#[test]
fn bigint_sequence_indices_support_negative_indexing_range_errors() {
    // Given: a list and a BigInt constant used as its index.
    let indexed = function(
        vec![
            Instruction::ConstSmallInt { dst: 0, value: 10 },
            Instruction::ConstSmallInt { dst: 1, value: 20 },
            Instruction::BuildList {
                dst: 2,
                items: vec![0, 1],
            },
            Instruction::LoadConstant {
                dst: 3,
                constant: ConstantId(0),
            },
            Instruction::GetItem {
                dst: 4,
                object: 2,
                key: 3,
            },
            Instruction::Return { src: 4 },
        ],
        vec![ExecutableConstant::BigInt(BigInt::from(-1))],
    );
    let oversized = function(
        vec![
            Instruction::ConstSmallInt { dst: 0, value: 10 },
            Instruction::BuildList {
                dst: 1,
                items: vec![0],
            },
            Instruction::LoadConstant {
                dst: 2,
                constant: ConstantId(0),
            },
            Instruction::GetItem {
                dst: 3,
                object: 1,
                key: 2,
            },
            Instruction::Return { src: 3 },
        ],
        vec![ExecutableConstant::BigInt(BigInt::from(i64::MAX) + 1)],
    );
    let mut vm = Vm::new();

    // When: the VM indexes with an in-range negative and an oversized BigInt.
    let indexed_result = vm.execute(&indexed);
    let oversized_result = vm.execute(&oversized);

    // Then: negative indexing follows Python and overflow is a controlled range error.
    assert_eq!(indexed_result.unwrap().value, Value::SmallInt(20));
    assert!(matches!(
        oversized_result,
        Err(error) if error == "sequence index out of range"
    ));
}

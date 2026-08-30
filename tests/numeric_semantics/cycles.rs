use super::*;

#[test]
fn distinct_self_referential_lists_compare_cycle_safe() {
    // Given: two distinct lists whose only element points to themselves.
    let function = cyclic_container_comparison(false);
    // When: the VM compares the cyclic structures for equality.
    let value = Vm::new().execute(&function).unwrap().value;
    // Then: bisimilar cycles compare equal without overflowing the stack.
    assert_eq!(value, Value::Bool(true));
}

#[test]
fn distinct_self_referential_dicts_compare_cycle_safe() {
    // Given: two distinct dictionaries whose value points to themselves.
    let function = cyclic_container_comparison(true);
    // When: the VM compares the cyclic structures for equality.
    let value = Vm::new().execute(&function).unwrap().value;
    // Then: bisimilar cycles compare equal without overflowing the stack.
    assert_eq!(value, Value::Bool(true));
}

fn cyclic_container_comparison(dict: bool) -> ExecutableFunction {
    let build_lhs = if dict {
        Instruction::BuildDict {
            dst: 2,
            entries: vec![(0, 1)],
        }
    } else {
        Instruction::BuildList {
            dst: 2,
            items: vec![1],
        }
    };
    let build_rhs = if dict {
        Instruction::BuildDict {
            dst: 3,
            entries: vec![(0, 1)],
        }
    } else {
        Instruction::BuildList {
            dst: 3,
            items: vec![1],
        }
    };
    executable(
        5,
        vec![
            Instruction::ConstSmallInt { dst: 0, value: 0 },
            Instruction::ConstBool {
                dst: 1,
                value: false,
            },
            build_lhs,
            build_rhs,
            Instruction::SetItem {
                object: 2,
                key: 0,
                value: 2,
            },
            Instruction::SetItem {
                object: 3,
                key: 0,
                value: 3,
            },
            Instruction::CompareOp {
                dst: 4,
                op: CompareOperator::Eq,
                lhs: 2,
                rhs: 3,
                site: OperationSiteId(0),
            },
            Instruction::Return { src: 4 },
        ],
        Vec::new(),
    )
}

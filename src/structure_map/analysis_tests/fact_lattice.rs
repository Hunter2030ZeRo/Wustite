use super::*;

#[test]
fn analysis_verification_rejects_dangling_value_refs() {
    // Given: a valid map whose instruction output is replaced with an unknown value id.
    let code = vec![Instruction::ConstSmallInt { dst: 0, value: 1 }];
    let mut map = StructureMapBuilder::new().finish(&code, 1).unwrap();
    map.instructions[0].output = Some(ValueId(u32::MAX));

    // When: the derived analysis is checked against its WVM bytecode.
    let error = map.verify_analysis(&code).unwrap_err();

    // Then: verifier diagnostics identify the dangling reference and its pc.
    assert!(error.contains("instruction fact at pc 0"));
    assert!(error.contains("unknown output value"));
}

#[test]
fn guardable_facts_distinct_from_proven_unknown_facts() {
    // Given: a comparison whose operand and result types require runtime guards.
    let mut builder = StructureMapBuilder::new();
    builder
        .record_parameter(0, 0, "lhs".to_string(), SlotType::SmallInt)
        .unwrap();
    builder
        .record_parameter(1, 1, "rhs".to_string(), SlotType::SmallInt)
        .unwrap();
    let site = builder
        .record_operation(
            0,
            TypeFact::Guardable(SlotType::SmallInt),
            TypeFact::Guardable(SlotType::SmallInt),
            TypeFact::Guardable(SlotType::Bool),
        )
        .unwrap();
    let code = vec![
        Instruction::CompareOp {
            dst: 2,
            op: CompareOperator::Lt,
            lhs: 0,
            rhs: 1,
            site,
        },
        Instruction::Branch {
            cond: 2,
            yes: 2,
            no: 4,
        },
        Instruction::ConstSmallInt { dst: 3, value: 1 },
        Instruction::Return { src: 3 },
        Instruction::Return { src: 0 },
    ];

    // When: WVM facts and control dependencies are derived.
    let map = builder.finish(&code, 4).unwrap();

    // Then: guarded candidates are never promoted to unconditional proofs.
    let result = map.instruction_fact(0).unwrap().output.unwrap();
    assert_eq!(
        map.value(result).unwrap().ty,
        TypeFact::Guardable(SlotType::Bool)
    );
    assert_eq!(
        map.instruction_fact(0).unwrap().failures,
        Fact::Guardable(Vec::new())
    );
    assert_eq!(
        map.instruction_fact(2).unwrap().control_dependencies[0].hoistable,
        Fact::Guardable(true)
    );
    assert!(Fact::<SlotType>::Unknown.candidate().is_none());
}

#[test]
fn branch_merge_multiple_reaching_definitions_unknown() {
    // Given: both branch arms assign different identities to the same register.
    let mut builder = StructureMapBuilder::new();
    builder
        .record_parameter(0, 0, "condition".to_string(), SlotType::Bool)
        .unwrap();
    let code = vec![
        Instruction::Branch {
            cond: 0,
            yes: 1,
            no: 3,
        },
        Instruction::ConstSmallInt { dst: 1, value: 10 },
        Instruction::Jump { target: 4 },
        Instruction::ConstSmallInt { dst: 1, value: 20 },
        Instruction::Return { src: 1 },
    ];

    // When: reaching definitions are joined at the return block.
    let map = builder.finish(&code, 2).unwrap();

    // Then: neither arm's identity is presented as a proven return value.
    assert_eq!(map.instruction_fact(4).unwrap().inputs[0].value, None);
}

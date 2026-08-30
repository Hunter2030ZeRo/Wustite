use super::*;
use crate::bytecode::{CompareOperator, Instruction};
use crate::object::ObjectKind;

#[test]
fn value_graph_records_provenance_composition_alias_effects_escape() {
    // Given: a parameter and literal used to build, alias, mutate, and return a list.
    let mut builder = StructureMapBuilder::new();
    builder
        .record_parameter(0, 0, "item".to_string(), SlotType::SmallInt)
        .unwrap();
    let code = vec![
        Instruction::ConstSmallInt { dst: 1, value: 7 },
        Instruction::BuildList {
            dst: 2,
            items: vec![0, 1],
        },
        Instruction::Move { dst: 3, src: 2 },
        Instruction::ListAppend { list: 3, value: 1 },
        Instruction::Return { src: 2 },
    ];

    // When: final WVM bytecode is analyzed.
    let map = builder.finish(&code, 4).unwrap();

    // Then: source identity, aggregate members, aliasing, mutation, and escape agree.
    let parameter = map
        .values()
        .iter()
        .find(|value| value.register == 0)
        .unwrap();
    assert_eq!(
        parameter.origin,
        Fact::Proven(ValueOrigin::Parameter {
            index: 0,
            name: "item".to_string(),
        })
    );
    let allocation = map.instruction_fact(1).unwrap().output.unwrap();
    let alias = map.instruction_fact(2).unwrap().output.unwrap();
    let allocation_fact = map.value(allocation).unwrap();
    assert_eq!(
        allocation_fact.origin,
        Fact::Proven(ValueOrigin::Allocation {
            pc: 1,
            kind: ObjectKind::List,
        })
    );
    assert_eq!(
        allocation_fact.composition,
        Fact::Proven(ValueComposition::Sequence(
            map.instruction_fact(1).unwrap().inputs.clone()
        ))
    );
    assert_eq!(
        allocation_fact.sequence.kind,
        Fact::Proven(SequenceKind::List)
    );
    assert_eq!(allocation_fact.sequence.exact_length, Fact::Proven(2));
    assert_eq!(
        allocation_fact.sequence.mutability,
        Fact::Proven(SequenceMutability::Mutable)
    );
    assert!(map.same_identity(allocation, alias));
    assert_eq!(allocation_fact.escape, Fact::Proven(EscapeState::Function));
    let mutation = map.instruction_fact(3).unwrap();
    assert!(mutation.effects.proven().unwrap().may_mutate);
    assert_eq!(mutation.mutated_values, Fact::Proven(vec![alias]));
    assert_eq!(
        mutation.mutations,
        Fact::Proven(vec![MutationEffect {
            identity_root: allocation,
            kind: MutationKind::Layout,
        }])
    );
    assert!(
        mutation
            .failures
            .proven()
            .unwrap()
            .contains(&FailureKind::Type)
    );
}

#[test]
fn nonescaping_allocs_guard_deps_optimization_facts() {
    // Given: a local tuple and a pure branch guard derived from a parameter.
    let mut builder = StructureMapBuilder::new();
    builder
        .record_parameter(0, 0, "limit".to_string(), SlotType::SmallInt)
        .unwrap();
    let site = builder
        .record_operation(
            3,
            TypeFact::Proven(SlotType::SmallInt),
            TypeFact::Proven(SlotType::SmallInt),
            TypeFact::Proven(SlotType::Bool),
        )
        .unwrap();
    let code = vec![
        Instruction::ConstSmallInt { dst: 1, value: 1 },
        Instruction::BuildTuple {
            dst: 2,
            items: vec![0, 1],
        },
        Instruction::Length { dst: 3, object: 2 },
        Instruction::CompareOp {
            dst: 4,
            op: CompareOperator::Lt,
            lhs: 0,
            rhs: 3,
            site,
        },
        Instruction::Branch {
            cond: 4,
            yes: 5,
            no: 7,
        },
        Instruction::ConstSmallInt { dst: 5, value: 10 },
        Instruction::Return { src: 5 },
        Instruction::ConstSmallInt { dst: 6, value: 20 },
        Instruction::Return { src: 6 },
    ];

    // When: the final CFG and value graph are built together.
    let map = builder.finish(&code, 7).unwrap();

    // Then: the tuple is virtualizable and each branch arm carries its guard dependency.
    let tuple = map.instruction_fact(1).unwrap().output.unwrap();
    assert!(map.value(tuple).unwrap().is_virtualizable());
    let yes = &map.instruction_fact(5).unwrap().control_dependencies;
    let no = &map.instruction_fact(7).unwrap().control_dependencies;
    assert_eq!(yes.len(), 1);
    assert_eq!(yes[0].branch_pc, 4);
    assert!(yes[0].expected);
    assert_eq!(yes[0].hoistable, Fact::Proven(true));
    assert_eq!(no.len(), 1);
    assert!(!no[0].expected);
}

#[test]
fn region_summary_aggregates_effect_escape_failure_guardable_facts() {
    // Given: a loop that allocates a list and passes it to an unknown call.
    let mut builder = StructureMapBuilder::new();
    builder
        .record_parameter(
            0,
            0,
            "callable".to_string(),
            SlotType::Object(ObjectKind::Function),
        )
        .unwrap();
    let region = builder.begin_region(0, vec![]);
    builder
        .finish_region(
            region,
            RegionKind::Loop { backedge: 2 },
            vec![RegionExit { target: 3 }],
        )
        .unwrap();
    let code = vec![
        Instruction::BuildList {
            dst: 1,
            items: vec![],
        },
        Instruction::Call {
            dst: 2,
            callable: 0,
            args: vec![1],
        },
        Instruction::Jump { target: 0 },
        Instruction::Return { src: 0 },
    ];

    // When: region-level optimization facts are summarized.
    let map = builder.finish(&code, 3).unwrap();

    // Then: the compiler sees an effectful call and an escaping, non-virtualizable allocation.
    let summary = map.region(region).unwrap().summary;
    assert!(summary.effects.proven().unwrap().may_call_unknown);
    assert_eq!(summary.escaping_allocation_count, 1);
    assert_eq!(summary.virtualizable_allocation_count, 0);
    assert_eq!(summary.failure_site_count, 2);
}

mod fact_lattice;

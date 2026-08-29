use wustite::bytecode::Instruction;
use wustite::object::{Object, ObjectHeap, SequenceStrategy};
use wustite::profiler::{Profile, SequenceLayoutCase, SequenceSpecialization};
use wustite::structure_map::{RegionId, SlotType, StateSlot};
use wustite::value::Value;

#[test]
fn sequence_sites_form_a_two_case_guardable_profile() {
    // Given: one length site and three differently specialized lists.
    let mut heap = ObjectHeap::new();
    let integer = heap
        .allocate(Object::list(vec![Value::SmallInt(1)]))
        .unwrap();
    let float = heap
        .allocate(Object::list(vec![Value::Float(1.0)]))
        .unwrap();
    let boolean = heap
        .allocate(Object::list(vec![Value::Bool(true)]))
        .unwrap();
    let instruction = Instruction::Length { dst: 1, object: 0 };
    let mut profile = Profile::new(0, 1);

    // When: two layouts are observed at the same access site.
    profile.observe_instruction(
        0,
        &instruction,
        &[Value::Object(integer), Value::SmallInt(1)],
        &heap,
    );
    profile.observe_instruction(
        0,
        &instruction,
        &[Value::Object(float), Value::SmallInt(1)],
        &heap,
    );

    // Then: both cases remain guardable until a third layout makes the site megamorphic.
    assert_eq!(
        profile.sequence_specialization(0),
        SequenceSpecialization::Bimorphic([
            SequenceLayoutCase::list(SequenceStrategy::I64),
            SequenceLayoutCase::list(SequenceStrategy::F64),
        ])
    );
    profile.observe_instruction(
        0,
        &instruction,
        &[Value::Object(boolean), Value::SmallInt(1)],
        &heap,
    );
    assert_eq!(
        profile.sequence_specialization(0),
        SequenceSpecialization::Megamorphic
    );
}

#[test]
fn region_entry_records_live_sequence_layouts_before_the_first_access() {
    // Given: a typed list already live when a region is first entered.
    let mut heap = ObjectHeap::new();
    let list = heap
        .allocate(Object::list(vec![Value::Float(1.0)]))
        .unwrap();
    let slots = [StateSlot {
        register: 0,
        ty: SlotType::Object(wustite::object::ObjectKind::List),
    }];
    let mut profile = Profile::new(1, 0);

    // When: entry profiling observes the region state before WXIR compilation.
    profile.observe_entry_sequences(RegionId(0), &slots, &[Value::Object(list)], &heap);

    // Then: lowering can guard on the actual typed storage immediately.
    assert_eq!(
        profile.entry_sequence_specialization(RegionId(0), 0),
        SequenceSpecialization::Monomorphic(SequenceLayoutCase::list(SequenceStrategy::F64))
    );
}

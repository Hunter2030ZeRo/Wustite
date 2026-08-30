use crate::bytecode::Instruction;
use crate::structure_map::{
    RegionExit, RegionId, RegionKind, SlotType, StateSlot, StructureMapBuilder,
};
use crate::value::Value;

use super::{Profile, ValueTag};

#[test]
fn site_result_exact_for_shared_tag() {
    // Given
    let mut profile = Profile::new(1, 1);
    // When
    profile.observe_result(0, Value::Float(1.0));
    profile.observe_result(0, Value::Float(2.0));
    // Then
    assert_eq!(profile.result_tag(0), Some(ValueTag::Float));
}

#[test]
fn site_result_generic_for_divergent_tags() {
    // Given
    let mut profile = Profile::new(1, 1);
    // When
    profile.observe_result(0, Value::Float(1.0));
    profile.observe_result(0, Value::SmallInt(1));
    // Then
    assert_eq!(profile.result_tag(0), None);
}

#[test]
fn region_entry_tag_records_live_value_class() {
    // Given
    let mut profile = Profile::new(1, 0);
    let slots = [StateSlot {
        register: 0,
        ty: SlotType::Any,
    }];
    // When
    profile.observe_entry(RegionId(0), &slots, &[Value::Float(3.5)]);
    // Then
    assert_eq!(profile.entry_tag(RegionId(0), 0), Some(ValueTag::Float));
}

#[test]
fn region_entry_prefers_stable_loop_type() {
    // Given
    let mut profile = Profile::new(1, 0);
    let slots = [StateSlot {
        register: 0,
        ty: SlotType::SmallInt,
    }];
    // When
    profile.observe_entry(RegionId(0), &slots, &[Value::SmallInt(0)]);
    profile.observe_entry(RegionId(0), &slots, &[Value::Float(1.0)]);
    profile.observe_entry(RegionId(0), &slots, &[Value::Float(2.0)]);
    // Then
    assert_eq!(profile.entry_tag(RegionId(0), 0), Some(ValueTag::Float));
}

#[test]
fn region_profile_requires_eight_compat_runtime_entries() {
    let slots = vec![StateSlot {
        register: 0,
        ty: SlotType::Any,
    }];
    let mut builder = StructureMapBuilder::new();
    let region = builder.begin_region(0, slots.clone());
    builder
        .finish_region(
            region,
            RegionKind::Loop { backedge: 1 },
            vec![RegionExit { target: 2 }],
        )
        .unwrap();
    let map = builder
        .finish(
            &[
                Instruction::Move { dst: 0, src: 0 },
                Instruction::Jump { target: 0 },
                Instruction::Return { src: 0 },
            ],
            1,
        )
        .unwrap();
    let mut profile = Profile::new(1, 3);

    for _ in 0..7 {
        profile.record_entry(region);
        profile.observe_entry(region, &slots, &[Value::SmallInt(1)]);
    }
    assert!(profile.ready_region(&map, region, 3).is_none());

    profile.record_entry(region);
    profile.observe_entry(region, &slots, &[Value::SmallInt(1)]);
    assert!(profile.ready_region(&map, region, 3).is_some());
    assert!(profile.ready_region(&map, region, u64::MAX).is_none());
}

#[test]
fn incompat_entry_resets_profile_ready_window() {
    let slots = vec![StateSlot {
        register: 0,
        ty: SlotType::Any,
    }];
    let mut builder = StructureMapBuilder::new();
    let region = builder.begin_region(0, slots.clone());
    builder
        .finish_region(
            region,
            RegionKind::Loop { backedge: 1 },
            vec![RegionExit { target: 2 }],
        )
        .unwrap();
    let map = builder
        .finish(
            &[
                Instruction::Move { dst: 0, src: 0 },
                Instruction::Jump { target: 0 },
                Instruction::Return { src: 0 },
            ],
            1,
        )
        .unwrap();
    let mut profile = Profile::new(1, 3);

    for _ in 0..8 {
        profile.record_entry(region);
        profile.observe_entry(region, &slots, &[Value::SmallInt(1)]);
    }
    profile.observe_entry(region, &slots, &[Value::Float(1.0)]);
    assert!(profile.ready_region(&map, region, 3).is_none());
}

#[test]
fn cached_candidates_keep_live_validation() {
    let slots = vec![StateSlot {
        register: 0,
        ty: SlotType::Any,
    }];
    let mut builder = StructureMapBuilder::new();
    let region = builder.begin_region(0, slots.clone());
    builder
        .finish_region(
            region,
            RegionKind::Loop { backedge: 1 },
            vec![RegionExit { target: 2 }],
        )
        .unwrap();
    let map = builder
        .finish(
            &[
                Instruction::Move { dst: 0, src: 0 },
                Instruction::Jump { target: 0 },
                Instruction::Return { src: 0 },
            ],
            1,
        )
        .unwrap();
    let mut previous = Profile::new(1, 3);
    previous.observe_entry(region, &slots, &[Value::SmallInt(1)]);
    previous.observe_entry(region, &slots, &[Value::SmallInt(1)]);
    let artifact = previous.artifact("exact".to_string());
    let mut current = Profile::new(1, 3);
    current.seed_from_artifact(&artifact, "exact").unwrap();

    for _ in 0..7 {
        current.record_entry(region);
        current.observe_entry(region, &slots, &[Value::SmallInt(1)]);
    }
    assert!(current.ready_region(&map, region, 1).is_none());

    current.record_entry(region);
    current.observe_entry(region, &slots, &[Value::SmallInt(1)]);
    assert!(current.ready_region(&map, region, 1).is_some());
}

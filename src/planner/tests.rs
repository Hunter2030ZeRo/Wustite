use crate::bytecode::Instruction;
use crate::profiler::Profile;
use crate::structure_map::{RegionExit, RegionKind, StructureMapBuilder};

use super::{JitPolicy, RegionPlanRequest, plan_region, select_hot_loop};

#[test]
fn hot_loops_prefer_low_risk_proof() {
    // Given: equally hot pure and unknown-call loops in one finalized StructureMap.
    let mut builder = StructureMapBuilder::new();
    let pure = builder.begin_region(0, vec![]);
    builder
        .finish_region(
            pure,
            RegionKind::Loop { backedge: 1 },
            vec![RegionExit { target: 4 }],
        )
        .unwrap();
    let effectful = builder.begin_region(2, vec![]);
    builder
        .finish_region(
            effectful,
            RegionKind::Loop { backedge: 3 },
            vec![RegionExit { target: 4 }],
        )
        .unwrap();
    let map = builder
        .finish(
            &[
                Instruction::ConstSmallInt { dst: 0, value: 1 },
                Instruction::Jump { target: 0 },
                Instruction::Call {
                    dst: 1,
                    callable: 0,
                    args: vec![],
                },
                Instruction::Jump { target: 2 },
                Instruction::Return { src: 0 },
            ],
            2,
        )
        .unwrap();
    let mut profile = Profile::new(2, 5);
    for _ in 0..3 {
        profile.record_entry(pure);
        profile.record_entry(effectful);
    }

    // When: the planner selects one eligible loop.
    let selected = select_hot_loop(&map, &profile, 3).unwrap();

    // Then: equal runtime heat is resolved with static effect/failure risk.
    assert_eq!(selected.region_id, pure);
}

#[test]
fn both_runtime_policies_require_hot_ready_profiles() {
    let mut builder = StructureMapBuilder::new();
    let region = builder.begin_region(0, vec![]);
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
                Instruction::ConstSmallInt { dst: 0, value: 1 },
                Instruction::Jump { target: 0 },
                Instruction::Return { src: 0 },
            ],
            1,
        )
        .unwrap();
    let mut profile = Profile::new(1, 3);

    for _ in 0..7 {
        profile.record_entry(region);
    }
    assert!(profile.ready_region(&map, region, 3).is_none());

    profile.record_entry(region);
    let ready = profile.ready_region(&map, region, 3).unwrap();
    for policy in [JitPolicy::Profile, JitPolicy::StructureMap] {
        assert!(
            plan_region(
                &map,
                ready,
                RegionPlanRequest {
                    policy,
                    threshold: 3,
                    region_id: region,
                },
            )
            .is_some()
        );
        assert!(profile.ready_region(&map, region, u64::MAX).is_none());
    }
}

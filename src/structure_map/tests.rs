use super::*;
use crate::bytecode::{BinaryOperator, Instruction};

fn slot(register: Register) -> StateSlot {
    StateSlot {
        register,
        ty: SlotType::SmallInt,
    }
}

#[test]
fn finish_builds_exact_cfg_loop_summary_from_final_bytecode() {
    // Given: a loop whose patched branch targets create five blocks.
    let mut builder = StructureMapBuilder::new();
    let operation = builder
        .record_operation(
            2,
            TypeFact::Proven(SlotType::SmallInt),
            TypeFact::Proven(SlotType::SmallInt),
            TypeFact::Proven(SlotType::SmallInt),
        )
        .unwrap();
    let region = builder.begin_region(1, vec![slot(0)]);
    builder
        .finish_region(
            region,
            RegionKind::Loop { backedge: 5 },
            vec![RegionExit { target: 6 }],
        )
        .unwrap();
    let code = vec![
        Instruction::ConstSmallInt { dst: 0, value: 1 },
        Instruction::Branch {
            cond: 0,
            yes: 2,
            no: 6,
        },
        Instruction::BinaryOp {
            dst: 0,
            op: BinaryOperator::Add,
            lhs: 0,
            rhs: 0,
            site: operation,
        },
        Instruction::Call {
            dst: 1,
            callable: 0,
            args: vec![],
        },
        Instruction::Branch {
            cond: 0,
            yes: 5,
            no: 6,
        },
        Instruction::Jump { target: 1 },
        Instruction::Return { src: 1 },
    ];

    // When: the final, patched bytecode is scanned.
    let map = builder.finish(&code, 2).unwrap();

    // Then: block bounds, edges, predecessors, membership, and summary are exact.
    assert_eq!(
        map.blocks(),
        &[
            BasicBlock {
                id: BlockId(0),
                start_pc: 0,
                end_pc: 1,
                successors: vec![BlockEdge {
                    target: BlockId(1),
                    kind: EdgeKind::Fallthrough,
                }],
                predecessors: vec![],
            },
            BasicBlock {
                id: BlockId(1),
                start_pc: 1,
                end_pc: 2,
                successors: vec![
                    BlockEdge {
                        target: BlockId(2),
                        kind: EdgeKind::BranchTrue,
                    },
                    BlockEdge {
                        target: BlockId(4),
                        kind: EdgeKind::BranchFalse,
                    },
                ],
                predecessors: vec![BlockId(0), BlockId(3)],
            },
            BasicBlock {
                id: BlockId(2),
                start_pc: 2,
                end_pc: 5,
                successors: vec![
                    BlockEdge {
                        target: BlockId(3),
                        kind: EdgeKind::BranchTrue,
                    },
                    BlockEdge {
                        target: BlockId(4),
                        kind: EdgeKind::BranchFalse,
                    },
                ],
                predecessors: vec![BlockId(1)],
            },
            BasicBlock {
                id: BlockId(3),
                start_pc: 5,
                end_pc: 6,
                successors: vec![BlockEdge {
                    target: BlockId(1),
                    kind: EdgeKind::Jump,
                }],
                predecessors: vec![BlockId(2)],
            },
            BasicBlock {
                id: BlockId(4),
                start_pc: 6,
                end_pc: 7,
                successors: vec![],
                predecessors: vec![BlockId(1), BlockId(2)],
            },
        ]
    );
    assert_eq!(map.block_by_pc(6), map.block(BlockId(4)));
    let region = map.region(region).unwrap();
    assert_eq!(region.blocks, vec![BlockId(1), BlockId(2), BlockId(3)]);
    assert_eq!(
        region.summary,
        RegionSummary {
            instruction_count: 5,
            block_count: 3,
            operation_count: 1,
            call_count: 1,
            effects: Fact::Proven(EffectSummary {
                may_mutate: true,
                may_allocate: true,
                may_call_unknown: true,
                may_access_global_state: true,
            }),
            failure_site_count: 1,
            ..RegionSummary::default()
        }
    );
}

#[test]
fn ids_follow_record_order_despite_finish_order() {
    // Given: two operation sites and two unfinished regions.
    let mut builder = StructureMapBuilder::new();
    let first_operation = builder
        .record_operation(0, TypeFact::Unknown, TypeFact::Unknown, TypeFact::Unknown)
        .unwrap();
    let second_operation = builder
        .record_operation(1, TypeFact::Unknown, TypeFact::Unknown, TypeFact::Unknown)
        .unwrap();
    let first_region = builder.begin_region(0, vec![]);
    let second_region = builder.begin_region(1, vec![]);

    // When: regions are completed in reverse order.
    builder
        .finish_region(second_region, RegionKind::Branch, vec![])
        .unwrap();
    builder
        .finish_region(first_region, RegionKind::Branch, vec![])
        .unwrap();
    let map = builder
        .finish(
            &[
                Instruction::Jump { target: 1 },
                Instruction::Return { src: 0 },
            ],
            1,
        )
        .unwrap();

    // Then: IDs and accessors retain record/begin order.
    assert_eq!(first_operation, OperationSiteId(0));
    assert_eq!(second_operation, OperationSiteId(1));
    assert_eq!(first_region, RegionId(0));
    assert_eq!(second_region, RegionId(1));
    assert_eq!(map.operation_sites().len(), 2);
    assert_eq!(map.regions().len(), 2);
    assert_eq!(map.region_by_entry_pc(1), Some(second_region));
}

#[test]
fn finish_region_rejects_unknown_double_finish() {
    // Given: one region that has already been finished.
    let mut builder = StructureMapBuilder::new();
    let region = builder.begin_region(0, vec![]);
    builder
        .finish_region(region, RegionKind::Branch, vec![])
        .unwrap();

    // When: completion is repeated or uses an unknown ID.
    let duplicate = builder.finish_region(region, RegionKind::Branch, vec![]);
    let unknown = builder.finish_region(RegionId(99), RegionKind::Branch, vec![]);

    // Then: both impossible transitions are rejected.
    assert!(duplicate.is_err());
    assert!(unknown.is_err());
}

#[test]
fn finish_rejects_unfinished_regions() {
    // Given: a builder with an open region.
    let mut builder = StructureMapBuilder::new();
    builder.begin_region(0, vec![]);

    // When: finalization is attempted.
    let result = builder.finish(&[Instruction::Return { src: 0 }], 1);

    // Then: the incomplete region is rejected.
    assert!(result.is_err());
}

#[test]
fn finish_rejects_out_range_cfg_targets() {
    // Given: final bytecode with a jump beyond its instruction range.
    let builder = StructureMapBuilder::new();

    // When: finalization scans the invalid target.
    let result = builder.finish(&[Instruction::Jump { target: 1 }], 0);

    // Then: impossible CFG construction is rejected.
    assert!(result.is_err());
}

#[test]
fn finish_keeps_out_range_op_site_verifier() {
    // Given: semantic operation metadata whose pc is outside bytecode.
    let mut builder = StructureMapBuilder::new();
    let site = builder
        .record_operation(1, TypeFact::Unknown, TypeFact::Unknown, TypeFact::Unknown)
        .unwrap();

    // When: the structurally valid bytecode is finalized.
    let map = builder
        .finish(&[Instruction::Return { src: 0 }], 1)
        .unwrap();

    // Then: the verifier-facing invalid metadata remains unchanged.
    assert_eq!(map.operation_site(site).unwrap().pc, 1);
}

#[test]
fn finish_allows_duplicate_region_headers_exits_verifier_tests() {
    // Given: semantically invalid duplicate headers and exit targets.
    let mut builder = StructureMapBuilder::new();
    let first = builder.begin_region(0, vec![]);
    let second = builder.begin_region(0, vec![]);
    let exits = vec![RegionExit { target: 1 }, RegionExit { target: 1 }];
    builder
        .finish_region(first, RegionKind::Branch, exits.clone())
        .unwrap();
    builder
        .finish_region(second, RegionKind::Branch, exits)
        .unwrap();

    // When: the structurally possible map is finalized.
    let map = builder
        .finish(
            &[
                Instruction::Jump { target: 1 },
                Instruction::Return { src: 0 },
            ],
            1,
        )
        .unwrap();

    // Then: semantic-invalid metadata remains available to the verifier.
    assert_eq!(map.regions().len(), 2);
    assert_eq!(map.regions()[0].exits.len(), 2);
}

#[test]
fn default_map_accessors_empty_bounds_safe() {
    // Given: the default empty map.
    let map = StructureMap::default();

    // When: every indexed and collection accessor is queried.
    let loops: Vec<_> = map.loop_regions().collect();

    // Then: collections are empty and indexed lookups return None.
    assert!(map.blocks().is_empty());
    assert!(map.regions().is_empty());
    assert!(map.operation_sites().is_empty());
    assert!(loops.is_empty());
    assert_eq!(map.block(BlockId(0)), None);
    assert_eq!(map.block_by_pc(0), None);
    assert_eq!(map.region(RegionId(0)), None);
    assert_eq!(map.region_by_entry_pc(0), None);
    assert_eq!(map.operation_site(OperationSiteId(0)), None);
}

use wustite::bytecode::{CompareOperator, Function, Instruction};
use wustite::executable::ExecutableFunction;
use wustite::frontend::python::compile_python_function;
use wustite::structure_map::{
    BlockEdge, BlockId, EdgeKind, OperationSiteId, RegionId, RegionKind, SlotType, StructureMap,
    TypeFact,
};
use wustite::value::Value;
use wustite::verifier;
use wustite::wvm::Vm;

const SUM_SOURCE: &str = r#"def main():
    acc = 0
    index = 1
    step = 1
    limit = 101
    while index < limit:
        acc = acc + index
        index = index + step
    return acc
"#;

#[test]
fn python_sum_runs_in_both_wvm_tiers() {
    let executable = compile_python_function(SUM_SOURCE, "main").unwrap();
    verifier::verify(&executable).unwrap();

    let structure_map = executable.structure_map();
    assert_eq!(structure_map.regions().len(), 1);
    let region = structure_map.region(RegionId(0)).unwrap();
    assert_eq!(region.entry, 8);
    assert_eq!(region.kind, RegionKind::Loop { backedge: 14 });
    assert!(matches!(
        executable.bytecode().code[region.entry],
        Instruction::CompareOp {
            op: CompareOperator::Lt,
            ..
        }
    ));
    assert!(matches!(
        executable.bytecode().code[14],
        Instruction::Jump { target } if target == region.entry
    ));
    assert_eq!(region.exits.len(), 1);
    assert_eq!(region.exits[0].target, 15);
    assert!(matches!(
        executable.bytecode().code[region.exits[0].target],
        Instruction::Return { .. }
    ));
    assert_eq!(region.entry_summary.len(), 4);
    assert_eq!(
        region
            .entry_summary
            .iter()
            .map(|slot| (slot.register, slot.ty))
            .collect::<Vec<_>>(),
        vec![
            (0, SlotType::SmallInt),
            (2, SlotType::SmallInt),
            (4, SlotType::SmallInt),
            (6, SlotType::SmallInt),
        ]
    );

    assert_eq!(structure_map.operation_sites().len(), 3);
    for (id, pc, result) in [
        (OperationSiteId(0), 8, SlotType::Bool),
        (OperationSiteId(1), 10, SlotType::SmallInt),
        (OperationSiteId(2), 12, SlotType::SmallInt),
    ] {
        let site = structure_map.operation_site(id).unwrap();
        assert_eq!(site.pc, pc);
        assert_eq!(site.lhs, TypeFact::Proven(SlotType::SmallInt));
        assert_eq!(site.rhs, TypeFact::Proven(SlotType::SmallInt));
        assert_eq!(site.result, TypeFact::Proven(result));
    }

    assert_eq!(
        structure_map
            .blocks()
            .iter()
            .map(|block| (block.start_pc, block.end_pc, block.successors.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                0,
                8,
                vec![BlockEdge {
                    target: BlockId(1),
                    kind: EdgeKind::Fallthrough,
                }],
            ),
            (
                8,
                10,
                vec![
                    BlockEdge {
                        target: BlockId(2),
                        kind: EdgeKind::BranchTrue,
                    },
                    BlockEdge {
                        target: BlockId(3),
                        kind: EdgeKind::BranchFalse,
                    },
                ],
            ),
            (
                10,
                15,
                vec![BlockEdge {
                    target: BlockId(1),
                    kind: EdgeKind::Jump,
                }],
            ),
            (15, 16, Vec::new()),
        ]
    );
    assert_eq!(region.blocks, vec![BlockId(1), BlockId(2)]);
    assert_eq!(region.summary.instruction_count, 7);
    assert_eq!(region.summary.block_count, 2);
    assert_eq!(region.summary.operation_count, 3);
    assert_eq!(region.summary.call_count, 0);

    let mut interpreter = Vm::with_hot_threshold(u64::MAX);
    assert_eq!(
        interpreter.execute(&executable).unwrap().value,
        Value::SmallInt(5050)
    );
    assert_eq!(interpreter.jit_report().compilation_attempts, 0);

    let mut tiered = Vm::with_hot_threshold(10);
    assert_eq!(
        tiered.execute(&executable).unwrap().value,
        Value::SmallInt(5050)
    );
    assert_eq!(tiered.jit_report().compilation_attempts, 1);
    assert_eq!(tiered.jit_report().compiled_regions, 1);
    assert_eq!(tiered.jit_report().native_executions, 1);
    assert_eq!(
        tiered.jit_report().last_resume_pc,
        Some(region.exits[0].target)
    );

    // The same VM retains the compiled region and profile for subsequent
    // executions of the same executable object.
    assert_eq!(
        tiered.execute(&executable).unwrap().value,
        Value::SmallInt(5050)
    );
    assert_eq!(tiered.jit_report().compilation_attempts, 0);
    assert_eq!(tiered.jit_report().compiled_regions, 0);
    assert_eq!(tiered.jit_report().native_executions, 1);
}

#[test]
fn frontend_rejects_invalid_loop_syntax_locations() {
    let unsupported = compile_python_function(
        "def main():\n    for value in reversed([1, 2, 3]):\n        value = value + 1\n    return 0\n",
        "main",
    )
    .err()
    .unwrap();
    assert_eq!(unsupported.location().unwrap().line, 2);
    assert!(unsupported.message().contains("enumerate"));

    let zero_step = compile_python_function(
        "def main():\n    for value in range(0, 10, 0):\n        value = value + 1\n    return 0\n",
        "main",
    )
    .err()
    .unwrap();
    assert_eq!(zero_step.location().unwrap().line, 2);
    assert!(zero_step.message().contains("step cannot be zero"));

    let unsupported_continue = compile_python_function(
        "def main():\n    while True:\n        continue\n    return 0\n",
        "main",
    )
    .err()
    .unwrap();
    assert_eq!(unsupported_continue.location().unwrap().line, 3);
    assert!(
        unsupported_continue
            .message()
            .contains("unsupported Python statement")
    );
}

#[test]
fn move_copies_values_verifier_checks_both_regs() {
    let executable = ExecutableFunction::new(
        Function {
            register_count: 2,
            code: vec![
                Instruction::ConstI64 { dst: 0, value: 42 },
                Instruction::Move { dst: 1, src: 0 },
                Instruction::Return { src: 1 },
            ],
        },
        StructureMap::default(),
    );
    assert_eq!(
        Vm::with_hot_threshold(u64::MAX)
            .execute(&executable)
            .unwrap()
            .value,
        Value::SmallInt(42)
    );

    for instruction in [
        Instruction::Move { dst: 2, src: 0 },
        Instruction::Move { dst: 0, src: 2 },
    ] {
        let invalid = ExecutableFunction::new(
            Function {
                register_count: 1,
                code: vec![instruction],
            },
            StructureMap::default(),
        );
        assert!(verifier::verify(&invalid).is_err());
    }
}

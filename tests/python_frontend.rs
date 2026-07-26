use wustite::bytecode::{Function, Instruction};
use wustite::executable::ExecutableFunction;
use wustite::frontend::python::compile_python_function;
use wustite::structure_map::{SlotType, StructureMap};
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
fn python_sum_compiles_and_runs_in_both_wvm_tiers() {
    let executable = compile_python_function(SUM_SOURCE, "main").unwrap();
    verifier::verify(&executable).unwrap();

    let region = &executable.structure_map.loops[0];
    assert_eq!(executable.structure_map.loops.len(), 1);
    assert_eq!((region.header, region.backedge), (8, 14));
    assert!(matches!(
        executable.bytecode.code[region.header],
        Instruction::LtI64 { .. }
    ));
    assert!(matches!(
        executable.bytecode.code[region.backedge],
        Instruction::Jump { target } if target == region.header
    ));
    assert_eq!(region.exits.len(), 1);
    assert_eq!(region.exits[0].target, 15);
    assert!(matches!(
        executable.bytecode.code[region.exits[0].target],
        Instruction::Return { .. }
    ));
    assert_eq!(region.live_slots.len(), 4);
    assert_eq!(
        region
            .live_slots
            .iter()
            .map(|slot| (slot.register, slot.ty))
            .collect::<Vec<_>>(),
        vec![
            (0, SlotType::I64),
            (2, SlotType::I64),
            (4, SlotType::I64),
            (6, SlotType::I64),
        ]
    );

    let mut interpreter = Vm::with_hot_threshold(u64::MAX);
    assert_eq!(
        interpreter.execute(&executable).unwrap().value,
        Value::I64(5050)
    );
    assert_eq!(interpreter.jit_report().compilation_attempts, 0);

    let mut tiered = Vm::with_hot_threshold(10);
    assert_eq!(tiered.execute(&executable).unwrap().value, Value::I64(5050));
    assert_eq!(tiered.jit_report().compilation_attempts, 1);
    assert_eq!(tiered.jit_report().compiled_regions, 1);
    assert_eq!(tiered.jit_report().native_executions, 1);
    assert_eq!(
        tiered.jit_report().last_resume_pc,
        Some(region.exits[0].target)
    );
}

#[test]
fn frontend_rejects_unsupported_or_unsafe_loop_syntax_with_locations() {
    let unsupported = compile_python_function(
        "def main():\n    if 1 < 2:\n        return 1\n    return 0\n",
        "main",
    )
    .err()
    .unwrap();
    assert_eq!(unsupported.location().unwrap().line, 2);
    assert!(unsupported.message().contains("unsupported"));

    let introduced = compile_python_function(
        "def main():\n    x = 0\n    while x < 1:\n        y = 1\n        x = x + 1\n    return x\n",
        "main",
    )
    .err()
    .unwrap();
    assert_eq!(introduced.location().unwrap().line, 4);
    assert!(introduced.message().contains("first introduced"));

    let nested = compile_python_function(
        "def main():\n    x = 0\n    while x < 1:\n        while x < 1:\n            x = x + 1\n    return x\n",
        "main",
    )
    .err()
    .unwrap();
    assert_eq!(nested.location().unwrap().line, 4);
    assert!(nested.message().contains("nested while"));
}

#[test]
fn move_copies_values_and_verifier_checks_both_registers() {
    let executable = ExecutableFunction {
        bytecode: Function {
            register_count: 2,
            code: vec![
                Instruction::ConstI64 { dst: 0, value: 42 },
                Instruction::Move { dst: 1, src: 0 },
                Instruction::Return { src: 1 },
            ],
        },
        structure_map: StructureMap::default(),
    };
    assert_eq!(
        Vm::with_hot_threshold(u64::MAX)
            .execute(&executable)
            .unwrap()
            .value,
        Value::I64(42)
    );

    for instruction in [
        Instruction::Move { dst: 2, src: 0 },
        Instruction::Move { dst: 0, src: 2 },
    ] {
        let invalid = ExecutableFunction {
            bytecode: Function {
                register_count: 1,
                code: vec![instruction],
            },
            structure_map: StructureMap::default(),
        };
        assert!(verifier::verify(&invalid).is_err());
    }
}

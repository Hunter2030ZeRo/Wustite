use wustite::bytecode::{Function, Instruction};
use wustite::executable::ExecutableFunction;
use wustite::object::ObjectKind;
use wustite::structure_map::{RegionExit, RegionKind, SlotType, StateSlot, StructureMapBuilder};
use wustite::value::Value;
use wustite::wvm::Vm;

fn executable_with_live_slot(slot_type: SlotType) -> ExecutableFunction {
    let function = Function {
        register_count: 3,
        code: vec![
            Instruction::ConstI64 { dst: 0, value: 0 },
            Instruction::ConstI64 { dst: 1, value: 1 },
            Instruction::LtI64 {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            Instruction::Branch {
                cond: 2,
                yes: 4,
                no: 6,
            },
            Instruction::AddI64 {
                dst: 0,
                lhs: 0,
                rhs: 1,
            },
            Instruction::Jump { target: 2 },
            Instruction::Return { src: 0 },
        ],
    };
    let mut map = StructureMapBuilder::new();
    let region = map.begin_region(
        2,
        vec![
            StateSlot {
                register: 0,
                ty: slot_type,
            },
            StateSlot {
                register: 1,
                ty: SlotType::SmallInt,
            },
        ],
    );
    map.finish_region(
        region,
        RegionKind::Loop { backedge: 5 },
        vec![RegionExit { target: 6 }],
    )
    .unwrap();
    let structure_map = map.finish(&function.code, function.register_count).unwrap();
    ExecutableFunction::new(function, structure_map)
}

#[test]
fn any_live_slot_executes_through_native_pointer_state() {
    // Given: a valid loop whose entry metadata contains an Any slot.
    let executable = executable_with_live_slot(SlotType::Any);
    let mut vm = Vm::with_hot_threshold(0);
    for _ in 0..3 {
        vm.execute(&executable).unwrap();
    }

    // When: the JIT executes the dynamically typed live state.
    let result = vm.execute(&executable).unwrap();

    // Then: the result is preserved without disabling the native region.
    assert_eq!(result.value, Value::SmallInt(1));
    assert_eq!(vm.jit_report().compiled_regions, 1);
    assert_eq!(vm.jit_report().native_executions, 1);
    assert!(vm.jit_report().failures.is_empty());
}

#[test]
fn object_live_slot_executes_through_native_pointer_state() {
    // Given: a loop whose entry metadata claims an object live slot.
    let slot_type = SlotType::Object(ObjectKind::List);
    let executable = executable_with_live_slot(slot_type);
    let mut vm = Vm::with_hot_threshold(0);
    for _ in 0..3 {
        vm.execute(&executable).unwrap();
    }

    // When: JIT construction lowers the object ABI boundary.
    let result = vm.execute(&executable).unwrap();

    // Then: execution succeeds without disabling the native region.
    assert_eq!(result.value, Value::SmallInt(1));
    assert_eq!(vm.jit_report().compiled_regions, 1);
    assert_eq!(vm.jit_report().native_executions, 1);
    assert!(vm.jit_report().failures.is_empty());
}

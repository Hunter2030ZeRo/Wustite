use num_bigint::BigInt;

use crate::object::{Object, ObjectHeap};
use crate::value::Value;
use crate::wvm::Frame;

use super::super::{QuickInstruction, QuickOutcome, execute_quick};

fn frame(pc: usize, registers: Vec<Value>) -> Frame {
    Frame {
        pc,
        registers,
        suppress_osr_pc: None,
        suppressed_regions: std::collections::HashSet::new(),
    }
}

#[test]
fn quick_execution_handles_exact_smallints_and_aliases() {
    let mut heap = ObjectHeap::new();
    for instruction in [
        QuickInstruction::Add {
            dst: 0,
            lhs: 0,
            rhs: 1,
        },
        QuickInstruction::Add {
            dst: 1,
            lhs: 0,
            rhs: 1,
        },
    ] {
        let mut frame = frame(4, vec![Value::SmallInt(40), Value::SmallInt(2)]);
        assert_eq!(
            execute_quick(instruction, &mut frame, &mut heap).unwrap(),
            QuickOutcome::Handled
        );
        assert_eq!(frame.pc, 5);
        let dst = match instruction {
            QuickInstruction::Add { dst, .. } | QuickInstruction::Lt { dst, .. } => dst,
        };
        assert_eq!(frame.registers[usize::from(dst)], Value::SmallInt(42));
    }

    let mut frame = frame(7, vec![Value::SmallInt(-3), Value::SmallInt(2)]);
    assert_eq!(
        execute_quick(
            QuickInstruction::Lt {
                dst: 0,
                lhs: 0,
                rhs: 1,
            },
            &mut frame,
            &mut heap,
        )
        .unwrap(),
        QuickOutcome::Handled
    );
    assert_eq!(frame.pc, 8);
    assert_eq!(frame.registers[0], Value::Bool(true));
}

#[test]
fn quick_execution_guard_miss_is_side_effect_free() {
    let mut heap = ObjectHeap::new();
    let bigint = heap.allocate(Object::BigInt(BigInt::from(9))).unwrap();
    for mismatch in [
        Value::Bool(true),
        Value::Float(1.5),
        Value::Object(bigint),
        Value::Uninitialized,
    ] {
        for instruction in [
            QuickInstruction::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            QuickInstruction::Lt {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
        ] {
            let mut frame = frame(3, vec![Value::SmallInt(1), mismatch, Value::SmallInt(77)]);
            let before = frame.registers.clone();
            assert_eq!(
                execute_quick(instruction, &mut frame, &mut heap).unwrap(),
                QuickOutcome::GuardMiss
            );
            assert_eq!(frame.pc, 3);
            assert_eq!(frame.registers, before);
            assert_eq!(heap.get(bigint).unwrap(), &Object::BigInt(BigInt::from(9)));
        }
    }

    let mut overflow = frame(
        10,
        vec![
            Value::SmallInt(i64::MAX),
            Value::SmallInt(1),
            Value::Uninitialized,
        ],
    );
    let add = QuickInstruction::Add {
        dst: 2,
        lhs: 0,
        rhs: 1,
    };
    assert_eq!(
        execute_quick(add, &mut overflow, &mut heap).unwrap(),
        QuickOutcome::Handled
    );
    assert_eq!(overflow.pc, 11);
    let Value::Object(promoted) = overflow.registers[2] else {
        panic!("overflow did not promote")
    };
    assert_eq!(
        heap.get(promoted).unwrap(),
        &Object::BigInt(BigInt::from(i64::MAX) + 1)
    );

    overflow.pc = 12;
    let before = overflow.registers.clone();
    let downstream = QuickInstruction::Lt {
        dst: 0,
        lhs: 2,
        rhs: 1,
    };
    assert_eq!(
        execute_quick(downstream, &mut overflow, &mut heap).unwrap(),
        QuickOutcome::GuardMiss
    );
    assert_eq!(overflow.pc, 12);
    assert_eq!(overflow.registers, before);
}

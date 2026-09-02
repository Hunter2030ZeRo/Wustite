use super::*;
use crate::adaptive_v2::native::{NativeCompiler, NativeValue};
use crate::adaptive_v2::profile::{AdaptiveProfile, FactClass, LiveObservation, ProfileCase};
use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
use crate::bytecode::{BinaryOperator, CompareOperator, Function, Instruction};
use crate::executable::ExecutableParameter;
use crate::structure_map::{OperationSiteId, SlotType, StructureMap};

fn permits(
    executable: &ExecutableFunction,
    arguments: &[Value],
) -> (SnapshotDraft, crate::adaptive_v2::profile::CompilePermit) {
    let mut profile = AdaptiveProfile::new(executable.id().as_u64(), 32);
    let observation = LiveObservation::new(ProfileCase::new(1), FactClass::UnknownClassified);
    for _ in 0..64 {
        profile.observe_live(observation);
    }
    let permit = profile.take_record_permit().expect("record permit");
    let draft = record_entry(executable, arguments, permit).expect("entry draft");
    assert!(profile.finish_recording());
    for _ in 0..32 {
        profile.observe_live(observation);
    }
    (
        draft,
        profile.take_compile_permit().expect("compile permit"),
    )
}

fn executable(code: Vec<Instruction>, parameters: Vec<ExecutableParameter>) -> ExecutableFunction {
    ExecutableFunction::new_with_parameters(
        Function {
            register_count: 5,
            code,
        },
        StructureMap::default(),
        parameters,
    )
}

#[test]
fn stable_int_branch_runs_native() {
    // Given: a non-template two-way integer function and both live profile windows.
    let function = executable(
        vec![
            Instruction::CompareOp {
                dst: 2,
                op: CompareOperator::Gt,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::Branch {
                cond: 2,
                yes: 2,
                no: 4,
            },
            Instruction::BinaryOp {
                dst: 3,
                op: BinaryOperator::Subtract,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(1),
            },
            Instruction::Return { src: 3 },
            Instruction::BinaryOp {
                dst: 4,
                op: BinaryOperator::Subtract,
                lhs: 1,
                rhs: 0,
                site: OperationSiteId(2),
            },
            Instruction::Return { src: 4 },
        ],
        vec![
            ExecutableParameter {
                name: "left".to_owned(),
                register: 0,
                ty: SlotType::SmallInt,
            },
            ExecutableParameter {
                name: "right".to_owned(),
                register: 1,
                ty: SlotType::SmallInt,
            },
        ],
    );
    let (draft, compile) = permits(&function, &[Value::SmallInt(19), Value::SmallInt(7)]);
    let snapshot = VerifiedSnapshot::seal(draft, compile).expect("verified entry CFG");
    let native = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("Cranelift entry CFG");

    // When: both native branch directions execute.
    let greater = native
        .execute(&[NativeValue::Integer(19), NativeValue::Integer(7)])
        .expect("greater branch");
    let lesser = native
        .execute(&[NativeValue::Integer(4), NativeValue::Integer(13)])
        .expect("lesser branch");

    // Then: results are exact and neither branch enters generic dispatch.
    assert_eq!(greater.values, vec![NativeValue::Integer(12)]);
    assert_eq!(lesser.values, vec![NativeValue::Integer(9)]);
    assert_eq!(greater.counters.generic_dispatch_calls, 0);
    assert_eq!(lesser.counters.generic_dispatch_calls, 0);
}

#[test]
fn stable_float_arithmetic_runs_native() {
    // Given: a float multiply-add entry with an observed F64 schema.
    let function = executable(
        vec![
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Multiply,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::ConstFloat { dst: 3, value: 0.5 },
            Instruction::BinaryOp {
                dst: 4,
                op: BinaryOperator::Add,
                lhs: 2,
                rhs: 3,
                site: OperationSiteId(1),
            },
            Instruction::Return { src: 4 },
        ],
        vec![
            ExecutableParameter {
                name: "left".to_owned(),
                register: 0,
                ty: SlotType::Float,
            },
            ExecutableParameter {
                name: "right".to_owned(),
                register: 1,
                ty: SlotType::Float,
            },
        ],
    );
    let (draft, compile) = permits(&function, &[Value::Float(3.0), Value::Float(4.0)]);
    let snapshot = VerifiedSnapshot::seal(draft, compile).expect("verified float entry");
    let native = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("Cranelift float entry");

    // When: the native entry executes.
    let outcome = native
        .execute(&[
            NativeValue::FloatBits(3.0_f64.to_bits()),
            NativeValue::FloatBits(4.0_f64.to_bits()),
        ])
        .expect("float execution");

    // Then: the result is 12.5 and the path is helper-free and generic-free.
    assert_eq!(
        outcome.values,
        vec![NativeValue::FloatBits(12.5_f64.to_bits())]
    );
    assert_eq!(outcome.counters.machine_entries, 1);
    assert_eq!(outcome.counters.helper_calls, 0);
    assert_eq!(outcome.counters.generic_dispatch_calls, 0);

    #[cfg(feature = "inkwell")]
    {
        native
            .execute(&[
                NativeValue::FloatBits(3.0_f64.to_bits()),
                NativeValue::FloatBits(4.0_f64.to_bits()),
            ])
            .and_then(|observed| {
                let mut compiler = NativeCompiler::new();
                compiler.observe_tier1(&observed)?;
                compiler.compile_tier2(&snapshot)?.execute(&[
                    NativeValue::FloatBits(2.0_f64.to_bits()),
                    NativeValue::FloatBits(5.0_f64.to_bits()),
                ])
            })
            .map(|tier2| {
                assert_eq!(
                    tier2.values,
                    vec![NativeValue::FloatBits(10.5_f64.to_bits())]
                );
                assert_eq!(tier2.counters.generic_dispatch_calls, 0);
            })
            .expect("LLVM float entry");
    }
}

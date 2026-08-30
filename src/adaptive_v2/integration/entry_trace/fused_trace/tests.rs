use super::*;
use crate::adaptive_v2::heap::GcConfig;
use crate::adaptive_v2::native::{AdaptiveNativeContext, NativeCompiler, NativeValue};
use crate::adaptive_v2::profile::{AdaptiveProfile, FactClass, LiveObservation, ProfileCase};
use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
use crate::adaptive_v2::wxir_v2::dependency::DependencyKind;
use crate::adaptive_v2::wxir_v2::ir::InstructionKind;
use crate::bytecode::{BinaryOperator, Function, Instruction};
use crate::executable::{ExecutableConstant, ExecutableParameter};
use crate::object::{ObjectKind, ObjectRef};
use crate::structure_map::{OperationSiteId, SlotType, StructureMapBuilder};

fn permits(
    executable: &ExecutableFunction,
) -> (
    crate::adaptive_v2::profile::RecordPermit,
    crate::adaptive_v2::profile::CompilePermit,
) {
    let mut profile = AdaptiveProfile::new(executable.id().as_u64());
    let observation = LiveObservation::new(ProfileCase::new(1), FactClass::UnknownClassified);
    for _ in 0..64 {
        profile.observe_live(observation);
    }
    let record = profile.take_record_permit().expect("live record permit");
    assert!(profile.finish_recording());
    for _ in 0..32 {
        profile.observe_live(observation);
    }
    let compile = profile.take_compile_permit().expect("live compile permit");
    (record, compile)
}

fn list_get() -> ExecutableFunction {
    let code = vec![
        Instruction::GetItem {
            dst: 2,
            object: 0,
            key: 1,
        },
        Instruction::Return { src: 2 },
    ];
    let mut map = StructureMapBuilder::new();
    map.record_parameter(0, 0, "items".to_owned(), SlotType::Object(ObjectKind::List))
        .expect("list parameter");
    map.record_parameter(1, 1, "index".to_owned(), SlotType::SmallInt)
        .expect("index parameter");
    ExecutableFunction::new_with_parameters(
        Function {
            code: code.clone(),
            register_count: 3,
        },
        map.finish(&code, 3).expect("list structure map"),
        vec![
            ExecutableParameter {
                name: "items".to_owned(),
                register: 0,
                ty: SlotType::Object(ObjectKind::List),
            },
            ExecutableParameter {
                name: "index".to_owned(),
                register: 1,
                ty: SlotType::SmallInt,
            },
        ],
    )
}

#[test]
fn param_free_rotation_loop_lowers_to_one_entry_snapshot() {
    const SOURCE: &str = r#"
def main():
    values = []
    index = 0
    while index < 64:
        values.append(index)
        index = index + 1
    index = 0
    while index < 32:
        values.insert(0, values.pop())
        index = index + 1
    total = 0
    for value in values:
        total = total + value
    return total
"#;
    let mut runtime = crate::Runtime::new_adaptive_v2(crate::RuntimeConfig {
        execution_mode: crate::ExecutionMode::AdaptiveJit,
        hot_threshold: 1,
    });
    let executable = runtime.compile_function(SOURCE, "main").expect("fixture");
    let (record_permit, compile_permit) = permits(&executable);
    let facts = FusedTraceFacts::new(executable.id().as_u64());
    let draft = record(FusedTraceRequest {
        executable: &executable,
        arguments: &[],
        permit: record_permit,
        facts: &facts,
    })
    .expect("entry record")
    .expect("accepted entry trace");
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit).expect("verified entry trace");
    assert_eq!(
        snapshot
            .body()
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| matches!(
                instruction.kind.semantic(),
                InstructionKind::ListReversePrefix { .. }
            ))
            .count(),
        3
    );
    let mut compiler = NativeCompiler::new();
    let code = compiler
        .compile_tier1(&snapshot)
        .expect("compiled entry snapshot");
    let mut heap = AdaptiveNativeContext::new(GcConfig::default());
    let outcome = code
        .execute_with_adaptive_heap(&[], &mut heap)
        .expect("executed entry snapshot");
    assert_eq!(outcome.values, vec![NativeValue::Integer(2_016)]);
    assert_eq!(outcome.counters.helper_calls, 0);

    let near_miss = runtime
        .compile_function(
            &SOURCE.replace("values.insert(0", "values.insert(1"),
            "main",
        )
        .expect("near-miss fixture");
    let (record_permit, compile_permit) = permits(&near_miss);
    let facts = FusedTraceFacts::new(near_miss.id().as_u64());
    let draft = record(FusedTraceRequest {
        executable: &near_miss,
        arguments: &[],
        permit: record_permit,
        facts: &facts,
    })
    .expect("near-miss record")
    .expect("accepted near-miss entry trace");
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit).expect("verified near miss");
    assert!(snapshot.body().blocks.iter().all(|block| {
        block.instructions.iter().all(|instruction| {
            !matches!(
                instruction.kind.semantic(),
                InstructionKind::ListReversePrefix { .. }
            )
        })
    }));
}

#[test]
fn list_input_result_depends_on_live_elements_index() {
    // Given: a verified list access and both required live profile windows.
    let executable = list_get();
    let (record_permit, compile_permit) = permits(&executable);
    let arguments = [Value::Object(ObjectRef::new(1, 0, 0)), Value::SmallInt(0)];
    let facts = FusedTraceFacts::new(executable.id().as_u64())
        .with_access(0, FusedAccessFact::ListI64 { layout_epoch: 1 });
    let draft = record(FusedTraceRequest {
        executable: &executable,
        arguments: &arguments,
        permit: record_permit,
        facts: &facts,
    })
    .expect("fused record")
    .expect("accepted list trace");
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit).expect("verified list trace");
    let mut compiler = NativeCompiler::new();
    let code = compiler
        .compile_tier1(&snapshot)
        .expect("compiled list trace");

    // When: the same machine code sees different list contents and indexes.
    let mut first_heap = AdaptiveNativeContext::new(GcConfig::default());
    let first_list = first_heap.allocate_list().expect("first list");
    first_heap
        .append_integer(first_list, 10)
        .expect("first item");
    first_heap
        .append_integer(first_list, 20)
        .expect("second item");
    let first = code
        .execute_with_adaptive_heap(&[first_list, NativeValue::Integer(0)], &mut first_heap)
        .expect("first list execution");
    let mut second_heap = AdaptiveNativeContext::new(GcConfig::default());
    let second_list = second_heap.allocate_list().expect("second list");
    second_heap
        .append_integer(second_list, 10)
        .expect("first item");
    second_heap
        .append_integer(second_list, 99)
        .expect("second item");
    let second = code
        .execute_with_adaptive_heap(&[second_list, NativeValue::Integer(1)], &mut second_heap)
        .expect("second list execution");

    // Then: emitted WXIR is direct and the results remain input-dependent.
    assert_eq!(first.values, vec![NativeValue::Integer(10)]);
    assert_eq!(second.values, vec![NativeValue::Integer(99)]);
    assert_eq!(first.counters.helper_calls, 0);
    assert_eq!(second.counters.helper_calls, 0);
    #[cfg(feature = "inkwell")]
    {
        compiler.observe_tier1(&first).expect("observe tier1");
        let tier2 = compiler.compile_tier2(&snapshot).expect("compiled tier2");
        let llvm = tier2
            .execute_with_adaptive_heap(&[first_list, NativeValue::Integer(1)], &mut first_heap)
            .expect("LLVM direct list execution");
        assert_eq!(llvm.values, vec![NativeValue::Integer(20)]);
        assert_eq!(llvm.counters.helper_calls, 0);
    }
    assert!(snapshot.body().blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction.kind.semantic(), InstructionKind::ListGet))
    }));
    assert!(snapshot.body().blocks.iter().all(|block| {
        block.instructions.iter().all(|instruction| {
            !matches!(instruction.kind.semantic(), InstructionKind::Helper { .. })
        })
    }));
}

fn add_callee() -> ExecutableFunction {
    let code = vec![
        Instruction::BinaryOp {
            dst: 2,
            op: BinaryOperator::Add,
            lhs: 0,
            rhs: 1,
            site: OperationSiteId(0),
        },
        Instruction::Return { src: 2 },
    ];
    let mut map = StructureMapBuilder::new();
    map.record_parameter(0, 0, "left".to_owned(), SlotType::SmallInt)
        .expect("left parameter");
    map.record_parameter(1, 1, "right".to_owned(), SlotType::SmallInt)
        .expect("right parameter");
    map.record_operation(
        0,
        crate::structure_map::Fact::Proven(SlotType::SmallInt),
        crate::structure_map::Fact::Proven(SlotType::SmallInt),
        crate::structure_map::Fact::Proven(SlotType::SmallInt),
    )
    .expect("add site");
    ExecutableFunction::new_with_parameters(
        Function {
            code: code.clone(),
            register_count: 3,
        },
        map.finish(&code, 3).expect("callee structure map"),
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
    )
}

#[test]
fn verified_constant_callee_inlined_helper_free() {
    // Given: a caller whose immutable constant pool owns a verified add body.
    let callee = add_callee();
    crate::verifier::verify(&callee).expect("verified callee");
    let callee_id = callee.id().as_u64();
    let code = vec![
        Instruction::LoadConstant {
            dst: 2,
            constant: crate::executable::ConstantId(0),
        },
        Instruction::Call {
            dst: 3,
            callable: 2,
            args: vec![0, 1],
        },
        Instruction::Return { src: 3 },
    ];
    let mut map = StructureMapBuilder::new();
    map.record_parameter(0, 0, "left".to_owned(), SlotType::SmallInt)
        .expect("left parameter");
    map.record_parameter(1, 1, "right".to_owned(), SlotType::SmallInt)
        .expect("right parameter");
    map.record_constant(0, ObjectKind::Function)
        .expect("callee constant");
    let caller = ExecutableFunction::new_with_abi(
        Function {
            code: code.clone(),
            register_count: 4,
        },
        map.finish(&code, 4).expect("caller structure map"),
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
        vec![ExecutableConstant::Function(Box::new(callee))],
    );
    let (record_permit, compile_permit) = permits(&caller);

    // When: the caller is statically lowered and executed with live operands.
    let arguments = [Value::SmallInt(20), Value::SmallInt(22)];
    let facts = FusedTraceFacts::new(caller.id().as_u64());
    let draft = record(FusedTraceRequest {
        executable: &caller,
        arguments: &arguments,
        permit: record_permit,
        facts: &facts,
    })
    .expect("fused record")
    .expect("accepted direct call");
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit).expect("verified caller");
    let native = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compiled caller");
    let outcome = native
        .execute(&[NativeValue::Integer(7), NativeValue::Integer(9)])
        .expect("inline execution");

    // Then: the body, dependency, and result prove real state-dependent inlining.
    assert_eq!(outcome.values, vec![NativeValue::Integer(16)]);
    assert_eq!(outcome.counters.helper_calls, 0);
    assert!(snapshot.body().dependencies.iter().any(|dependency| {
        dependency.kind == DependencyKind::Callee && dependency.identity == callee_id
    }));
    assert!(snapshot.body().blocks.iter().all(|block| {
        block.instructions.iter().all(|instruction| {
            !matches!(
                instruction.kind.semantic(),
                InstructionKind::Call { .. } | InstructionKind::Helper { .. }
            )
        })
    }));
}

#[test]
fn fifth_live_case_never_yields_record_permit() {
    // Given: four established live cases followed by a fifth polymorphic case.
    let mut profile = AdaptiveProfile::new(7);
    for case in 0..4 {
        for _ in 0..32 {
            profile.observe_live(LiveObservation::new(
                ProfileCase::new(case),
                FactClass::UnknownClassified,
            ));
        }
    }

    // When: a fifth distinct case is observed through the live probe.
    profile.observe_live(LiveObservation::new(
        ProfileCase::new(4),
        FactClass::UnknownClassified,
    ));

    // Then: the profile is generic/cold and cannot authorize static lowering.
    assert!(profile.is_generic());
    assert!(profile.take_record_permit().is_none());
}

#[test]
fn fusion_rejects_static_hint_without_live_windows() {
    // Given: a static hint with no live observations.
    let mut profile = AdaptiveProfile::new(7);
    profile.seed_static_hint(ProfileCase::new(1), 1_000_000);

    // When: recording authorization is requested.
    let permit = profile.take_record_permit();

    // Then: static metadata alone cannot enter recording or compilation.
    assert!(permit.is_none());
    assert_eq!(profile.live_entries(), 0);
}

#[test]
fn stale_schema_opaque_call_kept_cold() {
    // Given: a live permit for one schema and an opaque callable parameter.
    let code = vec![
        Instruction::Call {
            dst: 2,
            callable: 0,
            args: vec![1],
        },
        Instruction::Return { src: 2 },
    ];
    let mut map = StructureMapBuilder::new();
    map.record_parameter(
        0,
        0,
        "callable".to_owned(),
        SlotType::Object(ObjectKind::Function),
    )
    .expect("callable parameter");
    map.record_parameter(1, 1, "value".to_owned(), SlotType::SmallInt)
        .expect("value parameter");
    let executable = ExecutableFunction::new_with_parameters(
        Function {
            code: code.clone(),
            register_count: 3,
        },
        map.finish(&code, 3).expect("opaque call structure map"),
        vec![
            ExecutableParameter {
                name: "callable".to_owned(),
                register: 0,
                ty: SlotType::Object(ObjectKind::Function),
            },
            ExecutableParameter {
                name: "value".to_owned(),
                register: 1,
                ty: SlotType::SmallInt,
            },
        ],
    );
    let (record_permit, _) = permits(&executable);
    let arguments = [Value::Object(ObjectRef::new(1, 0, 0)), Value::SmallInt(9)];
    let stale = FusedTraceFacts::new(executable.id().as_u64().saturating_add(1));

    // When: lowering sees stale facts, then current facts with an unverified callee.
    let stale_result = record(FusedTraceRequest {
        executable: &executable,
        arguments: &arguments,
        permit: record_permit,
        facts: &stale,
    })
    .expect("stale lowering decision");
    let current = FusedTraceFacts::new(executable.id().as_u64());
    let opaque_result = record(FusedTraceRequest {
        executable: &executable,
        arguments: &arguments,
        permit: record_permit,
        facts: &current,
    })
    .expect("opaque lowering decision");

    // Then: both attempts remain on the cold interpreter path.
    assert!(stale_result.is_none());
    assert!(opaque_result.is_none());
}

#[test]
fn sequence_facts_keep_runtime_guard() {
    // Given: a live permit but only a Guardable sequence strategy and no emitted guard.
    let executable = list_get();
    let (record_permit, _) = permits(&executable);
    let mut facts = FusedTraceFacts::new(executable.id().as_u64());
    facts.include_sequence_access(
        0,
        crate::structure_map::Fact::Guardable(crate::object::SequenceStrategy::I64),
        crate::structure_map::Fact::Proven(true),
        crate::structure_map::Fact::Proven(SlotType::SmallInt),
        1,
    );

    // When: lowering is attempted without a Proven fact or explicit runtime guard.
    let result = record(FusedTraceRequest {
        executable: &executable,
        arguments: &[Value::Object(ObjectRef::new(1, 0, 0)), Value::SmallInt(0)],
        permit: record_permit,
        facts: &facts,
    })
    .expect("guardable lowering decision");

    // Then: the access remains cold instead of treating Guardable as Proven.
    assert!(facts.accesses.is_empty());
    assert!(result.is_none());
}

#[test]
fn shape_loop_fixture_lowers_to_input_derived_helper_free_entry() {
    // Given: the real shape benchmark and live-only recording/compilation permits.
    let executable = crate::frontend::python::compile_python_function(
        include_str!("../../../../../benchmarks/adaptive_shape_objects.py"),
        "main",
    )
    .expect("shape fixture");
    let (record_permit, compile_permit) = permits(&executable);
    let facts = FusedTraceFacts::new(executable.id().as_u64());

    // When: immutable bytecode and constant-pool call-tree facts are lowered.
    let draft = record(FusedTraceRequest {
        executable: &executable,
        arguments: &[],
        permit: record_permit,
        facts: &facts,
    })
    .expect("shape lowering")
    .expect("accepted scalar-replaced shape loop");
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit).expect("verified shape entry");
    let outcome = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compiled shape entry")
        .execute(&[])
        .expect("shape entry execution");

    // Then: the whole entry executes natively with its exact operation-derived result.
    assert_eq!(outcome.values, vec![NativeValue::Integer(4096)]);
    assert_eq!(outcome.counters.helper_calls, 0);
}

#[test]
fn call_loop_fixture_lowers_to_arg_derived_helper_free_entry() {
    // Given: the real bound-method benchmark and live-only recording/compilation permits.
    let executable = crate::frontend::python::compile_python_function(
        include_str!("../../../../../benchmarks/adaptive_call_objects.py"),
        "main",
    )
    .expect("call fixture");
    let (record_permit, compile_permit) = permits(&executable);
    let facts = FusedTraceFacts::new(executable.id().as_u64());

    // When: immutable bytecode and constant-pool call-tree facts are lowered.
    let draft = record(FusedTraceRequest {
        executable: &executable,
        arguments: &[],
        permit: record_permit,
        facts: &facts,
    })
    .expect("call lowering")
    .expect("accepted scalar-replaced call loop");
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit).expect("verified call entry");
    let outcome = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compiled call entry")
        .execute(&[])
        .expect("call entry execution");

    // Then: the whole entry executes natively with its exact operation-derived result.
    assert_eq!(outcome.values, vec![NativeValue::Integer(24_512)]);
    assert_eq!(outcome.counters.helper_calls, 0);
}

fn execute_macro(source: &str) -> Vec<NativeValue> {
    let executable =
        crate::frontend::python::compile_python_function(source, "main").expect("macro fixture");
    let (record_permit, compile_permit) = permits(&executable);
    let facts = FusedTraceFacts::new(executable.id().as_u64());
    let draft = record(FusedTraceRequest {
        executable: &executable,
        arguments: &[],
        permit: record_permit,
        facts: &facts,
    })
    .expect("macro lowering")
    .expect("accepted macro loop");
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit).expect("verified macro entry");
    NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compiled macro entry")
        .execute(&[])
        .expect("macro entry execution")
        .values
}

#[test]
fn scalar_replaced_fields_change_native_shape_result() {
    // Given: constructor arguments and a method body that differ from the benchmark constants.
    let source = r#"
class Point:
    def __init__(self, x: int, y: int):
        self.x = x
        self.y = y

    def total(self):
        return self.x * 2 + self.y

def main():
    total = 0
    index = 0
    while index < 5:
        point = Point(index + 2, index + 4)
        total = total + point.total()
        index = index + 1
    return total
"#;

    // When: the altered immutable call tree is lowered and executed.
    let values = execute_macro(source);

    // Then: the result reflects every live field-producing operation.
    assert_eq!(values, vec![NativeValue::Integer(70)]);
}

#[test]
fn scalar_replaced_callee_changes_native_call_result() {
    // Given: a callee whose multiplier, offset, argument, and loop bound all differ.
    let source = r#"
class Amplifier:
    def apply(self, value: int):
        return value * 4 + 2

def main():
    amplifier = Amplifier()
    total = 0
    index = 0
    while index < 7:
        total = total + amplifier.apply(index)
        index = index + 1
    return total
"#;

    // When: the altered immutable call tree is lowered and executed.
    let values = execute_macro(source);

    // Then: the result is derived from the callee arguments and body.
    assert_eq!(values, vec![NativeValue::Integer(98)]);
}

#[test]
fn compiler_kernels_entry_lowers_one_helper_free_native_cfg() {
    // Given: the real nested-loop compiler workload and live-only permits.
    let executable = crate::frontend::python::compile_python_function(
        include_str!("../../../../../benchmarks/compiler_kernels.py"),
        "main",
    )
    .expect("compiler kernels fixture");
    let (record_permit, compile_permit) = permits(&executable);
    let facts = FusedTraceFacts::new(executable.id().as_u64());

    // When: the immutable entry CFG and its verified scalar callee are lowered.
    let draft = record(FusedTraceRequest {
        executable: &executable,
        arguments: &[],
        permit: record_permit,
        facts: &facts,
    })
    .expect("compiler kernels lowering")
    .expect("accepted compiler kernels entry");
    let snapshot = VerifiedSnapshot::seal(draft, compile_permit).expect("verified entry CFG");
    let outcome = NativeCompiler::new()
        .compile_tier1(&snapshot)
        .expect("compiled entry CFG")
        .execute(&[])
        .expect("compiler kernels native execution");

    // Then: the result is operation-derived and the whole entry is helper-free.
    assert_eq!(outcome.values, vec![NativeValue::Integer(2_755)]);
    assert_eq!(outcome.counters.helper_calls, 0);
}

#[test]
fn scalar_cfg_result_tracks_changed_loop_callee_bodies() {
    // Given: two loops and a conditional callee whose operations differ from compiler_kernels.
    let source = r#"
def choose(left: int, right: int, enabled: bool):
    result = 0
    if enabled and left > right:
        result = left - right
    else:
        result = left * right
    return result

def main():
    total = 0
    index = 0
    while index < 3:
        total = total + index
        index = index + 1
    other = 0
    while other < 2:
        total = total + 10
        other = other + 1
    return total + choose(9, 4, True)
"#;

    // When: the changed immutable bytecode and callee body are lowered and executed.
    let values = execute_macro(source);

    // Then: native execution reflects the changed compare and arithmetic operations.
    assert_eq!(values, vec![NativeValue::Integer(28)]);
}

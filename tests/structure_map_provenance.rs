use wustite::frontend::python::compile_python_function;
use wustite::object::ObjectKind;
use wustite::structure_map::{EffectSummary, EscapeState, Fact, SlotType, TypeFact, ValueOrigin};

const SOURCE: &str = r#"
def helper(value: int):
    return value + 1

def main(value: int):
    pair = (value, 1)
    helper(pair[0])
    return len(pair)
"#;

#[test]
fn hir_wvm_lowering_compose_param_constant_call_escape_facts() {
    // Given: HIR with a typed parameter, function constant, aggregate, projection, and call.
    let executable = compile_python_function(SOURCE, "main").unwrap();

    // When: the finalized StructureMap is inspected.
    let map = executable.structure_map();

    // Then: HIR seeds and WVM-derived facts form one conservative value graph.
    let parameter = map
        .values()
        .iter()
        .find(|value| {
            matches!(
                value.origin,
                Fact::Proven(ValueOrigin::Parameter { index: 0, .. })
            )
        })
        .unwrap();
    assert_eq!(parameter.ty, TypeFact::Proven(SlotType::SmallInt));
    let function = map
        .values()
        .iter()
        .find(|value| {
            matches!(
                value.origin,
                Fact::Proven(ValueOrigin::ConstantPool {
                    kind: Some(ObjectKind::Function),
                    ..
                })
            )
        })
        .unwrap();
    assert_eq!(
        function.ty,
        TypeFact::Proven(SlotType::Object(ObjectKind::Function))
    );
    let call = map
        .instruction_facts()
        .iter()
        .find(|instruction| {
            instruction
                .effects
                .proven()
                .is_some_and(|effects| effects.may_call_unknown)
        })
        .unwrap();
    assert_eq!(
        call.effects,
        Fact::Proven(EffectSummary {
            may_mutate: true,
            may_allocate: true,
            may_call_unknown: true,
            may_access_global_state: true,
        })
    );
    assert!(
        call.inputs
            .iter()
            .filter_map(|input| input.value)
            .all(|id| map.value(id).unwrap().escape == Fact::Proven(EscapeState::Unknown))
    );
}

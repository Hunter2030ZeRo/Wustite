use std::sync::Arc;

use crate::bytecode::{BinaryOperator, Function, Instruction};
use crate::executable::ExecutableFunction;
use crate::structure_map::{OperationSiteId, SlotType, StructureMapBuilder, TypeFact};

use super::{FunctionRuntime, Vm};

fn exact_add(site_pc: usize) -> ExecutableFunction {
    let function = Function {
        register_count: 3,
        code: vec![
            Instruction::ConstSmallInt { dst: 0, value: 1 },
            Instruction::ConstSmallInt { dst: 1, value: 2 },
            Instruction::BinaryOp {
                dst: 2,
                op: BinaryOperator::Add,
                lhs: 0,
                rhs: 1,
                site: OperationSiteId(0),
            },
            Instruction::Return { src: 2 },
        ],
    };
    let mut builder = StructureMapBuilder::new();
    builder
        .record_operation(
            site_pc,
            TypeFact::Proven(SlotType::SmallInt),
            TypeFact::Proven(SlotType::SmallInt),
            TypeFact::Proven(SlotType::SmallInt),
        )
        .expect("operation site fixture should be representable");
    let structure_map = builder
        .finish(&function.code, function.register_count)
        .expect("structure map fixture should be representable");
    ExecutableFunction::new(function, structure_map)
}

#[test]
fn quick_code_runtime_identity() {
    let first = exact_add(2);
    let revision = exact_add(2);
    let active = FunctionRuntime::new(&first);
    let recursive = FunctionRuntime::recursive_placeholder(
        &first,
        Arc::clone(&active.quick_code),
        active.profiling_enabled,
    );
    let independent = FunctionRuntime::new(&revision);

    assert!(Arc::ptr_eq(&active.quick_code, &recursive.quick_code));
    assert!(!Arc::ptr_eq(&active.quick_code, &independent.quick_code));
}

#[test]
fn invalid_executable_builds_no_quick_runtime() {
    let invalid = exact_add(1);
    let mut vm = Vm::new();

    assert!(vm.execute(&invalid).is_err());
    assert!(vm.profile_for(&invalid).is_none());
}

#[test]
fn interpreter_getattr_uses_shape_guarded_inline_cache() {
    const SOURCE: &str = r#"
class Box:
    def __init__(self, value: int):
        self.value = value

    def read(self):
        return self.value

def main():
    box = Box(3)
    total = 0
    index = 0
    while index < 10:
        total = total + box.read()
        if index == 4:
            box.extra = 1
        index = index + 1
    return total
"#;
    let mut compiler = crate::Runtime::new(crate::RuntimeConfig::default());
    let executable = compiler.compile_function(SOURCE, "main").unwrap();
    let mut vm = Vm::interpreter();

    assert_eq!(
        vm.execute(&executable).unwrap().value,
        crate::value::Value::SmallInt(30)
    );

    let attribute_hits = vm
        .runtimes
        .values()
        .flat_map(|runtime| runtime.attribute_sites.iter())
        .map(|site| site.hits)
        .sum::<u64>();
    let method_hits = vm
        .runtimes
        .values()
        .flat_map(|runtime| runtime.call_sites.iter())
        .map(|site| site.hits)
        .sum::<u64>();
    assert!(
        attribute_hits >= 8,
        "expected guarded field-cache hits, got {attribute_hits}"
    );
    assert!(
        method_hits >= 8,
        "expected guarded method-cache hits, got {method_hits}"
    );
}

#[test]
fn interpreter_plain_calls_hit_the_function_inline_cache() {
    const SOURCE: &str = r#"
def add(left: int, right: int):
    return left + right

def main():
    total = 0
    index = 0
    while index < 10:
        total = total + add(index, 1)
        index = index + 1
    return total
"#;
    let mut compiler = crate::Runtime::new(crate::RuntimeConfig::default());
    let executable = compiler.compile_function(SOURCE, "main").unwrap();
    let mut vm = Vm::interpreter();

    assert_eq!(
        vm.execute(&executable).unwrap().value,
        crate::value::Value::SmallInt(55)
    );

    let hits = vm
        .runtimes
        .values()
        .flat_map(|runtime| runtime.call_sites.iter())
        .map(|site| site.hits)
        .sum::<u64>();
    assert!(hits >= 9, "expected function-cache hits, got {hits}");
    let leaf_hits = vm
        .runtimes
        .values()
        .flat_map(|runtime| runtime.call_sites.iter())
        .map(|site| site.interpreter_leaf_hits)
        .sum::<u64>();
    assert!(
        leaf_hits >= 9,
        "expected prepared scalar-leaf hits, got {leaf_hits}"
    );
}

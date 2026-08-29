use crate::bytecode::Instruction;
use crate::jit::CompilerBackend;
use crate::object::{Object, ObjectHeap};
use crate::runtime::{Runtime, RuntimeConfig};
use crate::value::Value;

use super::super::integration::AdaptiveVm;

#[test]
fn profiling_only_list_site_keeps_authoritative_wvm_ownership() {
    // Given: a cold list-read site and its authoritative public-heap receiver.
    let mut compiler = Runtime::new(RuntimeConfig::default());
    let executable = compiler
        .compile_function(
            "def main(values: list, index: int):\n    return values[index]\n",
            "main",
        )
        .expect("compile list fixture");
    let (pc, instruction) = executable
        .bytecode()
        .code
        .iter()
        .enumerate()
        .find(|(_, instruction)| matches!(instruction, Instruction::GetItem { .. }))
        .expect("list-read instruction");
    let Instruction::GetItem { object, key, .. } = instruction else {
        unreachable!("matched list-read instruction")
    };
    let mut heap = ObjectHeap::new();
    let reference = heap
        .allocate(Object::list(vec![Value::SmallInt(17)]))
        .expect("allocate list fixture");
    let mut registers = vec![Value::Uninitialized; executable.bytecode().register_count];
    registers[usize::from(*object)] = Value::Object(reference);
    registers[usize::from(*key)] = Value::SmallInt(0);
    let adaptive = AdaptiveVm::new(Some(CompilerBackend::Cranelift));

    // When: the site records its first live observation without compiled code.
    let ticket = adaptive.object_before(1, &executable, pc, instruction, &registers, &mut heap);

    // Then: WVM retains the operation and its receiver instead of round-tripping ownership.
    assert!(ticket.is_none());
    let Object::List(sequence) = heap
        .get(reference)
        .expect("authoritative receiver remains live")
    else {
        panic!("expected list receiver")
    };
    assert_eq!(sequence.get(0), Some(Value::SmallInt(17)));
}

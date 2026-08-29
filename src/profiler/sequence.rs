use crate::bytecode::{Instruction, Register};
use crate::object::{Object, ObjectHeap, ObjectKind, SequenceStrategy};
use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceLayoutCase {
    pub kind: ObjectKind,
    pub strategy: SequenceStrategy,
}

impl SequenceLayoutCase {
    pub const fn list(strategy: SequenceStrategy) -> Self {
        Self {
            kind: ObjectKind::List,
            strategy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceSpecialization {
    Unknown,
    Monomorphic(SequenceLayoutCase),
    Bimorphic([SequenceLayoutCase; 2]),
    Megamorphic,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct SiteSequenceProfile {
    first: Option<SequenceLayoutCase>,
    second: Option<SequenceLayoutCase>,
    megamorphic: bool,
}

impl SiteSequenceProfile {
    pub(super) fn observe(
        &mut self,
        instruction: &Instruction,
        registers: &[Value],
        heap: &ObjectHeap,
    ) {
        let Some(register) = sequence_register(instruction) else {
            return;
        };
        let Some(value) = registers.get(usize::from(register)).copied() else {
            return;
        };
        self.observe_value(value, heap);
    }

    pub(super) fn observe_value(&mut self, value: Value, heap: &ObjectHeap) {
        let Value::Object(reference) = value else {
            return;
        };
        let case = match heap.get(reference) {
            Ok(Object::List(sequence)) => SequenceLayoutCase::list(sequence.strategy()),
            Ok(Object::Tuple(sequence)) => SequenceLayoutCase {
                kind: ObjectKind::Tuple,
                strategy: sequence.strategy(),
            },
            Ok(_) | Err(_) => return,
        };
        self.observe_case(case);
    }

    fn observe_case(&mut self, case: SequenceLayoutCase) {
        if self.megamorphic || self.first == Some(case) || self.second == Some(case) {
            return;
        }
        if self.first.is_none() {
            self.first = Some(case);
        } else if self.second.is_none() {
            self.second = Some(case);
        } else {
            self.megamorphic = true;
        }
    }

    pub(super) const fn specialization(self) -> SequenceSpecialization {
        if self.megamorphic {
            SequenceSpecialization::Megamorphic
        } else if let (Some(first), Some(second)) = (self.first, self.second) {
            SequenceSpecialization::Bimorphic([first, second])
        } else if let Some(first) = self.first {
            SequenceSpecialization::Monomorphic(first)
        } else {
            SequenceSpecialization::Unknown
        }
    }
}

const fn sequence_register(instruction: &Instruction) -> Option<Register> {
    match instruction {
        Instruction::GetItem { object, .. }
        | Instruction::GetSlice { object, .. }
        | Instruction::SetItem { object, .. }
        | Instruction::SetSlice { object, .. }
        | Instruction::Length { object, .. } => Some(*object),
        Instruction::ListAppend { list, .. }
        | Instruction::ListInsert { list, .. }
        | Instruction::ListPop { list, .. } => Some(*list),
        _ => None,
    }
}

use std::collections::{BTreeMap, BTreeSet};

use super::super::SnapshotError;
use super::super::ir::{
    Block, BlockId, Instruction, InstructionKind, Terminator, ValueId, ValueType,
};
use super::DefinitionMap;

pub(super) fn verify_block(
    block: &Block,
    blocks: &BTreeMap<BlockId, &Block>,
    definitions: &DefinitionMap,
    dominators: &BTreeMap<BlockId, BTreeSet<BlockId>>,
) -> Result<(), SnapshotError> {
    let mut last_effect = None;
    let mut borrowed = block
        .parameters
        .iter()
        .filter(|parameter| parameter.ty == ValueType::BorrowedView)
        .map(|parameter| parameter.id)
        .collect::<BTreeSet<_>>();
    for (index, instruction) in block.instructions.iter().enumerate() {
        for input in &instruction.inputs {
            verify_use(*input, block.id, Some(index), definitions, dominators)?;
        }
        verify_instruction_types(instruction, definitions)?;
        if instruction.effect.is_barrier() && instruction.safepoint.is_none() {
            return Err(SnapshotError::MissingSafepoint {
                block: block.id.get(),
            });
        }
        if instruction.effect.is_ordered() {
            let sequence = instruction
                .effect_sequence
                .ok_or(SnapshotError::BadEffectOrdering {
                    block: block.id.get(),
                })?;
            if last_effect.is_some_and(|last| sequence <= last) {
                return Err(SnapshotError::BadEffectOrdering {
                    block: block.id.get(),
                });
            }
            last_effect = Some(sequence);
        }
        if instruction.effect.is_barrier()
            && let Some(value) = borrowed
                .iter()
                .copied()
                .find(|value| borrow_live_after(block, *value, index))
        {
            return Err(SnapshotError::BorrowAcrossSafepoint { value: value.get() });
        }
        if let Some(output) = instruction.output
            && output.ty == ValueType::BorrowedView
        {
            borrowed.insert(output.id);
        }
    }
    verify_terminator(block, blocks, definitions, dominators)
}

fn borrow_live_after(block: &Block, value: ValueId, barrier: usize) -> bool {
    block.instructions[barrier..]
        .iter()
        .any(|instruction| instruction.inputs.contains(&value))
        || terminator_values(&block.terminator).contains(&value)
}

fn verify_use(
    value: ValueId,
    block: BlockId,
    position: Option<usize>,
    definitions: &DefinitionMap,
    dominators: &BTreeMap<BlockId, BTreeSet<BlockId>>,
) -> Result<(), SnapshotError> {
    let (defined_block, defined_position, _) = definitions
        .get(&value)
        .ok_or(SnapshotError::UndefinedValue { value: value.get() })?;
    if *defined_block == block {
        if matches!((defined_position, position), (Some(definition), Some(use_at)) if definition >= &use_at)
        {
            return Err(SnapshotError::UseBeforeDefinition { value: value.get() });
        }
    } else if !dominators[&block].contains(defined_block) {
        return Err(SnapshotError::NonDominatingUse {
            value: value.get(),
            block: block.get(),
        });
    }
    Ok(())
}

fn verify_instruction_types(
    instruction: &Instruction,
    definitions: &DefinitionMap,
) -> Result<(), SnapshotError> {
    let types = instruction
        .inputs
        .iter()
        .filter_map(|id| definitions.get(id).map(|definition| definition.2))
        .collect::<Vec<_>>();
    let output = instruction.output.map(|definition| definition.ty);
    let valid = match instruction.kind.semantic() {
        InstructionKind::Constant(_) => instruction.inputs.is_empty() && output.is_some(),
        InstructionKind::Copy => {
            types.len() == 1 && output.is_some_and(|output| Some(output) == types.first().copied())
        }
        InstructionKind::IntegerAdd
        | InstructionKind::IntegerSubtract
        | InstructionKind::IntegerMultiply => {
            types == [ValueType::I64, ValueType::I64] && output == Some(ValueType::I64)
        }
        InstructionKind::IntegerFloorDivide { divisor } => {
            *divisor != 0
                && *divisor != -1
                && types == [ValueType::I64]
                && output == Some(ValueType::I64)
        }
        InstructionKind::IntegerToFloat => {
            types == [ValueType::I64] && output == Some(ValueType::F64)
        }
        InstructionKind::IntegerLessThan | InstructionKind::IntegerCompare { .. } => {
            types == [ValueType::I64, ValueType::I64] && output == Some(ValueType::Bool)
        }
        InstructionKind::FloatAdd
        | InstructionKind::FloatSubtract
        | InstructionKind::FloatMultiply
        | InstructionKind::FloatDivide
        | InstructionKind::FloatPower => {
            types == [ValueType::F64, ValueType::F64] && output == Some(ValueType::F64)
        }
        InstructionKind::FloatCompare { .. } => {
            types == [ValueType::F64, ValueType::F64] && output == Some(ValueType::Bool)
        }
        InstructionKind::IntegerNegate => {
            types == [ValueType::I64] && output == Some(ValueType::I64)
        }
        InstructionKind::FloatNegate => types == [ValueType::F64] && output == Some(ValueType::F64),
        InstructionKind::BooleanNot => {
            types == [ValueType::Bool] && output == Some(ValueType::Bool)
        }
        InstructionKind::BooleanAnd | InstructionKind::BooleanOr => {
            types == [ValueType::Bool, ValueType::Bool] && output == Some(ValueType::Bool)
        }
        InstructionKind::Select => {
            types.len() == 3
                && types[0] == ValueType::Bool
                && types[1] == types[2]
                && output == Some(types[1])
        }
        InstructionKind::BorrowView | InstructionKind::ResolveHandle => {
            types == [ValueType::Handle] && output == Some(ValueType::BorrowedView)
        }
        InstructionKind::ObjectGet | InstructionKind::ListGet => {
            types.first() == Some(&ValueType::Handle) && output.is_some()
        }
        InstructionKind::ListLength => {
            types == [ValueType::Handle] && output == Some(ValueType::I64)
        }
        InstructionKind::ListReversePrefix { element_type } => {
            *element_type == ValueType::I64
                && types == [ValueType::Handle, ValueType::I64]
                && output.is_none()
                && instruction.effect == super::super::ir::Effect::Write
        }
        InstructionKind::ListClear => {
            types == [ValueType::Handle] && output == Some(ValueType::Handle)
        }
        InstructionKind::ObjectSet | InstructionKind::ListSet | InstructionKind::ListAppend => {
            types.first() == Some(&ValueType::Handle)
        }
        InstructionKind::ListInsert => {
            types == [ValueType::Handle, ValueType::I64, ValueType::I64]
                && output == Some(ValueType::Handle)
        }
        InstructionKind::ListPop => {
            types == [ValueType::Handle, ValueType::I64] && output == Some(ValueType::I64)
        }
        InstructionKind::OwnedList {
            element_type,
            copy_from_source,
            ..
        } => {
            matches!(element_type, ValueType::I64 | ValueType::F64)
                && if *copy_from_source {
                    types == [ValueType::I64, ValueType::Handle]
                        && instruction.effect == super::super::ir::Effect::Read
                } else {
                    types == [ValueType::I64]
                        && instruction.effect == super::super::ir::Effect::Pure
                }
                && output == Some(ValueType::Handle)
        }
        InstructionKind::Allocate => {
            instruction.inputs.is_empty() && output == Some(ValueType::Handle)
        }
        InstructionKind::Call { .. }
        | InstructionKind::Guard { .. }
        | InstructionKind::Helper { .. }
        | InstructionKind::BranchGuard { .. }
        | InstructionKind::NestedLoopExit { .. }
        | InstructionKind::LiveProbe
        | InstructionKind::AtPc { .. } => true,
    };
    if valid {
        Ok(())
    } else {
        Err(SnapshotError::TypeMismatch {
            value: instruction.output.map_or(0, |value| value.id.get()),
        })
    }
}

fn verify_terminator(
    block: &Block,
    blocks: &BTreeMap<BlockId, &Block>,
    definitions: &DefinitionMap,
    dominators: &BTreeMap<BlockId, BTreeSet<BlockId>>,
) -> Result<(), SnapshotError> {
    for value in terminator_values(&block.terminator) {
        verify_use(value, block.id, None, definitions, dominators)?;
    }
    if let Terminator::Jump { target, arguments } = &block.terminator {
        let parameters = &blocks[target].parameters;
        if parameters.len() != arguments.len()
            || parameters
                .iter()
                .zip(arguments)
                .any(|(parameter, argument)| definitions[argument].2 != parameter.ty)
        {
            return Err(SnapshotError::InvalidPhi {
                block: target.get(),
            });
        }
    }
    if let Terminator::Branch { yes, no, .. } = &block.terminator
        && (!blocks[yes].parameters.is_empty() || !blocks[no].parameters.is_empty())
    {
        return Err(SnapshotError::InvalidPhi {
            block: if blocks[yes].parameters.is_empty() {
                no.get()
            } else {
                yes.get()
            },
        });
    }
    Ok(())
}

fn terminator_values(terminator: &Terminator) -> Vec<ValueId> {
    match terminator {
        Terminator::Jump { arguments, .. } => arguments.clone(),
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Return { values } | Terminator::SideExit { values, .. } => values.clone(),
        Terminator::Backedge { .. } | Terminator::IrreducibleBackedge => Vec::new(),
    }
}

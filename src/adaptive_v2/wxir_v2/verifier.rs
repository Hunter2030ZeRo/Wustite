use std::collections::{BTreeMap, BTreeSet};

use super::SnapshotError;
use super::dependency::DependencyKind;
use super::ir::{Block, BlockId, InstructionKind, SnapshotBody, ValueId, ValueType};

pub(crate) mod cfg;
mod deopt_roots;
mod values;

pub(super) fn verify(body: &SnapshotBody) -> Result<(), SnapshotError> {
    verify_dependencies(body)?;
    if body.blocks.is_empty() {
        return Err(SnapshotError::EmptyBlocks);
    }
    let blocks = block_map(body)?;
    if !blocks.contains_key(&body.entry) {
        return Err(SnapshotError::MissingEntry);
    }
    let definitions = definitions(body)?;
    let predecessors = cfg::predecessors(body, &blocks)?;
    let dominators = cfg::dominators(body.entry, &blocks, &predecessors);
    for block in &body.blocks {
        values::verify_block(block, &blocks, &definitions, &dominators)?;
    }
    verify_owned_lists(body)?;
    deopt_roots::verify(body, &definitions)?;
    Ok(())
}

fn verify_owned_lists(body: &SnapshotBody) -> Result<(), SnapshotError> {
    let definitions = body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(
            |instruction| match (instruction.kind.semantic(), instruction.output) {
                (InstructionKind::OwnedList { identity, .. }, Some(output)) => {
                    Some((*identity, output.id))
                }
                _ => None,
            },
        )
        .collect::<Vec<_>>();
    if definitions.len() > 2 {
        return Err(SnapshotError::InvalidOwnedList {
            identity: definitions[1].0,
        });
    }
    let Some((identity, definition)) = definitions.first().copied() else {
        return Ok(());
    };
    let mut aliases = BTreeSet::from([definition]);
    loop {
        let before = aliases.len();
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if instruction
                .inputs
                .first()
                .is_some_and(|input| aliases.contains(input))
                && matches!(
                    instruction.kind.semantic(),
                    InstructionKind::Copy
                        | InstructionKind::ListClear
                        | InstructionKind::ListAppend
                        | InstructionKind::ListInsert
                )
                && let Some(output) = instruction.output
            {
                aliases.insert(output.id);
            }
        }
        if aliases.len() == before {
            break;
        }
    }
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if matches!(
            instruction.effect,
            super::ir::Effect::Allocation | super::ir::Effect::Helper | super::ir::Effect::Call
        ) {
            return Err(SnapshotError::InvalidOwnedList { identity });
        }
        for (index, input) in instruction.inputs.iter().enumerate() {
            if !aliases.contains(input) {
                continue;
            }
            let valid = index == 0
                && matches!(
                    instruction.kind.semantic(),
                    InstructionKind::Copy
                        | InstructionKind::ListGet
                        | InstructionKind::ListLength
                        | InstructionKind::ListSet
                        | InstructionKind::ListReversePrefix { .. }
                        | InstructionKind::ListClear
                        | InstructionKind::ListAppend
                        | InstructionKind::ListInsert
                        | InstructionKind::ListPop
                );
            if !valid {
                return Err(SnapshotError::InvalidOwnedList { identity });
            }
        }
    }
    if body.blocks.iter().any(|block| match &block.terminator {
        super::ir::Terminator::Jump { .. } => false,
        super::ir::Terminator::Branch { condition, .. } => aliases.contains(condition),
        super::ir::Terminator::Return { values }
        | super::ir::Terminator::SideExit { values, .. } => {
            values.iter().any(|value| aliases.contains(value))
        }
        super::ir::Terminator::Backedge { .. } | super::ir::Terminator::IrreducibleBackedge => {
            false
        }
    }) {
        return Err(SnapshotError::InvalidOwnedList { identity });
    }
    Ok(())
}

fn verify_dependencies(body: &SnapshotBody) -> Result<(), SnapshotError> {
    let mut kinds = BTreeSet::new();
    for dependency in &body.dependencies {
        if !dependency.is_current() {
            return Err(SnapshotError::StaleDependency {
                kind: dependency.kind,
            });
        }
        if !kinds.insert((dependency.kind, dependency.identity)) {
            return Err(SnapshotError::DanglingDependency);
        }
    }
    let executable = body.dependencies.iter().find(|dependency| {
        dependency.kind == DependencyKind::Executable
            && dependency.identity == body.executable.id
            && dependency.expected_epoch == body.executable.epoch
    });
    let schema = body.dependencies.iter().find(|dependency| {
        dependency.kind == DependencyKind::Schema && dependency.expected_epoch == body.schema_epoch
    });
    if executable.is_none() || schema.is_none() {
        return Err(SnapshotError::DanglingDependency);
    }
    let mut required = BTreeSet::from([
        DependencyKind::Executable,
        DependencyKind::Schema,
        DependencyKind::GcAbi,
        DependencyKind::HelperAbi,
    ]);
    for block in &body.blocks {
        for instruction in &block.instructions {
            match instruction.kind.semantic() {
                InstructionKind::ObjectGet | InstructionKind::ObjectSet => {
                    required.insert(DependencyKind::Shape);
                    required.insert(DependencyKind::Class);
                }
                InstructionKind::ListGet
                | InstructionKind::ListLength
                | InstructionKind::ListSet
                | InstructionKind::ListReversePrefix { .. }
                | InstructionKind::ListClear
                | InstructionKind::ListAppend
                | InstructionKind::ListInsert
                | InstructionKind::ListPop
                | InstructionKind::OwnedList { .. } => {
                    required.insert(DependencyKind::ListLayout);
                }
                InstructionKind::Call { .. } => {
                    required.insert(DependencyKind::Callee);
                }
                InstructionKind::Constant(_)
                | InstructionKind::Copy
                | InstructionKind::IntegerAdd
                | InstructionKind::IntegerSubtract
                | InstructionKind::IntegerMultiply
                | InstructionKind::IntegerFloorDivide { .. }
                | InstructionKind::IntegerToFloat
                | InstructionKind::IntegerLessThan
                | InstructionKind::IntegerCompare { .. }
                | InstructionKind::FloatAdd
                | InstructionKind::FloatSubtract
                | InstructionKind::FloatMultiply
                | InstructionKind::FloatDivide
                | InstructionKind::FloatPower
                | InstructionKind::FloatCompare { .. }
                | InstructionKind::IntegerNegate
                | InstructionKind::FloatNegate
                | InstructionKind::BooleanNot
                | InstructionKind::BooleanAnd
                | InstructionKind::BooleanOr
                | InstructionKind::Select
                | InstructionKind::Guard { .. }
                | InstructionKind::Allocate
                | InstructionKind::Helper { .. }
                | InstructionKind::BranchGuard { .. }
                | InstructionKind::NestedLoopExit { .. }
                | InstructionKind::BorrowView
                | InstructionKind::ResolveHandle
                | InstructionKind::LiveProbe
                | InstructionKind::AtPc { .. } => {}
            }
        }
    }
    for kind in required {
        if !kinds.iter().any(|(present, _)| *present == kind) {
            return Err(SnapshotError::MissingDependency { kind });
        }
    }
    Ok(())
}

fn block_map(body: &SnapshotBody) -> Result<BTreeMap<BlockId, &Block>, SnapshotError> {
    let mut blocks = BTreeMap::new();
    for block in &body.blocks {
        if blocks.insert(block.id, block).is_some() {
            return Err(SnapshotError::DuplicateBlock {
                block: block.id.get(),
            });
        }
    }
    Ok(blocks)
}

pub(super) type DefinitionMap = BTreeMap<ValueId, (BlockId, Option<usize>, ValueType)>;

fn definitions(body: &SnapshotBody) -> Result<DefinitionMap, SnapshotError> {
    let mut values = BTreeMap::new();
    for block in &body.blocks {
        for parameter in &block.parameters {
            define(&mut values, parameter.id, (block.id, None, parameter.ty))?;
        }
        for (index, instruction) in block.instructions.iter().enumerate() {
            if let Some(output) = instruction.output {
                define(&mut values, output.id, (block.id, Some(index), output.ty))?;
            }
        }
    }
    Ok(values)
}

fn define(
    values: &mut DefinitionMap,
    id: ValueId,
    definition: (BlockId, Option<usize>, ValueType),
) -> Result<(), SnapshotError> {
    if values.insert(id, definition).is_some() {
        return Err(SnapshotError::DuplicateDefinition { value: id.get() });
    }
    Ok(())
}

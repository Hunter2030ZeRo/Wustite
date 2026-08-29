use std::collections::BTreeMap;

use super::FusedTraceRequest;
use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity};
use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId, SnapshotDraft, Terminator, ValueDef, ValueType,
};
use crate::bytecode::{CompareOperator, Instruction as WvmInstruction, Register};

mod scalarize;

use scalarize::{Emitter, Node};

struct LoopShape {
    header: usize,
    branch: usize,
    body: usize,
    backedge: usize,
    index: Register,
    total: Register,
}

pub(super) fn recognizes(executable: &crate::executable::ExecutableFunction) -> bool {
    recognize(executable.bytecode().code.as_slice()).is_some()
}

pub(super) fn lower(request: &FusedTraceRequest<'_>) -> Result<Option<SnapshotDraft>, String> {
    if !request.arguments.is_empty() || !request.executable.parameters().is_empty() {
        return Ok(None);
    }
    let Some(shape) = recognize(request.executable.bytecode().code.as_slice()) else {
        return Ok(None);
    };
    let executable = request.executable;
    let identity = ExecutableIdentity::new(executable.id().as_u64(), executable.id().as_u64());
    let mut dependencies = base_dependencies(executable.id().as_u64(), request.facts.schema_epoch);
    let mut emitter = Emitter::new(executable, &mut dependencies);
    let mut entry_values = BTreeMap::new();
    if !emitter.lower_range(
        &executable.bytecode().code[..shape.header],
        0,
        &mut entry_values,
    )? {
        return Ok(None);
    }
    let Some(initial_total) = scalar(&entry_values, shape.total) else {
        return Ok(None);
    };
    let Some(initial_index) = scalar(&entry_values, shape.index) else {
        return Ok(None);
    };
    let entry_instructions = emitter.take_instructions();
    let total_parameter = emitter.next(ValueType::I64)?;
    let index_parameter = emitter.next(ValueType::I64)?;
    let mut header_values = entry_values;
    header_values.insert(shape.total, Node::Scalar(total_parameter));
    header_values.insert(shape.index, Node::Scalar(index_parameter));
    if !emitter.lower_range(
        &executable.bytecode().code[shape.header..shape.branch],
        shape.header,
        &mut header_values,
    )? {
        return Ok(None);
    }
    let WvmInstruction::Branch { cond, .. } = executable.bytecode().code[shape.branch] else {
        return Ok(None);
    };
    let Some(condition) = scalar(&header_values, cond) else {
        return Ok(None);
    };
    if condition.ty != ValueType::Bool {
        return Ok(None);
    }
    let header_instructions = emitter.take_instructions();
    let mut body_values = header_values.clone();
    if !emitter.lower_range(
        &executable.bytecode().code[shape.body..shape.backedge],
        shape.body,
        &mut body_values,
    )? {
        return Ok(None);
    }
    let Some(next_total) = scalar(&body_values, shape.total) else {
        return Ok(None);
    };
    let Some(next_index) = scalar(&body_values, shape.index) else {
        return Ok(None);
    };
    let body_instructions = emitter.take_instructions();
    let blocks = vec![
        Block::new(
            BlockId::new(0),
            Vec::new(),
            entry_instructions,
            Terminator::Jump {
                target: BlockId::new(1),
                arguments: vec![initial_total.id, initial_index.id],
            },
        ),
        Block::new(
            BlockId::new(1),
            vec![total_parameter, index_parameter],
            header_instructions,
            Terminator::Branch {
                condition: condition.id,
                yes: BlockId::new(2),
                no: BlockId::new(3),
            },
        ),
        Block::new(
            BlockId::new(2),
            Vec::new(),
            body_instructions,
            Terminator::Jump {
                target: BlockId::new(1),
                arguments: vec![next_total.id, next_index.id],
            },
        ),
        Block::new(
            BlockId::new(3),
            Vec::new(),
            Vec::new(),
            Terminator::Return {
                values: vec![total_parameter.id],
            },
        ),
    ];
    Ok(Some(
        SnapshotDraft::new(
            identity,
            EntryKind::FunctionEntry,
            BlockId::new(0),
            blocks,
            Vec::new(),
            Vec::new(),
            dependencies,
        )
        .with_schema_epoch(request.permit.schema_epoch()),
    ))
}

fn recognize(code: &[WvmInstruction]) -> Option<LoopShape> {
    let (branch, index, body, exit) = code.iter().enumerate().find_map(|(pc, instruction)| {
        let WvmInstruction::Branch { cond, yes, no } = instruction else {
            return None;
        };
        let WvmInstruction::CompareOp {
            dst,
            op: CompareOperator::Lt,
            lhs,
            ..
        } = pc.checked_sub(1).and_then(|previous| code.get(previous))?
        else {
            return None;
        };
        (*dst == *cond && matches!(code.get(*no), Some(WvmInstruction::Return { .. })))
            .then_some((pc, *lhs, *yes, *no))
    })?;
    let (backedge, header) =
        code[body..exit]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(offset, instruction)| match instruction {
                WvmInstruction::Jump { target } if *target < branch => {
                    Some((body + offset, *target))
                }
                _ => None,
            })?;
    if backedge.saturating_add(1) != exit {
        return None;
    }
    let WvmInstruction::Return { src: total } = code[exit] else {
        return None;
    };
    Some(LoopShape {
        header,
        branch,
        body,
        backedge,
        index,
        total,
    })
}

fn scalar(values: &BTreeMap<Register, Node>, register: Register) -> Option<ValueDef> {
    match values.get(&register) {
        Some(Node::Scalar(value)) => Some(*value),
        Some(Node::Class(_) | Node::Object(_) | Node::None) | None => None,
    }
}

fn base_dependencies(executable: u64, schema_epoch: u64) -> Vec<Dependency> {
    vec![
        Dependency::current(DependencyKind::Executable, executable, executable),
        Dependency::current(DependencyKind::Schema, executable, schema_epoch),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
    ]
}

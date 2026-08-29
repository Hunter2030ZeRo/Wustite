use crate::adaptive_v2::wxir_v2::{SnapshotId, VerifiedSnapshot};

mod scalar_replacement;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum OptimizationPass {
    PropagateAndFold,
    DirectObjectListCall,
    GuardedInline,
    EscapeAndScalarReplace,
    HeapForward,
    LicmAndGvn,
}

impl OptimizationPass {
    pub(crate) const ORDERED: [Self; 6] = [
        Self::PropagateAndFold,
        Self::DirectObjectListCall,
        Self::GuardedInline,
        Self::EscapeAndScalarReplace,
        Self::HeapForward,
        Self::LicmAndGvn,
    ];
}

#[derive(Debug, Clone)]
pub(crate) struct OptimizedSnapshot {
    original: SnapshotId,
    selected: SnapshotId,
    passes: Vec<OptimizationPass>,
    snapshot: VerifiedSnapshot,
    reports: Vec<PassReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PassReport {
    pub(crate) pass: OptimizationPass,
    pub(crate) changed: bool,
    pub(crate) blocked_by_barrier: bool,
}

impl OptimizedSnapshot {
    pub(crate) const fn original_id(&self) -> SnapshotId {
        self.original
    }

    pub(crate) const fn selected_id(&self) -> SnapshotId {
        self.selected
    }

    pub(crate) fn passes(&self) -> &[OptimizationPass] {
        &self.passes
    }

    pub(crate) fn verified(&self) -> &VerifiedSnapshot {
        &self.snapshot
    }

    pub(crate) fn reports(&self) -> &[PassReport] {
        &self.reports
    }
}

#[derive(Debug, Default)]
pub(crate) struct OptimizerPipeline;

#[derive(Debug, Clone)]
pub(crate) struct VerifiedCallee {
    callee: u64,
    snapshot: VerifiedSnapshot,
}

impl VerifiedCallee {
    pub(crate) fn prove_add(
        callee: u64,
        snapshot: &VerifiedSnapshot,
    ) -> Result<Self, super::NativeError> {
        use crate::adaptive_v2::wxir_v2::ir::{InstructionKind, Terminator, ValueType};
        let body = snapshot.body();
        let [block] = body.blocks.as_slice() else {
            return Err(super::NativeError::Unsupported("callee CFG"));
        };
        let [left, right] = block.parameters.as_slice() else {
            return Err(super::NativeError::Unsupported("callee arity"));
        };
        let [instruction] = block.instructions.as_slice() else {
            return Err(super::NativeError::Unsupported("callee body"));
        };
        let Terminator::Return { values } = &block.terminator else {
            return Err(super::NativeError::Unsupported("callee return"));
        };
        if body.executable.id != callee
            || left.ty != ValueType::I64
            || right.ty != ValueType::I64
            || !matches!(instruction.kind.semantic(), InstructionKind::IntegerAdd)
            || instruction.inputs != [left.id, right.id]
            || instruction
                .output
                .is_none_or(|output| output.ty != ValueType::I64 || values != &[output.id])
        {
            return Err(super::NativeError::Unsupported("callee is not add"));
        }
        Ok(Self {
            callee,
            snapshot: snapshot.clone(),
        })
    }
}

impl OptimizerPipeline {
    pub(crate) fn run(
        &self,
        snapshot: &VerifiedSnapshot,
        enabled_count: usize,
    ) -> Result<OptimizedSnapshot, super::NativeError> {
        self.run_with_callees(snapshot, enabled_count, &[])
    }

    pub(crate) fn run_with_callees(
        &self,
        snapshot: &VerifiedSnapshot,
        enabled_count: usize,
        callees: &[VerifiedCallee],
    ) -> Result<OptimizedSnapshot, super::NativeError> {
        if enabled_count > OptimizationPass::ORDERED.len() {
            return Err(super::NativeError::Unsupported("optimizer pass count"));
        }
        let passes = OptimizationPass::ORDERED[..enabled_count].to_vec();
        let mut selected = snapshot.clone();
        let mut reports = Vec::with_capacity(passes.len());
        for pass in &passes {
            let mut body = selected.body().clone();
            let blocked_by_barrier = body
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .any(|instruction| instruction.effect.is_barrier());
            let changed = match pass {
                OptimizationPass::PropagateAndFold => fold_constants(&mut body),
                OptimizationPass::DirectObjectListCall => lower_direct_operations(&mut body),
                OptimizationPass::GuardedInline => guarded_inline(&mut body, callees),
                OptimizationPass::EscapeAndScalarReplace => scalar_replace(&mut body),
                OptimizationPass::HeapForward => heap_forward(&mut body),
                OptimizationPass::LicmAndGvn => {
                    let hoisted = licm(&mut body);
                    gvn(&mut body) || hoisted
                }
            };
            if changed {
                selected = selected
                    .derive_optimized(body)
                    .map_err(|error| super::NativeError::Backend(error.to_string()))?;
            }
            reports.push(PassReport {
                pass: *pass,
                changed,
                blocked_by_barrier,
            });
        }
        Ok(OptimizedSnapshot {
            original: snapshot.id(),
            selected: selected.id(),
            passes,
            snapshot: selected,
            reports,
        })
    }
}

fn lower_direct_operations(body: &mut crate::adaptive_v2::wxir_v2::ir::SnapshotBody) -> bool {
    use crate::adaptive_v2::wxir_v2::ir::InstructionKind;

    let mut changed = false;
    for instruction in body
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
    {
        let InstructionKind::AtPc { operation, .. } = &instruction.kind else {
            continue;
        };
        if matches!(
            operation.semantic(),
            InstructionKind::ObjectGet
                | InstructionKind::ObjectSet
                | InstructionKind::ListGet
                | InstructionKind::ListSet
                | InstructionKind::ListReversePrefix { .. }
                | InstructionKind::ListAppend
                | InstructionKind::ListInsert
                | InstructionKind::ListPop
                | InstructionKind::Call { .. }
        ) {
            instruction.kind = (**operation).clone();
            changed = true;
        }
    }
    changed
}

fn guarded_inline(
    body: &mut crate::adaptive_v2::wxir_v2::ir::SnapshotBody,
    callees: &[VerifiedCallee],
) -> bool {
    use crate::adaptive_v2::wxir_v2::dependency::DependencyKind;
    use crate::adaptive_v2::wxir_v2::ir::InstructionKind;

    let mut changed = false;
    for block in &mut body.blocks {
        let mut guarded = false;
        for instruction in &mut block.instructions {
            match instruction.kind.semantic() {
                InstructionKind::Guard { .. } => guarded = true,
                InstructionKind::Call { callee }
                    if guarded
                        && instruction.inputs.len() == 2
                        && callees.iter().any(|proof| {
                            proof.callee == *callee
                                && body.dependencies.iter().any(|dependency| {
                                    dependency.kind == DependencyKind::Callee
                                        && dependency.identity == *callee
                                        && dependency.expected_epoch
                                            == proof.snapshot.body().executable.epoch
                                })
                        }) =>
                {
                    instruction.kind = InstructionKind::IntegerAdd;
                    changed = true;
                    guarded = false;
                }
                _ if instruction.effect.is_barrier() => guarded = false,
                _ => {}
            }
        }
    }
    changed
}

fn scalar_replace(body: &mut crate::adaptive_v2::wxir_v2::ir::SnapshotBody) -> bool {
    scalar_replacement::run(body)
}

fn heap_forward(body: &mut crate::adaptive_v2::wxir_v2::ir::SnapshotBody) -> bool {
    use crate::adaptive_v2::wxir_v2::ir::{Effect, InstructionKind};

    let value_types = value_types(body);
    let mut changed = false;
    for block in &mut body.blocks {
        let mut fields = std::collections::BTreeMap::new();
        for instruction in &mut block.instructions {
            match instruction.kind.semantic() {
                InstructionKind::ObjectSet if instruction.inputs.len() == 3 => {
                    fields.clear();
                    fields.insert(
                        (instruction.inputs[0], instruction.inputs[1]),
                        instruction.inputs[2],
                    );
                }
                InstructionKind::ObjectGet if instruction.inputs.len() == 2 => {
                    let key = (instruction.inputs[0], instruction.inputs[1]);
                    if let Some(forwarded) = fields.get(&key).copied()
                        && instruction
                            .output
                            .is_some_and(|output| value_types.get(&forwarded) == Some(&output.ty))
                    {
                        instruction.kind = InstructionKind::Copy;
                        instruction.inputs = vec![forwarded];
                        instruction.effect = Effect::Pure;
                        changed = true;
                    }
                }
                _ if instruction.effect.is_barrier()
                    || matches!(instruction.effect, Effect::Write) =>
                {
                    fields.clear()
                }
                _ => {}
            }
        }
    }
    changed
}

fn gvn(body: &mut crate::adaptive_v2::wxir_v2::ir::SnapshotBody) -> bool {
    use crate::adaptive_v2::wxir_v2::ir::InstructionKind;

    let mut changed = false;
    for block in &mut body.blocks {
        let mut adds = std::collections::BTreeMap::new();
        for instruction in &mut block.instructions {
            if instruction.effect.is_barrier() {
                adds.clear();
                continue;
            }
            if matches!(instruction.kind.semantic(), InstructionKind::IntegerAdd)
                && instruction.effect == crate::adaptive_v2::wxir_v2::ir::Effect::Pure
                && instruction.inputs.len() == 2
            {
                let mut key = [instruction.inputs[0], instruction.inputs[1]];
                key.sort();
                if let Some(previous) = adds.get(&key).copied() {
                    instruction.kind = InstructionKind::Copy;
                    instruction.inputs = vec![previous];
                    changed = true;
                } else if let Some(output) = instruction.output {
                    adds.insert(key, output.id);
                }
            }
        }
    }
    changed
}

fn licm(body: &mut crate::adaptive_v2::wxir_v2::ir::SnapshotBody) -> bool {
    use crate::adaptive_v2::wxir_v2::ir::{InstructionKind, RootLocation, Terminator};

    let Some(preheader_index) = body.blocks.iter().position(|block| block.id == body.entry) else {
        return false;
    };
    let Terminator::Jump { target: header, .. } = body.blocks[preheader_index].terminator else {
        return false;
    };
    let Some(latch_index) = body.blocks.iter().position(|block| {
        block.id != body.entry
            && matches!(block.terminator, Terminator::Jump { target, .. } if target == header)
    }) else {
        return false;
    };
    let latch = &body.blocks[latch_index];
    if latch.instructions.iter().any(|instruction| {
        instruction.safepoint.is_some()
            || instruction.effect != crate::adaptive_v2::wxir_v2::ir::Effect::Pure
            || matches!(instruction.kind.semantic(), InstructionKind::Guard { .. })
    }) {
        return false;
    }
    let preheader_values = body.blocks[preheader_index]
        .parameters
        .iter()
        .map(|value| value.id)
        .chain(
            body.blocks[preheader_index]
                .instructions
                .iter()
                .filter_map(|instruction| instruction.output.map(|output| output.id)),
        )
        .collect::<std::collections::BTreeSet<_>>();
    let candidate = latch.instructions.iter().position(|instruction| {
        matches!(
            instruction.kind.semantic(),
            InstructionKind::Constant(_)
                | InstructionKind::Copy
                | InstructionKind::IntegerAdd
                | InstructionKind::IntegerLessThan
        ) && instruction.output.is_some()
            && instruction
                .inputs
                .iter()
                .all(|input| preheader_values.contains(input))
            && instruction.output.is_some_and(|output| {
                !body
                    .root_maps
                    .iter()
                    .any(|map| map.roots.contains(&RootLocation::Ssa(output.id)))
                    && !body
                        .deopts
                        .iter()
                        .any(|recipe| deopt_uses(recipe, output.id))
            })
    });
    let Some(candidate) = candidate else {
        return false;
    };
    let instruction = body.blocks[latch_index].instructions.remove(candidate);
    body.blocks[preheader_index].instructions.push(instruction);
    true
}

fn value_types(
    body: &crate::adaptive_v2::wxir_v2::ir::SnapshotBody,
) -> std::collections::BTreeMap<
    crate::adaptive_v2::wxir_v2::ir::ValueId,
    crate::adaptive_v2::wxir_v2::ir::ValueType,
> {
    body.blocks
        .iter()
        .flat_map(|block| {
            block.parameters.iter().copied().chain(
                block
                    .instructions
                    .iter()
                    .filter_map(|instruction| instruction.output),
            )
        })
        .map(|definition| (definition.id, definition.ty))
        .collect()
}

fn terminator_uses(
    terminator: &crate::adaptive_v2::wxir_v2::ir::Terminator,
    value: crate::adaptive_v2::wxir_v2::ir::ValueId,
) -> bool {
    use crate::adaptive_v2::wxir_v2::ir::Terminator;
    match terminator {
        Terminator::Jump { arguments, .. }
        | Terminator::Return { values: arguments }
        | Terminator::SideExit {
            values: arguments, ..
        } => arguments.contains(&value),
        Terminator::Branch { condition, .. } => *condition == value,
        Terminator::Backedge { .. } | Terminator::IrreducibleBackedge => false,
    }
}

fn deopt_uses(
    recipe: &crate::adaptive_v2::wxir_v2::deopt::DeoptRecipe,
    value: crate::adaptive_v2::wxir_v2::ir::ValueId,
) -> bool {
    use crate::adaptive_v2::wxir_v2::deopt::{RegisterSource, VirtualKind};
    let uses = |source: &RegisterSource| matches!(source, RegisterSource::Ssa(candidate) if *candidate == value);
    recipe
        .frames
        .iter()
        .flat_map(|frame| &frame.registers)
        .any(|register| uses(&register.source))
        || recipe.virtuals.iter().any(|virtual_| match &virtual_.kind {
            VirtualKind::Object { fields, .. } => fields.iter().any(|(_, source)| uses(source)),
            VirtualKind::List { items } | VirtualKind::Tuple { items } => items.iter().any(uses),
        })
}

fn fold_constants(body: &mut crate::adaptive_v2::wxir_v2::ir::SnapshotBody) -> bool {
    use crate::adaptive_v2::wxir_v2::ir::{Constant, InstructionKind};
    use std::collections::BTreeMap;

    let mut changed = false;
    for block in &mut body.blocks {
        let mut constants = BTreeMap::new();
        for instruction in &mut block.instructions {
            match instruction.kind.semantic() {
                InstructionKind::Constant(Constant::Integer(value)) => {
                    if let Some(output) = instruction.output {
                        constants.insert(output.id, *value);
                    }
                }
                InstructionKind::IntegerAdd => {
                    let folded = match instruction.inputs.as_slice() {
                        [left, right] => constants
                            .get(left)
                            .zip(constants.get(right))
                            .and_then(|(left, right)| left.checked_add(*right)),
                        _ => None,
                    };
                    if let Some(value) = folded {
                        instruction.kind = InstructionKind::Constant(Constant::Integer(value));
                        instruction.inputs.clear();
                        if let Some(output) = instruction.output {
                            constants.insert(output.id, value);
                        }
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }
    changed
}

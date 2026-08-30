use super::abi::{
    DIRECT_STORAGE_ABI, DIRECT_STORAGE_MAGIC, NativeDirectStorage, NativeDirectStorageReceipt,
};
use super::{NativeError, NativeValue};
use crate::adaptive_v2::lists::NativeIntegerLease;
use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
use crate::adaptive_v2::wxir_v2::dependency::DependencyKind;
use crate::adaptive_v2::wxir_v2::ir::{
    BlockId, Constant, Effect, InstructionKind, NumericComparison, SnapshotBody, Terminator,
    ValueId, ValueType,
};

pub(super) const MAX_DYNAMIC_DIRECT_STORAGES: usize = 32;
pub(super) const DYNAMIC_STORAGE_INDEX_EMPTY: u8 = u8::MAX;

pub(super) const fn dynamic_storage_slot(alias: u64) -> Option<usize> {
    let generation = (alias >> 32) as u16;
    let reserved = alias >> 48;
    let slot = alias as u32 as usize;
    if generation == 0
        || reserved != 0
        || slot >= crate::adaptive_v2::handles::NATIVE_HANDLE_CAPACITY
    {
        None
    } else {
        Some(slot)
    }
}

#[derive(Debug, Clone)]
pub(super) struct DirectStoragePlan {
    storages: Vec<DirectStorageSlotPlan>,
    aliases: std::collections::BTreeMap<ValueId, usize>,
    dynamic_aliases: std::collections::BTreeSet<ValueId>,
    append_capacity_proofs: Vec<(ValueId, ValueId, usize)>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DirectStorageSlotPlan {
    pub(super) kind: DirectStorageKind,
    pub(super) source: DirectStorageSource,
    pub(super) index_input: Option<usize>,
    pub(super) output: Option<ValueId>,
    pub(super) mutates: bool,
    pub(super) clears: bool,
    pub(super) reserve_hint: usize,
    pub(super) element_type: crate::adaptive_v2::wxir_v2::ir::ValueType,
    pub(super) copy_from: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DirectStorageSource {
    EntryHandle(usize),
    OwnedList { identity: u32 },
}

impl DirectStorageSource {
    pub(super) const fn receipt_identity(self) -> u64 {
        match self {
            Self::EntryHandle(index) => index as u64,
            Self::OwnedList { identity } => (1_u64 << 63) | identity as u64,
        }
    }
}

pub(super) const fn owned_alias(identity: u32) -> u64 {
    (1_u64 << 63) | identity as u64
}

impl DirectStoragePlan {
    pub(super) fn storages(&self) -> &[DirectStorageSlotPlan] {
        &self.storages
    }

    pub(super) fn storage_for(&self, handle: ValueId) -> Option<(usize, DirectStorageSlotPlan)> {
        let index = *self.aliases.get(&handle)?;
        Some((index, self.storages[index]))
    }

    pub(super) fn is_dynamic(&self, handle: ValueId) -> bool {
        self.dynamic_aliases.contains(&handle)
    }

    pub(super) fn has_dynamic(&self) -> bool {
        !self.dynamic_aliases.is_empty()
    }

    pub(super) fn mutates(&self) -> bool {
        self.storages.iter().any(|storage| storage.mutates)
    }

    pub(super) fn entry_storage_count(&self) -> usize {
        self.storages
            .iter()
            .filter(|storage| matches!(storage.source, DirectStorageSource::EntryHandle(_)))
            .count()
    }

    pub(super) fn owned_storage_count(&self) -> usize {
        self.storages
            .iter()
            .filter(|storage| matches!(storage.source, DirectStorageSource::OwnedList { .. }))
            .count()
    }

    pub(super) fn single(&self) -> Option<DirectStorageSlotPlan> {
        (self.storages.len() == 1).then(|| self.storages[0])
    }

    pub(super) fn capacity_extent_for(&self, guard: ValueId) -> Option<usize> {
        self.append_capacity_proofs
            .iter()
            .find_map(|(candidate, _, extent)| (*candidate == guard).then_some(*extent))
    }

    pub(super) fn append_capacity_is_proven(&self, append: ValueId) -> bool {
        self.append_capacity_proofs
            .iter()
            .any(|(_, candidate, _)| *candidate == append)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DirectStorageKind {
    List,
    Object,
}

pub(super) struct DirectStorageLease {
    descriptor: NativeDirectStorage,
    storage: DirectStorageBacking,
}

enum DirectStorageBacking {
    List(NativeIntegerLease),
    Snapshot(std::sync::Arc<[i64]>),
    Transaction(Vec<i64>),
    Object(Box<[i64]>),
}

impl DirectStorageLease {
    pub(super) fn new(
        alias: u64,
        index: i64,
        lease: NativeIntegerLease,
    ) -> Result<Self, NativeError> {
        let index = usize::try_from(index).map_err(|_| NativeError::Helper)?;
        if index >= lease.values().len() || !lease.is_current() {
            return Err(NativeError::StaleDependency);
        }
        let descriptor = NativeDirectStorage {
            magic: DIRECT_STORAGE_MAGIC,
            abi: DIRECT_STORAGE_ABI,
            strategy: 1,
            alias,
            owner: lease.owner().packed_local(),
            layout_epoch: lease.layout_epoch(),
            version: lease.version(),
            length: u64::try_from(lease.values().len()).map_err(|_| NativeError::CountOverflow)?,
            capacity: u64::try_from(lease.values().len())
                .map_err(|_| NativeError::CountOverflow)?,
            values: lease.values().as_ptr().cast_mut(),
        };
        Ok(Self {
            descriptor,
            storage: DirectStorageBacking::List(lease),
        })
    }

    pub(super) fn object(alias: u64, key: i64, value: i64) -> Result<Self, NativeError> {
        let index = usize::try_from(key).map_err(|_| NativeError::Helper)?;
        let mut values =
            vec![0; index.checked_add(1).ok_or(NativeError::CountOverflow)?].into_boxed_slice();
        values[index] = value;
        let descriptor = NativeDirectStorage {
            magic: DIRECT_STORAGE_MAGIC,
            abi: DIRECT_STORAGE_ABI,
            strategy: 1,
            alias,
            owner: alias,
            layout_epoch: 0,
            version: 0,
            length: u64::try_from(values.len()).map_err(|_| NativeError::CountOverflow)?,
            capacity: u64::try_from(values.len()).map_err(|_| NativeError::CountOverflow)?,
            values: values.as_mut_ptr(),
        };
        Ok(Self {
            descriptor,
            storage: DirectStorageBacking::Object(values),
        })
    }

    pub(super) fn snapshot(
        alias: u64,
        index: i64,
        values: std::sync::Arc<[i64]>,
        layout_version: u64,
    ) -> Result<Self, NativeError> {
        let index = usize::try_from(index).map_err(|_| NativeError::Helper)?;
        if index >= values.len() {
            return Err(NativeError::StaleDependency);
        }
        let descriptor = NativeDirectStorage {
            magic: DIRECT_STORAGE_MAGIC,
            abi: DIRECT_STORAGE_ABI,
            strategy: 1,
            alias,
            owner: alias,
            layout_epoch: layout_version,
            version: 0,
            length: u64::try_from(values.len()).map_err(|_| NativeError::CountOverflow)?,
            capacity: u64::try_from(values.len()).map_err(|_| NativeError::CountOverflow)?,
            values: values.as_ptr().cast_mut(),
        };
        Ok(Self {
            descriptor,
            storage: DirectStorageBacking::Snapshot(values),
        })
    }

    pub(super) fn transaction(
        alias: u64,
        mut values: Vec<i64>,
        layout_version: u64,
        reserve: usize,
    ) -> Result<Self, NativeError> {
        values
            .try_reserve(reserve)
            .map_err(|_| NativeError::CountOverflow)?;
        let descriptor = NativeDirectStorage {
            magic: DIRECT_STORAGE_MAGIC,
            abi: DIRECT_STORAGE_ABI,
            strategy: 1,
            alias,
            owner: alias,
            layout_epoch: layout_version,
            version: 0,
            length: u64::try_from(values.len()).map_err(|_| NativeError::CountOverflow)?,
            capacity: u64::try_from(values.capacity()).map_err(|_| NativeError::CountOverflow)?,
            values: values.as_mut_ptr(),
        };
        Ok(Self {
            descriptor,
            storage: DirectStorageBacking::Transaction(values),
        })
    }

    pub(super) fn into_transaction_with_descriptor(
        self,
        descriptor: NativeDirectStorage,
    ) -> Result<Option<Vec<i64>>, NativeError> {
        match self.storage {
            DirectStorageBacking::Transaction(mut values) => {
                let length =
                    usize::try_from(descriptor.length).map_err(|_| NativeError::CountOverflow)?;
                if length > values.capacity() {
                    return Err(NativeError::StaleDependency);
                }
                // SAFETY: native code may initialize only slots below descriptor.length,
                // which is bounded by the retained Vec allocation's capacity.
                unsafe { values.set_len(length) };
                Ok(Some(values))
            }
            _ => Ok(None),
        }
    }

    pub(super) const fn descriptor(&self) -> &NativeDirectStorage {
        &self.descriptor
    }

    pub(super) const fn receipt(&self, source: DirectStorageSource) -> NativeDirectStorageReceipt {
        self.receipt_with_identity(source.receipt_identity())
    }

    pub(super) const fn receipt_with_identity(
        &self,
        storage_identity: u64,
    ) -> NativeDirectStorageReceipt {
        NativeDirectStorageReceipt {
            storage_identity,
            strategy: self.descriptor.strategy,
            reserved: 0,
            alias: self.descriptor.alias,
            owner: self.descriptor.owner,
            layout_epoch: self.descriptor.layout_epoch,
            version: self.descriptor.version,
        }
    }

    pub(super) fn validate_after_call(&self) -> Result<(), NativeError> {
        self.validate_after_call_with_descriptor(&self.descriptor)
    }

    pub(super) fn validate_after_call_with_descriptor(
        &self,
        descriptor: &NativeDirectStorage,
    ) -> Result<(), NativeError> {
        if descriptor.magic != self.descriptor.magic
            || descriptor.abi != self.descriptor.abi
            || descriptor.strategy != self.descriptor.strategy
            || descriptor.alias != self.descriptor.alias
            || descriptor.owner != self.descriptor.owner
            || descriptor.layout_epoch != self.descriptor.layout_epoch
            || descriptor.version != self.descriptor.version
            || descriptor.capacity != self.descriptor.capacity
            || descriptor.values != self.descriptor.values
        {
            return Err(NativeError::StaleDependency);
        }
        match &self.storage {
            DirectStorageBacking::List(lease) => lease
                .is_current()
                .then_some(())
                .ok_or(NativeError::StaleDependency),
            DirectStorageBacking::Snapshot(values) => (!values.is_empty())
                .then_some(())
                .ok_or(NativeError::StaleDependency),
            DirectStorageBacking::Object(values) => (!values.is_empty())
                .then_some(())
                .ok_or(NativeError::StaleDependency),
            DirectStorageBacking::Transaction(values) => {
                let length =
                    usize::try_from(descriptor.length).map_err(|_| NativeError::CountOverflow)?;
                (length <= values.capacity() && descriptor.values == values.as_ptr().cast_mut())
                    .then_some(())
                    .ok_or(NativeError::StaleDependency)
            }
        }
    }
}

fn canonical_append_proofs(
    body: &SnapshotBody,
    storage_for: impl Fn(ValueId) -> Option<usize> + Copy,
) -> Vec<(ValueId, ValueId, usize)> {
    let blocks = body
        .blocks
        .iter()
        .map(|block| (block.id, block))
        .collect::<std::collections::BTreeMap<_, _>>();
    let Ok(predecessors) = crate::adaptive_v2::wxir_v2::verifier::cfg::predecessors(body, &blocks)
    else {
        return Vec::new();
    };
    let dominators =
        crate::adaptive_v2::wxir_v2::verifier::cfg::dominators(body.entry, &blocks, &predecessors);
    let definitions = body
        .blocks
        .iter()
        .flat_map(|block| {
            block
                .instructions
                .iter()
                .enumerate()
                .filter_map(move |(index, instruction)| {
                    instruction
                        .output
                        .map(|output| (output.id, (block.id, index, instruction)))
                })
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let source = |mut value: ValueId| {
        while let Some((_, _, instruction)) = definitions.get(&value)
            && matches!(instruction.kind.semantic(), InstructionKind::Copy)
            && instruction.inputs.len() == 1
        {
            value = instruction.inputs[0];
        }
        value
    };
    let incoming = |target: BlockId, position: usize| {
        body.blocks
            .iter()
            .filter_map(|block| match &block.terminator {
                Terminator::Jump {
                    target: candidate,
                    arguments,
                } if *candidate == target => arguments
                    .get(position)
                    .copied()
                    .map(|value| (block.id, value)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let preheader_source = |value: ValueId| {
        let value = source(value);
        body.blocks
            .iter()
            .find_map(|block| {
                let position = block
                    .parameters
                    .iter()
                    .position(|parameter| parameter.id == value)?;
                let arguments = incoming(block.id, position);
                (arguments.len() == 1).then(|| source(arguments[0].1))
            })
            .unwrap_or(value)
    };
    let integer = |value: ValueId| match definitions
        .get(&preheader_source(value))
        .map(|(_, _, instruction)| instruction.kind.semantic())
    {
        Some(InstructionKind::Constant(Constant::Integer(value))) => Some(*value),
        _ => None,
    };
    let targets = |terminator: &Terminator| match terminator {
        Terminator::Jump { target, .. } => vec![*target],
        Terminator::Branch { yes, no, .. } => vec![*yes, *no],
        Terminator::Return { .. }
        | Terminator::SideExit { .. }
        | Terminator::Backedge { .. }
        | Terminator::IrreducibleBackedge => Vec::new(),
    };
    let mut proofs = Vec::new();
    for header in &body.blocks {
        let Terminator::Branch { condition, yes, .. } = header.terminator else {
            continue;
        };
        let Some((_, _, condition)) = definitions.get(&source(condition)) else {
            continue;
        };
        let comparisons = if matches!(condition.kind.semantic(), InstructionKind::BooleanAnd)
            && condition.inputs.len() == 2
        {
            condition
                .inputs
                .iter()
                .filter_map(|value| definitions.get(&source(*value)).map(|(_, _, value)| *value))
                .collect::<Vec<_>>()
        } else {
            vec![*condition]
        };
        for comparison in comparisons {
            if !matches!(
                comparison.kind.semantic(),
                InstructionKind::IntegerLessThan
                    | InstructionKind::IntegerCompare {
                        comparison: NumericComparison::LessThan
                    }
            ) || comparison.inputs.len() != 2
            {
                continue;
            }
            let index = source(comparison.inputs[0]);
            let Some((position, parameter)) = header
                .parameters
                .iter()
                .enumerate()
                .find(|(_, parameter)| parameter.id == index)
            else {
                continue;
            };
            let loop_source = |value: ValueId| {
                let value = source(value);
                let Some(position) = header
                    .parameters
                    .iter()
                    .position(|parameter| parameter.id == value)
                else {
                    return preheader_source(value);
                };
                let arguments = incoming(header.id, position);
                if arguments.len() == 2
                    && arguments
                        .iter()
                        .any(|(_, candidate)| source(*candidate) == value)
                {
                    arguments
                        .iter()
                        .map(|(_, candidate)| preheader_source(*candidate))
                        .find(|candidate| *candidate != value)
                        .unwrap_or(value)
                } else {
                    value
                }
            };
            let is_step = |value: ValueId| {
                definitions
                    .get(&source(value))
                    .is_some_and(|(_, _, instruction)| {
                        matches!(instruction.kind.semantic(), InstructionKind::IntegerAdd)
                            && instruction.inputs.len() == 2
                            && ((source(instruction.inputs[0]) == parameter.id
                                && integer(loop_source(instruction.inputs[1])) == Some(1))
                                || (source(instruction.inputs[1]) == parameter.id
                                    && integer(loop_source(instruction.inputs[0])) == Some(1)))
                    })
            };
            let index_arguments = incoming(header.id, position);
            if index_arguments.len() != 2
                || index_arguments
                    .iter()
                    .filter(|(_, value)| integer(*value) == Some(0))
                    .count()
                    != 1
                || index_arguments
                    .iter()
                    .filter(|(_, value)| is_step(*value))
                    .count()
                    != 1
            {
                continue;
            }
            let latch = index_arguments
                .iter()
                .find_map(|(block, value)| is_step(*value).then_some(*block))
                .expect("one canonical latch");
            let extent = loop_source(comparison.inputs[1]);
            let Some((_, _, length)) = definitions.get(&extent) else {
                continue;
            };
            if !matches!(length.kind.semantic(), InstructionKind::ListLength)
                || length.inputs.len() != 1
            {
                continue;
            }
            let Some(extent_storage) =
                storage_for(length.inputs[0]).or_else(|| storage_for(source(length.inputs[0])))
            else {
                continue;
            };
            let mut pending = vec![yes];
            let mut iteration = std::collections::BTreeSet::new();
            while let Some(block) = pending.pop() {
                if block == header.id || !iteration.insert(block) {
                    continue;
                }
                let Some(block) = blocks.get(&block) else {
                    iteration.clear();
                    break;
                };
                pending.extend(targets(&block.terminator));
            }
            if !iteration.contains(&latch)
                || iteration.iter().any(|block| {
                    let block = blocks[block];
                    matches!(
                        block.terminator,
                        Terminator::Return { .. }
                            | Terminator::SideExit { .. }
                            | Terminator::Backedge { .. }
                            | Terminator::IrreducibleBackedge
                    ) || targets(&block.terminator)
                        .iter()
                        .filter(|target| **target == header.id)
                        .count()
                        != usize::from(block.id == latch)
                })
            {
                continue;
            }
            let mut appends = Vec::new();
            let mut valid = true;
            for block in &iteration {
                for instruction in &blocks[block].instructions {
                    if instruction.effect.is_barrier()
                        && !(instruction.effect == Effect::Backedge
                            && matches!(instruction.kind.semantic(), InstructionKind::Copy))
                    {
                        valid = false;
                        break;
                    }
                    let mutation = matches!(
                        instruction.kind.semantic(),
                        InstructionKind::ListSet
                            | InstructionKind::ListReversePrefix { .. }
                            | InstructionKind::ListClear
                            | InstructionKind::ListAppend
                            | InstructionKind::ListInsert
                            | InstructionKind::ListPop
                    );
                    if !mutation {
                        continue;
                    }
                    let storage = instruction.inputs.first().and_then(|value| {
                        storage_for(*value).or_else(|| storage_for(source(*value)))
                    });
                    if matches!(instruction.kind.semantic(), InstructionKind::ListAppend) {
                        if let Some(output) = instruction.output {
                            appends.push((*block, output.id, storage));
                        } else {
                            valid = false;
                            break;
                        }
                    } else if storage == Some(extent_storage) {
                        valid = false;
                        break;
                    }
                }
                if !valid {
                    break;
                }
            }
            if !valid {
                continue;
            }
            for target_storage in appends
                .iter()
                .filter_map(|(_, _, storage)| *storage)
                .collect::<std::collections::BTreeSet<_>>()
            {
                let matching = appends
                    .iter()
                    .filter(|(_, _, storage)| *storage == Some(target_storage))
                    .collect::<Vec<_>>();
                if target_storage == extent_storage
                    || matching.len() != 1
                    || !dominators[&latch].contains(&matching[0].0)
                {
                    continue;
                }
                let mut guards = Vec::new();
                for block in &body.blocks {
                    for (index, instruction) in block.instructions.iter().enumerate() {
                        let Some(output) = instruction.output.map(|output| output.id) else {
                            continue;
                        };
                        let storage = match instruction.kind.semantic() {
                            InstructionKind::ListClear => {
                                instruction.inputs.first().and_then(|value| {
                                    storage_for(*value).or_else(|| storage_for(source(*value)))
                                })
                            }
                            InstructionKind::OwnedList {
                                reset_on_definition: true,
                                ..
                            } => storage_for(output),
                            _ => None,
                        };
                        if storage == Some(target_storage)
                            && dominators[&header.id].contains(&block.id)
                        {
                            guards.push((dominators[&block.id].len(), block.id, index, output));
                        }
                    }
                }
                guards.sort_unstable();
                let Some((depth, guard_block, guard_index, guard)) = guards.pop() else {
                    continue;
                };
                if guards.last().is_some_and(|candidate| candidate.0 == depth) {
                    continue;
                }
                let _ = (guard_block, guard_index);
                proofs.push((guard, matching[0].1, extent_storage));
            }
        }
    }
    proofs.sort_unstable();
    proofs.dedup();
    proofs
}

pub(super) fn verify(snapshot: &VerifiedSnapshot) -> Option<DirectStoragePlan> {
    let body = snapshot.body();
    let loop_body = matches!(
        body.entry_kind,
        crate::adaptive_v2::trace::EntryKind::LoopHeader { .. }
    );
    let operations = body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| {
            matches!(
                instruction.kind.semantic(),
                InstructionKind::ListGet
                    | InstructionKind::ListLength
                    | InstructionKind::ListSet
                    | InstructionKind::ListReversePrefix { .. }
                    | InstructionKind::ListClear
                    | InstructionKind::ListAppend
                    | InstructionKind::ListInsert
                    | InstructionKind::ListPop
                    | InstructionKind::ObjectGet
            )
        })
        .collect::<Vec<_>>();
    let first = *operations.first()?;
    let entry = body.blocks.iter().find(|block| block.id == body.entry)?;
    let mut value_types = std::collections::BTreeMap::new();
    for definition in body
        .blocks
        .iter()
        .flat_map(|block| block.parameters.iter())
        .chain(body.blocks.iter().flat_map(|block| {
            block
                .instructions
                .iter()
                .filter_map(|instruction| instruction.output.as_ref())
        }))
    {
        value_types.insert(definition.id, definition.ty);
    }
    let mut identities = value_types
        .keys()
        .copied()
        .map(|value| (value, value))
        .collect::<std::collections::BTreeMap<_, _>>();
    let owned_lists = body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(
            |instruction| match (instruction.kind.semantic(), instruction.output) {
                (
                    InstructionKind::OwnedList {
                        identity,
                        element_type,
                        copy_from_source,
                        ..
                    },
                    Some(output),
                ) => Some((
                    output.id,
                    (
                        *identity,
                        *element_type,
                        copy_from_source
                            .then(|| instruction.inputs.get(1).copied())
                            .flatten(),
                    ),
                )),
                _ => None,
            },
        )
        .collect::<std::collections::BTreeMap<_, _>>();
    if owned_lists.len() > 2 {
        return None;
    }
    let single_owned_entry_root = (!loop_body
        && entry.parameters.is_empty()
        && owned_lists.len() == 1
        && owned_lists
            .values()
            .all(|(_, element_type, _)| *element_type == ValueType::I64)
        && body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| {
                instruction
                    .output
                    .is_some_and(|output| output.ty == ValueType::Handle)
            })
            .all(|instruction| {
                matches!(
                    instruction.kind.semantic(),
                    InstructionKind::OwnedList { .. }
                        | InstructionKind::Copy
                        | InstructionKind::ListClear
                        | InstructionKind::ListAppend
                        | InstructionKind::ListInsert
                )
            }))
    .then(|| *owned_lists.keys().next().expect("one owned list"));
    if body.blocks.len() != 1 && !loop_body && single_owned_entry_root.is_none() {
        return None;
    }
    fn identity_root(
        identities: &std::collections::BTreeMap<ValueId, ValueId>,
        mut value: ValueId,
    ) -> ValueId {
        while let Some(parent) = identities.get(&value).copied() {
            if parent == value {
                break;
            }
            value = parent;
        }
        value
    }
    fn unite_identity(
        identities: &mut std::collections::BTreeMap<ValueId, ValueId>,
        left: ValueId,
        right: ValueId,
    ) -> bool {
        let left = identity_root(identities, left);
        let right = identity_root(identities, right);
        if left == right {
            return false;
        }
        let (root, child) = if left < right {
            (left, right)
        } else {
            (right, left)
        };
        identities.insert(child, root);
        true
    }
    if let Some(owned) = single_owned_entry_root {
        for (value, ty) in &value_types {
            if *ty == ValueType::Handle {
                unite_identity(&mut identities, owned, *value);
            }
        }
    }
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if matches!(
            instruction.kind.semantic(),
            InstructionKind::Copy
                | InstructionKind::ListClear
                | InstructionKind::ListAppend
                | InstructionKind::ListInsert
        ) && instruction.inputs.len() == 1
            || matches!(
                instruction.kind.semantic(),
                InstructionKind::ListClear
                    | InstructionKind::ListAppend
                    | InstructionKind::ListInsert
            ) && instruction.inputs.len() >= 2
        {
            let aliases_handle = instruction.output.is_some_and(|output| {
                output.ty == crate::adaptive_v2::wxir_v2::ir::ValueType::Handle
            }) && value_types.get(&instruction.inputs[0])
                == Some(&crate::adaptive_v2::wxir_v2::ir::ValueType::Handle);
            if aliases_handle {
                unite_identity(
                    &mut identities,
                    instruction.inputs[0],
                    instruction.output.expect("checked handle output").id,
                );
            }
        }
    }
    let incoming_jumps = body
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator {
            Terminator::Jump { target, arguments } => Some((*target, arguments.as_slice())),
            _ => None,
        })
        .collect::<Vec<_>>();
    loop {
        let mut changed = false;
        for block in &body.blocks {
            if block.parameters.is_empty() {
                continue;
            }
            let incoming = incoming_jumps
                .iter()
                .filter(|(target, _)| *target == block.id)
                .map(|(_, arguments)| *arguments)
                .collect::<Vec<_>>();
            if incoming.is_empty()
                || incoming
                    .iter()
                    .any(|arguments| arguments.len() != block.parameters.len())
            {
                continue;
            }
            for (index, parameter) in block.parameters.iter().enumerate() {
                if parameter.ty != crate::adaptive_v2::wxir_v2::ir::ValueType::Handle {
                    continue;
                }
                let parameter_root = identity_root(&identities, parameter.id);
                let mut anchored = None;
                let mut valid = true;
                for arguments in &incoming {
                    let argument_root = identity_root(&identities, arguments[index]);
                    if argument_root == parameter_root {
                        continue;
                    }
                    let anchor = entry
                        .parameters
                        .iter()
                        .filter(|candidate| {
                            candidate.ty == crate::adaptive_v2::wxir_v2::ir::ValueType::Handle
                        })
                        .find(|candidate| identity_root(&identities, candidate.id) == argument_root)
                        .map(|candidate| candidate.id)
                        .or_else(|| {
                            owned_lists.keys().copied().find(|candidate| {
                                identity_root(&identities, *candidate) == argument_root
                            })
                        });
                    let Some(anchor) = anchor else {
                        valid = false;
                        break;
                    };
                    if anchored.is_some_and(|current| current != anchor) {
                        valid = false;
                        break;
                    }
                    anchored = Some(anchor);
                }
                if valid && let Some(anchor) = anchored {
                    changed |= unite_identity(&mut identities, parameter.id, anchor);
                }
            }
        }
        if !changed {
            break;
        }
    }
    let canonical_entry = |value| {
        let root = identity_root(&identities, value);
        entry
            .parameters
            .iter()
            .filter(|candidate| candidate.ty == crate::adaptive_v2::wxir_v2::ir::ValueType::Handle)
            .find(|candidate| identity_root(&identities, candidate.id) == root)
            .map(|candidate| candidate.id)
    };
    let input_index = |value| {
        entry
            .parameters
            .iter()
            .position(|parameter| parameter.id == value)
    };
    let derived_list_roots = body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.kind.semantic(), InstructionKind::ListGet))
        .filter_map(|instruction| instruction.output)
        .filter(|output| output.ty == crate::adaptive_v2::wxir_v2::ir::ValueType::Handle)
        .map(|output| identity_root(&identities, output.id))
        .collect::<std::collections::BTreeSet<_>>();
    let mut storage_roots = Vec::<(ValueId, DirectStorageSource)>::new();
    let mut storage_aliases = std::collections::BTreeMap::new();
    let mut dynamic_aliases = std::collections::BTreeSet::new();
    for operation in &operations {
        let handle = *operation.inputs.first()?;
        let storage = if let Some(storage) = storage_aliases.get(&handle).copied() {
            storage
        } else {
            let anchored = if let Some(canonical_handle) = canonical_entry(handle) {
                Some((
                    canonical_handle,
                    DirectStorageSource::EntryHandle(input_index(canonical_handle)?),
                ))
            } else {
                let root = identity_root(&identities, handle);
                owned_lists
                    .iter()
                    .find(|(definition, _)| identity_root(&identities, **definition) == root)
                    .map(|(definition, (identity, _, _))| {
                        (
                            *definition,
                            DirectStorageSource::OwnedList {
                                identity: *identity,
                            },
                        )
                    })
            };
            let Some((canonical_handle, source)) = anchored else {
                if matches!(
                    operation.kind.semantic(),
                    InstructionKind::ListGet
                        | InstructionKind::ListSet
                        | InstructionKind::ListReversePrefix { .. }
                ) && derived_list_roots.contains(&identity_root(&identities, handle))
                {
                    dynamic_aliases.insert(handle);
                    continue;
                }
                return None;
            };
            if let Some(storage) = storage_aliases.get(&canonical_handle).copied() {
                storage_aliases.insert(handle, storage);
                storage
            } else {
                let storage = storage_roots.len();
                storage_roots.push((canonical_handle, source));
                storage_aliases.insert(canonical_handle, storage);
                storage_aliases.insert(handle, storage);
                storage
            }
        };
        if matches!(
            operation.kind.semantic(),
            InstructionKind::ListClear | InstructionKind::ListAppend | InstructionKind::ListInsert
        ) && let Some(output) = operation.output
        {
            storage_aliases.insert(output.id, storage);
        }
    }
    if storage_roots
        .iter()
        .filter(|(_, source)| matches!(source, DirectStorageSource::EntryHandle(_)))
        .count()
        > 2
        || storage_roots
            .iter()
            .filter(|(_, source)| matches!(source, DirectStorageSource::OwnedList { .. }))
            .count()
            > 2
    {
        return None;
    }
    let original_roots = storage_roots.clone();
    storage_roots.sort_by_key(|(_, source)| match source {
        DirectStorageSource::EntryHandle(index) => (0, *index),
        DirectStorageSource::OwnedList { identity } => (1, *identity as usize),
    });
    for storage in storage_aliases.values_mut() {
        let root = original_roots.get(*storage)?.0;
        *storage = storage_roots
            .iter()
            .position(|candidate| candidate.0 == root)?;
    }
    if body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| {
            if instruction.safepoint.is_some() {
                return !loop_body
                    || instruction.effect != Effect::Backedge
                    || !matches!(instruction.kind.semantic(), InstructionKind::Copy);
            }
            !matches!(
                instruction.effect,
                Effect::Pure | Effect::Read | Effect::Write
            )
        })
    {
        return None;
    }
    let (kind, index_input, output) = match first.kind.semantic() {
        InstructionKind::ListGet
        | InstructionKind::ListLength
        | InstructionKind::ListSet
        | InstructionKind::ListReversePrefix { .. }
        | InstructionKind::ListClear
        | InstructionKind::ListAppend
        | InstructionKind::ListInsert
        | InstructionKind::ListPop
            if body.dependencies.iter().any(|dependency| {
                dependency.kind == DependencyKind::ListLayout && dependency.is_current()
            }) =>
        {
            if operations
                .iter()
                .any(|operation| matches!(operation.kind.semantic(), InstructionKind::ObjectGet))
            {
                return None;
            }
            (DirectStorageKind::List, None, None)
        }
        InstructionKind::ObjectGet
            if !loop_body
                && body.dependencies.iter().any(|dependency| {
                    dependency.kind == DependencyKind::Shape && dependency.is_current()
                }) =>
        {
            if operations.len() != 1 || first.inputs.len() != 2 {
                return None;
            }
            let output = first.output?.id;
            if !matches!(&entry.terminator, Terminator::Return { values } if values == &[output]) {
                return None;
            }
            (
                DirectStorageKind::Object,
                Some(input_index(first.inputs[1])?),
                Some(output),
            )
        }
        _ => return None,
    };
    if kind == DirectStorageKind::Object && storage_roots.len() != 1 {
        return None;
    }
    let reserve_hint = body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match instruction.kind.semantic() {
            InstructionKind::Constant(crate::adaptive_v2::wxir_v2::ir::Constant::Integer(
                value,
            )) => usize::try_from(*value).ok(),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let storages = storage_roots
        .iter()
        .map(|(storage_handle, source)| {
            let storage = *storage_aliases.get(storage_handle)?;
            let value_type = |value| value_types.get(&value).copied();
            let mut element_type = None;
            for operation in &operations {
                if storage_aliases.get(&operation.inputs[0]).copied() != Some(storage) {
                    continue;
                }
                let observed = match operation.kind.semantic() {
                    InstructionKind::ListGet => operation.output.map(|output| output.ty),
                    InstructionKind::ListReversePrefix { element_type } => Some(*element_type),
                    InstructionKind::ListLength | InstructionKind::ListClear => None,
                    InstructionKind::ListSet
                    | InstructionKind::ListAppend
                    | InstructionKind::ListInsert => {
                        operation.inputs.last().and_then(|value| value_type(*value))
                    }
                    InstructionKind::ListPop => operation.output.map(|output| output.ty),
                    InstructionKind::ObjectGet => {
                        Some(crate::adaptive_v2::wxir_v2::ir::ValueType::I64)
                    }
                    _ => None,
                };
                if let Some(observed) = observed {
                    if element_type.is_some_and(|current| current != observed) {
                        return None;
                    }
                    element_type = Some(observed);
                }
            }
            let copy_from = match owned_lists
                .get(storage_handle)
                .and_then(|(_, _, source)| *source)
            {
                Some(source) => {
                    let source_storage = *storage_aliases.get(&source)?;
                    if source_storage == storage
                        || !matches!(
                            storage_roots.get(source_storage)?.1,
                            DirectStorageSource::EntryHandle(_)
                        )
                    {
                        return None;
                    }
                    Some(source_storage)
                }
                None => None,
            };
            Some(DirectStorageSlotPlan {
                kind,
                source: *source,
                index_input,
                output,
                mutates: operations.iter().any(|operation| {
                    storage_aliases.get(&operation.inputs[0]) == storage_aliases.get(storage_handle)
                        && matches!(
                            operation.kind.semantic(),
                            InstructionKind::ListSet
                                | InstructionKind::ListReversePrefix { .. }
                                | InstructionKind::ListClear
                                | InstructionKind::ListAppend
                                | InstructionKind::ListInsert
                                | InstructionKind::ListPop
                        )
                }),
                clears: operations.iter().any(|operation| {
                    storage_aliases.get(&operation.inputs[0]) == storage_aliases.get(storage_handle)
                        && matches!(operation.kind.semantic(), InstructionKind::ListClear)
                }),
                reserve_hint,
                element_type: owned_lists
                    .get(storage_handle)
                    .map_or(element_type?, |(_, element_type, _)| *element_type),
                copy_from,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    if storages.iter().any(|storage| {
        storage.copy_from.is_some_and(|source| {
            source >= storages.len()
                || storages[source].kind != DirectStorageKind::List
                || storages[source].element_type != storage.element_type
        })
    }) {
        return None;
    }
    let append_capacity_proofs =
        canonical_append_proofs(body, |value| storage_aliases.get(&value).copied());
    Some(DirectStoragePlan {
        storages,
        aliases: storage_aliases,
        dynamic_aliases,
        append_capacity_proofs,
    })
}

pub(super) fn integer(value: NativeValue) -> Result<i64, NativeError> {
    match value {
        NativeValue::Integer(value) => Ok(value),
        _ => Err(NativeError::MalformedValue),
    }
}

pub(super) fn alias(value: NativeValue) -> Result<u64, NativeError> {
    match value {
        NativeValue::Handle(alias) => Ok(alias),
        _ => Err(NativeError::MalformedValue),
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::super::context::HelperOperations;
    use super::super::{AdaptiveNativeContext, NativeError, NativeValue};
    use super::DirectStorageLease;
    use crate::adaptive_v2::heap::GcConfig;
    use crate::adaptive_v2::profile::{AdaptiveProfile, FactClass, LiveObservation, ProfileCase};
    use crate::adaptive_v2::trace::EntryKind;
    use crate::adaptive_v2::trace::ExecutableIdentity;
    use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
    use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
    use crate::adaptive_v2::wxir_v2::ir::{
        Block, BlockId, Constant, Effect, Instruction, InstructionKind, NumericComparison,
        SnapshotDraft, Terminator, ValueDef, ValueId, ValueType,
    };

    fn nested_append_body(
        comparison: NumericComparison,
    ) -> crate::adaptive_v2::wxir_v2::ir::SnapshotBody {
        let value = |id, ty| ValueDef::new(ValueId::new(id), ty);
        let instruction = |kind, inputs, id: Option<u32>, ty, effect| {
            Instruction::new(kind, inputs, id.map(|id| value(id, ty)), effect)
        };
        SnapshotDraft::new(
            identity(),
            EntryKind::LoopHeader {
                header_pc: 0,
                backedge_pc: 1,
                preheader: None,
            },
            BlockId::new(0),
            vec![
                Block::new(
                    BlockId::new(0),
                    vec![value(0, ValueType::Handle), value(1, ValueType::Handle)],
                    vec![
                        instruction(
                            InstructionKind::Constant(Constant::Integer(0)),
                            vec![],
                            Some(2),
                            ValueType::I64,
                            Effect::Pure,
                        ),
                        instruction(
                            InstructionKind::Constant(Constant::Integer(1)),
                            vec![],
                            Some(3),
                            ValueType::I64,
                            Effect::Pure,
                        ),
                        instruction(
                            InstructionKind::ListLength,
                            vec![ValueId::new(0)],
                            Some(4),
                            ValueType::I64,
                            Effect::Read,
                        ),
                        instruction(
                            InstructionKind::ListClear,
                            vec![ValueId::new(1)],
                            Some(5),
                            ValueType::Handle,
                            Effect::Write,
                        ),
                    ],
                    Terminator::Jump {
                        target: BlockId::new(1),
                        arguments: vec![ValueId::new(2)],
                    },
                ),
                Block::new(
                    BlockId::new(1),
                    vec![value(6, ValueType::I64)],
                    vec![instruction(
                        InstructionKind::IntegerCompare { comparison },
                        vec![ValueId::new(6), ValueId::new(4)],
                        Some(7),
                        ValueType::Bool,
                        Effect::Pure,
                    )],
                    Terminator::Branch {
                        condition: ValueId::new(7),
                        yes: BlockId::new(2),
                        no: BlockId::new(6),
                    },
                ),
                Block::new(
                    BlockId::new(2),
                    vec![],
                    vec![],
                    Terminator::Jump {
                        target: BlockId::new(3),
                        arguments: vec![ValueId::new(2)],
                    },
                ),
                Block::new(
                    BlockId::new(3),
                    vec![value(8, ValueType::I64)],
                    vec![instruction(
                        InstructionKind::IntegerCompare {
                            comparison: NumericComparison::LessThan,
                        },
                        vec![ValueId::new(8), ValueId::new(3)],
                        Some(9),
                        ValueType::Bool,
                        Effect::Pure,
                    )],
                    Terminator::Branch {
                        condition: ValueId::new(9),
                        yes: BlockId::new(4),
                        no: BlockId::new(5),
                    },
                ),
                Block::new(
                    BlockId::new(4),
                    vec![],
                    vec![instruction(
                        InstructionKind::IntegerAdd,
                        vec![ValueId::new(8), ValueId::new(3)],
                        Some(10),
                        ValueType::I64,
                        Effect::Pure,
                    )],
                    Terminator::Jump {
                        target: BlockId::new(3),
                        arguments: vec![ValueId::new(10)],
                    },
                ),
                Block::new(
                    BlockId::new(5),
                    vec![],
                    vec![
                        instruction(
                            InstructionKind::ListAppend,
                            vec![ValueId::new(5), ValueId::new(6)],
                            Some(11),
                            ValueType::Handle,
                            Effect::Write,
                        ),
                        instruction(
                            InstructionKind::IntegerAdd,
                            vec![ValueId::new(6), ValueId::new(3)],
                            Some(12),
                            ValueType::I64,
                            Effect::Pure,
                        ),
                    ],
                    Terminator::Jump {
                        target: BlockId::new(1),
                        arguments: vec![ValueId::new(12)],
                    },
                ),
                Block::new(
                    BlockId::new(6),
                    vec![],
                    vec![],
                    Terminator::Return { values: vec![] },
                ),
            ],
            vec![],
            vec![],
            vec![],
        )
        .body
    }

    #[test]
    fn nested_append_has_single_capacity_proof() {
        let proofs = super::canonical_append_proofs(
            &nested_append_body(NumericComparison::LessThan),
            |value| match value.get() {
                0 => Some(0),
                1 | 5 | 11 => Some(1),
                _ => None,
            },
        );
        assert_eq!(proofs, vec![(ValueId::new(5), ValueId::new(11), 0)]);
        assert!(
            super::canonical_append_proofs(
                &nested_append_body(NumericComparison::LessEqual),
                |value| match value.get() {
                    0 => Some(0),
                    1 | 5 | 11 => Some(1),
                    _ => None,
                },
            )
            .is_empty()
        );
    }

    #[test]
    fn concurrent_mutation_keeps_segment_alive_invalidates_version() {
        // Given: an owned integer segment lease and another context for the same runtime.
        let mut context = AdaptiveNativeContext::new(GcConfig::default());
        let list = context.allocate_list().expect("list");
        context.append_integer(list, 12).expect("first integer");
        let NativeValue::Handle(alias) = list else {
            panic!("list handle")
        };
        let lease = HelperOperations::native_integer_lease(&mut context, alias)
            .expect("lease lookup")
            .expect("integer lease");
        let native_lease = DirectStorageLease::new(alias, 0, lease.clone()).expect("native lease");
        let mutator = context.clone();

        // When: another thread resizes the source list while the lease remains owned.
        thread::spawn(move || mutator.append_integer(list, 34).expect("concurrent append"))
            .join()
            .expect("mutation thread");

        // Then: the old segment remains readable but its version can no longer authorize a load.
        assert_eq!(lease.values(), &[12]);
        assert!(!lease.is_current());
        assert_eq!(
            native_lease.validate_after_call(),
            Err(NativeError::StaleDependency)
        );
    }

    #[test]
    fn discarded_alias_cannot_reprepare_after_gc() {
        // Given: a rooted list and an owned sidecar lease.
        let mut context = AdaptiveNativeContext::new(GcConfig::default());
        let list = context.allocate_list().expect("list");
        context.append_integer(list, 55).expect("integer");
        let NativeValue::Handle(alias) = list else {
            panic!("list handle")
        };
        let lease = HelperOperations::native_integer_lease(&mut context, alias)
            .expect("lease lookup")
            .expect("integer lease");

        // When: collection runs while rooted, followed by authoritative alias removal.
        context.collect_minor().expect("minor collection");
        assert!(lease.is_current());
        context.discard_value(list).expect("discard alias");

        // Then: the owned bytes remain safe, while stale alias lookup is rejected.
        assert_eq!(lease.values(), &[55]);
        assert!(matches!(
            HelperOperations::native_integer_lease(&mut context, alias),
            Err(NativeError::Helper)
        ));
    }

    fn list_snapshot(instructions: Vec<Instruction>, returned: ValueId) -> VerifiedSnapshot {
        let mut deps = dependencies(7);
        deps.push(Dependency::current(DependencyKind::ListLayout, 9, 1));
        let draft = SnapshotDraft::new(
            identity(),
            EntryKind::FunctionEntry,
            BlockId::new(0),
            vec![Block::new(
                BlockId::new(0),
                vec![
                    ValueDef::new(ValueId::new(0), ValueType::Handle),
                    ValueDef::new(ValueId::new(1), ValueType::Handle),
                    ValueDef::new(ValueId::new(2), ValueType::I64),
                    ValueDef::new(ValueId::new(3), ValueType::I64),
                    ValueDef::new(ValueId::new(6), ValueType::Handle),
                ],
                instructions,
                Terminator::Return {
                    values: vec![returned],
                },
            )],
            vec![],
            vec![],
            deps,
        )
        .with_schema_epoch(7);
        VerifiedSnapshot::seal(draft, compile_permit(7)).expect("verified list snapshot")
    }

    const fn identity() -> ExecutableIdentity {
        ExecutableIdentity::new(9, 3)
    }

    fn dependencies(schema_epoch: u64) -> Vec<Dependency> {
        vec![
            Dependency::current(DependencyKind::Executable, 9, 3),
            Dependency::current(DependencyKind::Schema, 7, schema_epoch),
            Dependency::current(DependencyKind::GcAbi, 0, 1),
            Dependency::current(DependencyKind::HelperAbi, 0, 1),
        ]
    }

    fn compile_permit(schema_epoch: u64) -> crate::adaptive_v2::profile::CompilePermit {
        let mut profile = AdaptiveProfile::new(schema_epoch);
        let observation = LiveObservation::new(ProfileCase::new(1), FactClass::UnknownClassified);
        for _ in 0..64 {
            profile.observe_live(observation);
        }
        let _record = profile.take_record_permit().expect("record permit");
        assert!(profile.finish_recording());
        for _ in 0..32 {
            profile.observe_live(observation);
        }
        profile.take_compile_permit().expect("compile permit")
    }

    #[test]
    fn verifier_accepts_semantic_handle_chain() {
        let snapshot = list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::ListAppend,
                    vec![ValueId::new(0), ValueId::new(3)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListInsert,
                    vec![ValueId::new(4), ValueId::new(2), ValueId::new(3)],
                    Some(ValueDef::new(ValueId::new(5), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(1),
            ],
            ValueId::new(5),
        );
        let plan = super::verify(&snapshot).expect("same-handle direct storage plan");
        assert_eq!(
            plan.single().expect("one storage").source,
            super::DirectStorageSource::EntryHandle(0)
        );
        assert!(plan.mutates());
    }

    #[test]
    fn verifier_accepts_typed_handle_copy_exact_storage_alias() {
        let snapshot = list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::Copy,
                    vec![ValueId::new(0)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::Handle)),
                    Effect::Pure,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListSet,
                    vec![ValueId::new(4), ValueId::new(2), ValueId::new(3)],
                    None,
                    Effect::Write,
                )
                .ordered(1),
            ],
            ValueId::new(0),
        );
        let plan = super::verify(&snapshot).expect("typed Handle Copy storage alias");
        assert_eq!(
            plan.single().expect("one storage").source,
            super::DirectStorageSource::EntryHandle(0)
        );
        assert!(plan.mutates());
    }

    #[test]
    fn verifier_accepts_two_entry_list_storages() {
        let snapshot = list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(0), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::I64)),
                    Effect::Read,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListClear,
                    vec![ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(5), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(1),
                Instruction::new(
                    InstructionKind::ListAppend,
                    vec![ValueId::new(5), ValueId::new(4)],
                    Some(ValueDef::new(ValueId::new(7), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(2),
            ],
            ValueId::new(7),
        );
        let plan = super::verify(&snapshot).expect("bounded two-storage plan");
        assert_eq!(plan.storages().len(), 2);
        assert_eq!(
            plan.storages()[0].source,
            super::DirectStorageSource::EntryHandle(0)
        );
        assert!(!plan.storages()[0].mutates);
        assert_eq!(
            plan.storages()[1].source,
            super::DirectStorageSource::EntryHandle(1)
        );
        assert!(plan.storages()[1].mutates);
    }

    #[test]
    fn verifier_rejects_third_direct_storage_slot() {
        let snapshot = list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(0), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::I64)),
                    Effect::Read,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(1), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(5), ValueType::I64)),
                    Effect::Read,
                )
                .ordered(1),
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(6), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(7), ValueType::I64)),
                    Effect::Read,
                )
                .ordered(2),
            ],
            ValueId::new(7),
        );
        assert!(super::verify(&snapshot).is_none());
    }

    #[test]
    fn verifier_separates_owned_and_entry_storage() {
        // Given: two authoritative entry lists and one nonescaping invocation-owned list.
        let snapshot = list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::OwnedList {
                        identity: 17,
                        element_type: ValueType::I64,
                        reset_on_definition: true,
                        copy_from_source: false,
                    },
                    vec![ValueId::new(3)],
                    Some(ValueDef::new(ValueId::new(8), ValueType::Handle)),
                    Effect::Pure,
                ),
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(0), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::I64)),
                    Effect::Read,
                ),
                Instruction::new(
                    InstructionKind::ListAppend,
                    vec![ValueId::new(8), ValueId::new(4)],
                    Some(ValueDef::new(ValueId::new(9), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListClear,
                    vec![ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(10), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(1),
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(9), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(11), ValueType::I64)),
                    Effect::Read,
                ),
                Instruction::new(
                    InstructionKind::ListAppend,
                    vec![ValueId::new(10), ValueId::new(11)],
                    Some(ValueDef::new(ValueId::new(12), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(2),
            ],
            ValueId::new(12),
        );

        // When: the direct-storage topology is verified.
        let plan = super::verify(&snapshot).expect("two entry storages plus owned intermediate");

        // Then: the owned transaction is distinct and does not consume a third entry slot.
        assert_eq!(plan.entry_storage_count(), 2);
        assert_eq!(plan.owned_storage_count(), 1);

        let mut compiler = super::super::NativeCompiler::new();
        let code = compiler
            .compile_tier1(&snapshot)
            .expect("compile owned intermediate");
        let (outcome, transactions) = code
            .execute_with_integer_storages(
                &[
                    NativeValue::Handle(11),
                    NativeValue::Handle(12),
                    NativeValue::Integer(0),
                    NativeValue::Integer(4),
                    NativeValue::Handle(13),
                ],
                vec![
                    super::super::SnapshotInput::Read(vec![3].into(), 7),
                    super::super::SnapshotInput::Transaction(vec![9], 9),
                ],
            )
            .expect("execute owned intermediate");
        assert_eq!(outcome.values, vec![NativeValue::Handle(12)]);
        assert_eq!(transactions, vec![None, Some(vec![3])]);
        assert_eq!(outcome.counters.machine_entries, 1);
        assert_eq!(outcome.counters.helper_calls, 0);
    }

    #[test]
    fn verifier_rejects_handle_phi_conflicting_entry_roots() {
        let mut deps = dependencies(7);
        deps.push(Dependency::current(DependencyKind::ListLayout, 9, 1));
        let draft = SnapshotDraft::new(
            identity(),
            EntryKind::LoopHeader {
                header_pc: 0,
                backedge_pc: 3,
                preheader: None,
            },
            BlockId::new(0),
            vec![
                Block::new(
                    BlockId::new(0),
                    vec![
                        ValueDef::new(ValueId::new(0), ValueType::Handle),
                        ValueDef::new(ValueId::new(1), ValueType::Handle),
                        ValueDef::new(ValueId::new(2), ValueType::Bool),
                        ValueDef::new(ValueId::new(3), ValueType::I64),
                    ],
                    vec![],
                    Terminator::Branch {
                        condition: ValueId::new(2),
                        yes: BlockId::new(1),
                        no: BlockId::new(2),
                    },
                ),
                Block::new(
                    BlockId::new(1),
                    vec![],
                    vec![],
                    Terminator::Jump {
                        target: BlockId::new(3),
                        arguments: vec![ValueId::new(0)],
                    },
                ),
                Block::new(
                    BlockId::new(2),
                    vec![],
                    vec![],
                    Terminator::Jump {
                        target: BlockId::new(3),
                        arguments: vec![ValueId::new(1)],
                    },
                ),
                Block::new(
                    BlockId::new(3),
                    vec![ValueDef::new(ValueId::new(4), ValueType::Handle)],
                    vec![Instruction::new(
                        InstructionKind::ListGet,
                        vec![ValueId::new(4), ValueId::new(3)],
                        Some(ValueDef::new(ValueId::new(5), ValueType::I64)),
                        Effect::Read,
                    )],
                    Terminator::Return {
                        values: vec![ValueId::new(5)],
                    },
                ),
            ],
            vec![],
            vec![],
            deps,
        )
        .with_schema_epoch(7);
        let snapshot = VerifiedSnapshot::seal(draft, compile_permit(7)).expect("verified phi");
        assert!(super::verify(&snapshot).is_none());
    }

    #[test]
    fn cranelift_reads_one_owned_snapshot_mutates_only_other() {
        let snapshot = list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(0), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::I64)),
                    Effect::Read,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListClear,
                    vec![ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(5), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(1),
                Instruction::new(
                    InstructionKind::ListAppend,
                    vec![ValueId::new(5), ValueId::new(4)],
                    Some(ValueDef::new(ValueId::new(7), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(2),
            ],
            ValueId::new(7),
        );
        let mut compiler = super::super::NativeCompiler::new();
        let code = compiler
            .compile_tier1(&snapshot)
            .expect("compile two storage region");
        let (outcome, transactions) = code
            .execute_with_integer_storages(
                &[
                    NativeValue::Handle(11),
                    NativeValue::Handle(12),
                    NativeValue::Integer(0),
                    NativeValue::Integer(0),
                    NativeValue::Handle(13),
                ],
                vec![
                    super::super::SnapshotInput::Read(vec![3].into(), 7),
                    super::super::SnapshotInput::Transaction(vec![9], 9),
                ],
            )
            .expect("execute two storage region");
        assert_eq!(outcome.values, vec![NativeValue::Handle(12)]);
        assert_eq!(transactions, vec![None, Some(vec![3])]);
        assert_eq!(outcome.counters.machine_entries, 1);
        assert_eq!(outcome.counters.helper_calls, 0);
        assert_eq!(outcome.counters.generic_dispatch_calls, 0);
        assert_eq!(outcome.counters.deopts, 0);
    }

    #[test]
    fn cranelift_rejects_bad_receipts_pre_mutation() {
        let snapshot = list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(0), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::I64)),
                    Effect::Read,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListClear,
                    vec![ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(5), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(1),
                Instruction::new(
                    InstructionKind::ListAppend,
                    vec![ValueId::new(5), ValueId::new(4)],
                    Some(ValueDef::new(ValueId::new(7), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(2),
            ],
            ValueId::new(7),
        );
        let mut compiler = super::super::NativeCompiler::new();
        let code = compiler
            .compile_tier1(&snapshot)
            .expect("compile receipt check");
        let leases = [
            DirectStorageLease::snapshot(11, 0, vec![3].into(), 7).expect("read lease"),
            DirectStorageLease::transaction(12, vec![9], 9, 1).expect("transaction lease"),
        ];
        let plan = code.direct_storage.as_ref().expect("direct plan");
        let base_descriptors = leases
            .iter()
            .map(|lease| *lease.descriptor())
            .collect::<Vec<_>>();
        let base_receipts = leases
            .iter()
            .zip(plan.storages())
            .map(|(lease, storage)| lease.receipt(storage.source))
            .collect::<Vec<_>>();
        let inputs = [
            NativeValue::Handle(11),
            NativeValue::Handle(12),
            NativeValue::Integer(0),
            NativeValue::Integer(0),
            NativeValue::Handle(13),
        ]
        .map(super::super::abi::NativeSlot::from_value);

        let execute =
            |descriptors: *const super::super::abi::NativeDirectStorage,
             receipts: &[super::super::abi::NativeDirectStorageReceipt]| {
                let mut outputs = [
                    super::super::abi::NativeSlot::zero(ValueType::Handle).expect("output slot")
                ];
                let mut frame =
                    super::super::abi::NativeFrame::new(snapshot.id(), &inputs, &mut outputs)
                        .expect("native frame");
                frame.direct_storage = descriptors;
                frame.direct_storage_receipts = receipts.as_ptr();
                code._owner.call(&mut frame)
            };

        assert_eq!(execute(std::ptr::null(), &base_receipts), 2);
        for mutation in 0..6 {
            let mut descriptors = base_descriptors.clone();
            let mut receipts = base_receipts.clone();
            match mutation {
                0 => descriptors[0].strategy ^= 1,
                1 => descriptors[0].alias ^= 1,
                2 => descriptors[0].owner ^= 1,
                3 => descriptors[0].layout_epoch ^= 1,
                4 => descriptors[0].version = descriptors[0].version.wrapping_add(2),
                5 => receipts[0].storage_identity ^= 1,
                _ => unreachable!(),
            }
            let destination_length = descriptors[1].length;
            assert_eq!(execute(descriptors.as_mut_ptr(), &receipts), 2);
            assert_eq!(descriptors[1].length, destination_length);
        }
    }

    #[cfg(feature = "inkwell")]
    #[test]
    fn llvm_reads_one_owned_snapshot_mutates_only_other() {
        let snapshot = list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(0), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::I64)),
                    Effect::Read,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListClear,
                    vec![ValueId::new(1)],
                    Some(ValueDef::new(ValueId::new(5), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(1),
                Instruction::new(
                    InstructionKind::ListAppend,
                    vec![ValueId::new(5), ValueId::new(4)],
                    Some(ValueDef::new(ValueId::new(7), ValueType::Handle)),
                    Effect::Write,
                )
                .ordered(2),
            ],
            ValueId::new(7),
        );
        let inputs = [
            NativeValue::Handle(11),
            NativeValue::Handle(12),
            NativeValue::Integer(0),
            NativeValue::Integer(0),
            NativeValue::Handle(13),
        ];
        let mut compiler = super::super::NativeCompiler::new();
        let tier1 = compiler.compile_tier1(&snapshot).expect("compile tier1");
        let observed = tier1
            .execute_with_integer_storages(
                &inputs,
                vec![
                    super::super::SnapshotInput::Read(vec![3].into(), 7),
                    super::super::SnapshotInput::Transaction(vec![9], 9),
                ],
            )
            .expect("execute tier1")
            .0;
        compiler.observe_tier1(&observed).expect("observe tier1");
        let tier2 = compiler.compile_tier2(&snapshot).expect("compile tier2");
        let (outcome, transactions) = tier2
            .execute_with_integer_storages(
                &inputs,
                vec![
                    super::super::SnapshotInput::Read(vec![3].into(), 7),
                    super::super::SnapshotInput::Transaction(vec![9], 9),
                ],
            )
            .expect("execute tier2");
        assert_eq!(outcome.values, vec![NativeValue::Handle(12)]);
        assert_eq!(transactions, vec![None, Some(vec![3])]);
        assert_eq!(outcome.counters.machine_entries, 1);
        assert_eq!(outcome.counters.helper_calls, 0);

        assert_eq!(
            tier2.execute_with_integer_storages(
                &[
                    NativeValue::Handle(11),
                    NativeValue::Handle(11),
                    NativeValue::Integer(0),
                    NativeValue::Integer(0),
                    NativeValue::Handle(13),
                ],
                vec![
                    super::super::SnapshotInput::Read(vec![3].into(), 7),
                    super::super::SnapshotInput::Transaction(Vec::new(), 9),
                ],
            ),
            Err(NativeError::StaleDependency)
        );
    }

    fn dynamically_derived_list_snapshot() -> VerifiedSnapshot {
        list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(0), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::Handle)),
                    Effect::Read,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(4), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(5), ValueType::I64)),
                    Effect::Read,
                )
                .ordered(1),
            ],
            ValueId::new(5),
        )
    }

    #[cfg(feature = "inkwell")]
    fn dynamically_derived_list_set_snapshot() -> VerifiedSnapshot {
        list_snapshot(
            vec![
                Instruction::new(
                    InstructionKind::ListGet,
                    vec![ValueId::new(0), ValueId::new(2)],
                    Some(ValueDef::new(ValueId::new(4), ValueType::Handle)),
                    Effect::Read,
                )
                .ordered(0),
                Instruction::new(
                    InstructionKind::ListSet,
                    vec![ValueId::new(4), ValueId::new(2), ValueId::new(3)],
                    None,
                    Effect::Write,
                )
                .ordered(1),
            ],
            ValueId::new(4),
        )
    }

    #[cfg(feature = "inkwell")]
    fn execute_malformed_dynamic_set(
        code: &super::super::NativeCode,
        alias: u64,
        version: u64,
        length: u64,
        capacity: u64,
        null_values: bool,
        corrupt_receipt_identity: bool,
    ) -> (u32, i64, u64) {
        use super::super::abi::{
            DIRECT_STORAGE_ABI, DIRECT_STORAGE_MAGIC, NativeDirectStorage,
            NativeDirectStorageReceipt, NativeFrame, NativeSlot,
        };

        let mut root_values = [i64::try_from(alias).expect("local handle fits i64")];
        let mut dynamic_values = [42];
        let dynamic_values_pointer = if null_values {
            std::ptr::null_mut()
        } else {
            dynamic_values.as_mut_ptr()
        };
        let descriptors = [
            NativeDirectStorage {
                magic: DIRECT_STORAGE_MAGIC,
                abi: DIRECT_STORAGE_ABI,
                strategy: 1,
                alias: 11,
                owner: 11,
                layout_epoch: 7,
                version: 0,
                length: 1,
                capacity: 1,
                values: root_values.as_mut_ptr(),
            },
            NativeDirectStorage {
                magic: DIRECT_STORAGE_MAGIC,
                abi: DIRECT_STORAGE_ABI,
                strategy: 1,
                alias,
                owner: alias,
                layout_epoch: 9,
                version,
                length,
                capacity,
                values: dynamic_values_pointer,
            },
        ];
        let receipts = [
            NativeDirectStorageReceipt {
                storage_identity: 0,
                strategy: 1,
                reserved: 0,
                alias: 11,
                owner: 11,
                layout_epoch: 7,
                version: 0,
            },
            NativeDirectStorageReceipt {
                storage_identity: if corrupt_receipt_identity {
                    alias ^ (1_u64 << 32)
                } else {
                    alias
                },
                strategy: 1,
                reserved: 0,
                alias,
                owner: alias,
                layout_epoch: 9,
                version,
            },
        ];
        let mut index = vec![
            super::DYNAMIC_STORAGE_INDEX_EMPTY;
            crate::adaptive_v2::handles::NATIVE_HANDLE_CAPACITY
        ];
        index[super::dynamic_storage_slot(alias).expect("valid dynamic alias")] = 1;
        let inputs = [
            NativeValue::Handle(11),
            NativeValue::Handle(12),
            NativeValue::Integer(0),
            NativeValue::Integer(99),
            NativeValue::Handle(13),
        ]
        .map(NativeSlot::from_value);
        let mut outputs = [NativeSlot::zero(ValueType::Handle).expect("output slot")];
        let mut frame =
            NativeFrame::new(code.snapshot_id(), &inputs, &mut outputs).expect("native frame");
        frame.direct_storage = descriptors.as_ptr();
        frame.direct_storage_receipts = receipts.as_ptr();
        frame.direct_storage_count = 2;
        frame.direct_storage_index = index.as_ptr();

        let raw_exit = code._owner.call(&mut frame);
        (raw_exit, dynamic_values[0], outputs[0].payload)
    }

    #[cfg(feature = "inkwell")]
    fn compile_dynamic_set_backends() -> (super::super::NativeCode, super::super::NativeCode) {
        let snapshot = dynamically_derived_list_set_snapshot();
        let alias = test_alias(77, 3);
        let inputs = [
            NativeValue::Handle(11),
            NativeValue::Handle(12),
            NativeValue::Integer(0),
            NativeValue::Integer(99),
            NativeValue::Handle(13),
        ];
        let mut compiler = super::super::NativeCompiler::new();
        let tier1 = compiler.compile_tier1(&snapshot).expect("compile Tier 1");
        let (tier1_outcome, transactions) = tier1
            .execute_with_storages(
                &inputs,
                vec![super::super::SnapshotInput::Read(
                    vec![i64::try_from(alias).expect("local handle fits i64")].into(),
                    7,
                )],
                vec![super::super::DynamicSnapshotInput {
                    alias,
                    values: vec![42],
                    layout_version: 9,
                    mutates: true,
                }],
            )
            .expect("valid Cranelift dynamic set");
        assert_eq!(transactions, vec![None, Some(vec![99])]);
        compiler
            .observe_tier1(&tier1_outcome)
            .expect("observe Tier 1");
        let tier2 = compiler.compile_tier2(&snapshot).expect("compile Tier 2");
        (tier1, tier2)
    }

    #[cfg(feature = "inkwell")]
    #[test]
    fn llvm_cranelift_reject_dynamic_length_above_capacity_pre_mutation() {
        let alias = test_alias(77, 3);
        let (tier1, tier2) = compile_dynamic_set_backends();

        for code in [&tier1, &tier2] {
            assert_eq!(
                execute_malformed_dynamic_set(code, alias, 0, 1, 0, false, false),
                (2, 42, 0)
            );
        }
    }

    #[cfg(feature = "inkwell")]
    #[test]
    fn llvm_cranelift_reject_null_dynamic_values_pre_mutation() {
        const CHILD: &str = "WUSTITE_LLVM_NULL_DYNAMIC_VALUES_CHILD";
        let alias = test_alias(77, 3);
        let (tier1, tier2) = compile_dynamic_set_backends();

        assert_eq!(
            execute_malformed_dynamic_set(&tier1, alias, 0, 1, 1, true, false),
            (2, 42, 0)
        );
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(
                execute_malformed_dynamic_set(&tier2, alias, 0, 1, 1, true, false),
                (2, 42, 0)
            );
            return;
        }

        let test_name = std::thread::current()
            .name()
            .expect("test thread name")
            .to_owned();
        let child =
            std::process::Command::new(std::env::current_exe().expect("current test binary"))
                .args([test_name.as_str(), "--exact", "--nocapture"])
                .env(CHILD, "1")
                .output()
                .expect("run isolated LLVM null-descriptor case");
        assert!(
            child.status.success(),
            "isolated LLVM null-descriptor case failed: status={} stdout={} stderr={}",
            child.status,
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
    }

    #[cfg(feature = "inkwell")]
    #[test]
    fn llvm_cranelift_reject_in_flight_dynamic_version_pre_mutation() {
        let alias = test_alias(77, 3);
        let (tier1, tier2) = compile_dynamic_set_backends();

        for code in [&tier1, &tier2] {
            assert_eq!(
                execute_malformed_dynamic_set(code, alias, 1, 1, 1, false, false),
                (2, 42, 0)
            );
        }
    }

    #[cfg(feature = "inkwell")]
    #[test]
    fn llvm_cranelift_reject_dynamic_receipt_identity_pre_mutation() {
        let alias = test_alias(77, 3);
        let (tier1, tier2) = compile_dynamic_set_backends();

        for code in [&tier1, &tier2] {
            assert_eq!(
                execute_malformed_dynamic_set(code, alias, 0, 1, 1, false, true),
                (2, 42, 0)
            );
        }
    }

    fn execute_dynamic_list_read(
        code: &super::super::NativeCode,
        derived_alias: u64,
        descriptors: Vec<super::super::DynamicSnapshotInput>,
    ) -> Result<super::super::NativeOutcome, NativeError> {
        code.execute_with_storages(
            &[
                NativeValue::Handle(11),
                NativeValue::Handle(12),
                NativeValue::Integer(0),
                NativeValue::Integer(0),
                NativeValue::Handle(13),
            ],
            vec![super::super::SnapshotInput::Read(
                vec![i64::try_from(derived_alias).expect("local handle fits i64")].into(),
                7,
            )],
            descriptors,
        )
        .map(|(outcome, _)| outcome)
    }

    const fn test_alias(slot: u32, generation: u16) -> u64 {
        (slot as u64) | ((generation as u64) << 32)
    }

    #[test]
    fn dynamic_list_lookup_rejects_unknown_alias_bounded_set_overflow() {
        // Given: native code whose second list is selected by a Handle read from the first.
        let snapshot = dynamically_derived_list_snapshot();
        let code = super::super::NativeCompiler::new()
            .compile_tier1(&snapshot)
            .expect("compile derived lookup");
        let descriptor = |alias| super::super::DynamicSnapshotInput {
            alias,
            values: vec![42],
            layout_version: 9,
            mutates: false,
        };

        // When/Then: a matching descriptor resolves exactly, while an unknown token exits safely.
        let known = test_alias(77, 3);
        let outcome = execute_dynamic_list_read(&code, known, vec![descriptor(known)])
            .expect("known derived storage");
        assert_eq!(outcome.values, vec![NativeValue::Integer(42)]);
        assert_eq!(
            execute_dynamic_list_read(&code, test_alias(78, 3), vec![descriptor(known)]),
            Err(NativeError::InvalidExit(2))
        );
        assert_eq!(
            execute_dynamic_list_read(&code, test_alias(77, 4), vec![descriptor(known)]),
            Err(NativeError::InvalidExit(2))
        );
        assert_eq!(
            execute_dynamic_list_read(
                &code,
                test_alias(
                    crate::adaptive_v2::handles::NATIVE_HANDLE_CAPACITY as u32,
                    3,
                ),
                vec![descriptor(known)],
            ),
            Err(NativeError::InvalidExit(2))
        );
        assert_eq!(
            execute_dynamic_list_read(&code, known, vec![descriptor(1_u64 << 63)]),
            Err(NativeError::StaleDependency)
        );
        assert_eq!(
            execute_dynamic_list_read(
                &code,
                known,
                vec![descriptor(known), descriptor(test_alias(77, 4))],
            ),
            Err(NativeError::StaleDependency)
        );

        // When/Then: exceeding the verified ABI bound is rejected before machine entry.
        let excessive = (0..super::MAX_DYNAMIC_DIRECT_STORAGES)
            .map(|index| descriptor(test_alias(100 + index as u32, 3)))
            .collect();
        assert_eq!(
            execute_dynamic_list_read(&code, test_alias(100, 3), excessive),
            Err(NativeError::CountOverflow)
        );
    }

    #[cfg(feature = "inkwell")]
    #[test]
    fn llvm_cranelift_resolve_same_dynamic_handle_descriptor() {
        // Given: one immutable snapshot with a dynamically derived list Handle.
        let snapshot = dynamically_derived_list_snapshot();
        let mut compiler = super::super::NativeCompiler::new();
        let tier1 = compiler.compile_tier1(&snapshot).expect("compile Tier 1");
        let alias = test_alias(77, 3);
        let descriptor = || super::super::DynamicSnapshotInput {
            alias,
            values: vec![42],
            layout_version: 9,
            mutates: false,
        };

        // When: Cranelift and LLVM execute the exact selected snapshot and descriptor identity.
        let tier1_outcome = execute_dynamic_list_read(&tier1, alias, vec![descriptor()])
            .expect("Cranelift derived read");
        compiler
            .observe_tier1(&tier1_outcome)
            .expect("observe Tier 1");
        let tier2 = compiler.compile_tier2(&snapshot).expect("compile Tier 2");
        let tier2_outcome = execute_dynamic_list_read(&tier2, alias, vec![descriptor()])
            .expect("LLVM derived read");

        // Then: both backends retain snapshot identity and return the same direct value.
        assert_eq!(tier1.snapshot_id(), tier2.snapshot_id());
        assert_eq!(tier1_outcome.snapshot_id(), tier2_outcome.snapshot_id());
        assert_eq!(tier1_outcome.values, vec![NativeValue::Integer(42)]);
        assert_eq!(tier1_outcome.values, tier2_outcome.values);
        assert_eq!(tier2_outcome.counters.helper_calls, 0);
        assert_eq!(tier2_outcome.counters.generic_dispatch_calls, 0);
    }
}

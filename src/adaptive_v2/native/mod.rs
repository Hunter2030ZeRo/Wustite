mod abi;
pub(crate) mod bridge;
pub(crate) mod cache;
mod context;
mod cranelift;
mod direct_storage;
mod entry;
mod helpers;
#[cfg(feature = "inkwell")]
mod llvm;
pub(crate) mod optimizer;

use std::error::Error;
use std::fmt;
use std::rc::Rc;

use self::abi::{NativeFrame, NativeSlot};
use self::context::{HelperContext, HelperOperations};
use self::entry::NativeEntry;
use self::optimizer::{OptimizationPass, OptimizerPipeline};
use crate::adaptive_v2::wxir_v2::ir::WxIrAbi;
use crate::adaptive_v2::wxir_v2::{SnapshotId, VerifiedSnapshot};

pub(crate) type IntegerStorageTransactions = Vec<Option<Vec<i64>>>;

pub(crate) use self::context::AdaptiveNativeContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeValue {
    Integer(i64),
    FloatBits(u64),
    Boolean(bool),
    Handle(u64),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeCounters {
    pub(crate) machine_entries: u64,
    pub(crate) generic_dispatch_calls: u64,
    pub(crate) helper_calls: u64,
    pub(crate) deopts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeOutcome {
    pub(crate) values: Vec<NativeValue>,
    pub(crate) exit_id: u32,
    pub(crate) guard_id: u32,
    pub(crate) safepoint_id: u32,
    pub(crate) deopt_id: u32,
    pub(crate) counters: NativeCounters,
    receipt: NativeReceipt,
}

impl NativeOutcome {
    pub(crate) const fn snapshot_id(&self) -> SnapshotId {
        self.receipt.snapshot
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeReceipt {
    snapshot: SnapshotId,
    tier: NativeTier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeTier {
    Cranelift,
    #[cfg(feature = "inkwell")]
    Llvm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeError {
    AbiMismatch,
    SnapshotMismatch,
    StaleDependency,
    Tier1NotObserved,
    Unsupported(&'static str),
    Backend(String),
    MalformedValue,
    CountOverflow,
    InvalidExit(u32),
    Helper,
}

impl fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Error for NativeError {}

#[derive(Debug, Default)]
pub(crate) struct NativeCompiler {
    tier1_observed: std::collections::BTreeSet<SnapshotId>,
    selected_snapshots: std::collections::BTreeMap<SnapshotId, VerifiedSnapshot>,
}

impl NativeCompiler {
    pub(crate) const fn new() -> Self {
        Self {
            tier1_observed: std::collections::BTreeSet::new(),
            selected_snapshots: std::collections::BTreeMap::new(),
        }
    }

    pub(crate) fn selected_snapshot<'a>(
        &'a self,
        snapshot: &'a VerifiedSnapshot,
    ) -> &'a VerifiedSnapshot {
        self.selected_snapshots
            .get(&snapshot.id())
            .unwrap_or(snapshot)
    }

    pub(crate) fn compile_tier1(
        &mut self,
        snapshot: &VerifiedSnapshot,
    ) -> Result<NativeCode, NativeError> {
        if snapshot.abi() != WxIrAbi::V2 {
            return Err(NativeError::AbiMismatch);
        }
        if snapshot
            .body()
            .dependencies
            .iter()
            .any(|dependency| !dependency.is_current())
        {
            return Err(NativeError::StaleDependency);
        }
        let selected = OptimizerPipeline
            .run(snapshot, OptimizationPass::ORDERED.len())?
            .verified()
            .clone();
        let symbol = tier1_symbol(selected.id());
        let code = cranelift::compile(&selected, &symbol)?;
        self.selected_snapshots
            .insert(snapshot.id(), selected.clone());
        Ok(code)
    }

    pub(crate) fn observe_tier1(&mut self, outcome: &NativeOutcome) -> Result<(), NativeError> {
        if outcome.receipt.tier != NativeTier::Cranelift {
            return Err(NativeError::Tier1NotObserved);
        }
        self.tier1_observed.insert(outcome.receipt.snapshot);
        Ok(())
    }

    #[cfg(feature = "inkwell")]
    pub(crate) fn compile_tier2(
        &mut self,
        snapshot: &VerifiedSnapshot,
    ) -> Result<NativeCode, NativeError> {
        let selected = self.selected_snapshot(snapshot).clone();
        if !self.tier1_observed.contains(&selected.id()) {
            return Err(NativeError::Tier1NotObserved);
        }
        let symbol = tier2_symbol(selected.id());
        llvm::compile(&selected, &symbol)
    }
}

pub(crate) fn snapshot_id_hex(snapshot: SnapshotId) -> String {
    snapshot
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn tier1_symbol(snapshot: SnapshotId) -> String {
    format!("adaptive_v2_t1_{}", snapshot_id_hex(snapshot))
}

#[cfg(feature = "inkwell")]
fn tier2_symbol(snapshot: SnapshotId) -> String {
    format!("adaptive_v2_t2_{}", snapshot_id_hex(snapshot))
}

pub(crate) fn clif_artifact_path(symbol: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("WUSTITE_ADAPTIVE_V2_CLIF_DIR")
        .map(std::path::PathBuf::from)
        .map(|directory| directory.join(format!("{symbol}.clif")))
}

enum NativeOwner {
    Cranelift {
        entry: NativeEntry,
        _module: Box<cranelift_jit::JITModule>,
    },
    #[cfg(feature = "inkwell")]
    Llvm {
        _entry: inkwell::execution_engine::JitFunction<'static, NativeEntry>,
        _context: Box<inkwell::context::Context>,
    },
}

impl NativeOwner {
    fn call(&self, frame: &mut NativeFrame) -> u32 {
        match self {
            Self::Cranelift { entry, .. } => entry::call(*entry, frame),
            #[cfg(feature = "inkwell")]
            Self::Llvm { _entry: entry, .. } => {
                // SAFETY: [Categories 3, 5, 6, 8, 10, and 14] the JitFunction
                // retains its execution engine and has the exact NativeEntry
                // signature; the safe caller validated the live frame buffers.
                unsafe { entry.call(frame) }
            }
        }
    }

    const fn tier(&self) -> NativeTier {
        match self {
            Self::Cranelift { .. } => NativeTier::Cranelift,
            #[cfg(feature = "inkwell")]
            Self::Llvm { .. } => NativeTier::Llvm,
        }
    }
}

pub(crate) struct NativeCode {
    snapshot_id: SnapshotId,
    input_types: Vec<crate::adaptive_v2::wxir_v2::ir::ValueType>,
    output_types: Vec<crate::adaptive_v2::wxir_v2::ir::ValueType>,
    direct_storage: Option<direct_storage::DirectStoragePlan>,
    _owner: NativeOwner,
}

pub(crate) enum SnapshotInput {
    Read(std::sync::Arc<[i64]>, u64),
    Transaction(Vec<i64>, u64),
}

pub(crate) struct DynamicSnapshotInput {
    pub(crate) alias: u64,
    pub(crate) values: Vec<i64>,
    pub(crate) layout_version: u64,
    pub(crate) mutates: bool,
}

impl NativeCode {
    pub(crate) fn accepts_inputs(&self, values: &[NativeValue]) -> bool {
        values.len() == self.input_types.len()
            && values
                .iter()
                .zip(&self.input_types)
                .all(|(value, expected)| value.matches(*expected))
    }

    pub(crate) const fn is_cranelift(&self) -> bool {
        matches!(self._owner, NativeOwner::Cranelift { .. })
    }

    pub(crate) const fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }

    pub(crate) fn direct_storage_inputs(
        &self,
    ) -> Vec<(usize, bool, crate::adaptive_v2::wxir_v2::ir::ValueType)> {
        self.direct_storage
            .as_ref()
            .map(|plan| {
                plan.storages()
                    .iter()
                    .filter_map(|storage| match storage.source {
                        direct_storage::DirectStorageSource::EntryHandle(input) => {
                            Some((input, storage.mutates, storage.element_type))
                        }
                        direct_storage::DirectStorageSource::OwnedList { .. } => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn uses_dynamic_storage(&self) -> bool {
        self.direct_storage
            .as_ref()
            .is_some_and(direct_storage::DirectStoragePlan::has_dynamic)
    }

    pub(crate) fn allows_owned_split_pair(&self) -> bool {
        self.direct_storage.as_ref().is_some_and(|plan| {
            let mut entry_storages = plan.storages().iter().filter(|storage| {
                matches!(
                    storage.source,
                    direct_storage::DirectStorageSource::EntryHandle(_)
                )
            });
            entry_storages.clone().count() == 2
                && entry_storages.all(|storage| storage.mutates && storage.clears)
        })
    }

    pub(crate) fn execute(&self, values: &[NativeValue]) -> Result<NativeOutcome, NativeError> {
        self.execute_with_runtime(values, None)
    }

    pub(crate) fn execute_with_heap(
        &self,
        values: &[NativeValue],
        runtime: &mut NativeRuntime,
    ) -> Result<NativeOutcome, NativeError> {
        self.execute_with_runtime(values, Some(runtime))
    }

    pub(crate) fn execute_with_adaptive_heap(
        &self,
        values: &[NativeValue],
        runtime: &mut AdaptiveNativeContext,
    ) -> Result<NativeOutcome, NativeError> {
        self.execute_with_runtime(values, Some(runtime))
    }

    pub(crate) fn execute_with_integer_storages(
        &self,
        values: &[NativeValue],
        snapshots: Vec<SnapshotInput>,
    ) -> Result<(NativeOutcome, IntegerStorageTransactions), NativeError> {
        self.execute_with_runtime_and_snapshots(values, None, Some(snapshots), None)
    }

    pub(crate) fn execute_with_storages(
        &self,
        values: &[NativeValue],
        snapshots: Vec<SnapshotInput>,
        dynamic_snapshots: Vec<DynamicSnapshotInput>,
    ) -> Result<(NativeOutcome, IntegerStorageTransactions), NativeError> {
        self.execute_with_runtime_and_snapshots(
            values,
            None,
            Some(snapshots),
            Some(dynamic_snapshots),
        )
    }

    fn execute_with_runtime(
        &self,
        values: &[NativeValue],
        runtime: Option<&mut dyn HelperOperations>,
    ) -> Result<NativeOutcome, NativeError> {
        self.execute_with_runtime_and_snapshots(values, runtime, None, None)
            .map(|(outcome, _)| outcome)
    }

    fn execute_with_runtime_and_snapshots(
        &self,
        values: &[NativeValue],
        mut runtime: Option<&mut dyn HelperOperations>,
        snapshots: Option<Vec<SnapshotInput>>,
        dynamic_snapshots: Option<Vec<DynamicSnapshotInput>>,
    ) -> Result<(NativeOutcome, IntegerStorageTransactions), NativeError> {
        if !self.accepts_inputs(values) {
            return Err(NativeError::MalformedValue);
        }
        let inputs = values
            .iter()
            .copied()
            .map(NativeSlot::from_value)
            .collect::<Vec<_>>();
        let mut outputs = self
            .output_types
            .iter()
            .copied()
            .map(NativeSlot::zero)
            .collect::<Result<Vec<_>, _>>()?;
        let mut frame = NativeFrame::new(self.snapshot_id, &inputs, &mut outputs)?;
        let reserve_hint = values
            .iter()
            .filter_map(|value| match value {
                NativeValue::Integer(value) => usize::try_from(*value).ok(),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let snapshot_reserve_hint = snapshots
            .as_ref()
            .into_iter()
            .flatten()
            .map(|snapshot| match snapshot {
                SnapshotInput::Read(values, _) => values.len(),
                SnapshotInput::Transaction(values, _) => values.len(),
            })
            .max()
            .unwrap_or(0);
        let mut snapshots = snapshots.map(Vec::into_iter);
        let mut direct_storage = Vec::new();
        let mut receipt_identities = Vec::new();
        let mut return_transactions = Vec::new();
        let mut storage_aliases = std::collections::BTreeSet::new();
        if let Some(plan) = self.direct_storage.as_ref() {
            for storage in plan.storages() {
                let alias = match storage.source {
                    direct_storage::DirectStorageSource::EntryHandle(input) => {
                        direct_storage::alias(values[input])?
                    }
                    direct_storage::DirectStorageSource::OwnedList { identity } => {
                        direct_storage::owned_alias(identity)
                    }
                };
                if !storage_aliases.insert(alias) {
                    return Err(NativeError::StaleDependency);
                }
                let snapshot = match storage.source {
                    direct_storage::DirectStorageSource::EntryHandle(_) => {
                        snapshots.as_mut().and_then(Iterator::next)
                    }
                    direct_storage::DirectStorageSource::OwnedList { .. } => None,
                };
                direct_storage.push(match storage.kind {
                    direct_storage::DirectStorageKind::List => match snapshot {
                        Some(SnapshotInput::Read(values, layout_version)) => {
                            direct_storage::DirectStorageLease::snapshot(
                                alias,
                                0,
                                values,
                                layout_version,
                            )?
                        }
                        Some(SnapshotInput::Transaction(values, layout_version)) => {
                            direct_storage::DirectStorageLease::transaction(
                                alias,
                                values,
                                layout_version,
                                reserve_hint
                                    .max(snapshot_reserve_hint)
                                    .max(storage.reserve_hint),
                            )?
                        }
                        None => match storage.source {
                            direct_storage::DirectStorageSource::EntryHandle(_) => {
                                let lease = runtime
                                    .as_deref_mut()
                                    .ok_or(NativeError::Helper)?
                                    .native_integer_lease(alias)?
                                    .ok_or(NativeError::Helper)?;
                                direct_storage::DirectStorageLease::new(alias, 0, lease)?
                            }
                            direct_storage::DirectStorageSource::OwnedList { .. } => {
                                direct_storage::DirectStorageLease::transaction(
                                    alias,
                                    Vec::new(),
                                    1,
                                    reserve_hint
                                        .max(snapshot_reserve_hint)
                                        .max(storage.reserve_hint),
                                )?
                            }
                        },
                    },
                    direct_storage::DirectStorageKind::Object => {
                        let index_input = storage.index_input.ok_or(NativeError::MalformedValue)?;
                        let index = direct_storage::integer(values[index_input])?;
                        let value = runtime
                            .as_deref_mut()
                            .ok_or(NativeError::Helper)?
                            .object_get(alias, index)?;
                        direct_storage::DirectStorageLease::object(alias, index, value)?
                    }
                });
                receipt_identities.push(storage.source.receipt_identity());
                return_transactions.push(matches!(
                    storage.source,
                    direct_storage::DirectStorageSource::EntryHandle(_)
                ));
            }
        }
        if snapshots
            .as_mut()
            .is_some_and(|remaining| remaining.next().is_some())
        {
            return Err(NativeError::MalformedValue);
        }
        if let Some(dynamic_snapshots) = dynamic_snapshots {
            if direct_storage.len().saturating_add(dynamic_snapshots.len())
                > direct_storage::MAX_DYNAMIC_DIRECT_STORAGES
            {
                return Err(NativeError::CountOverflow);
            }
            for snapshot in dynamic_snapshots {
                if direct_storage::dynamic_storage_slot(snapshot.alias).is_none() {
                    return Err(NativeError::StaleDependency);
                }
                if !storage_aliases.insert(snapshot.alias) {
                    return Err(NativeError::StaleDependency);
                }
                let alias = snapshot.alias;
                direct_storage.push(if snapshot.mutates {
                    direct_storage::DirectStorageLease::transaction(
                        alias,
                        snapshot.values,
                        snapshot.layout_version,
                        0,
                    )?
                } else {
                    direct_storage::DirectStorageLease::snapshot(
                        alias,
                        0,
                        snapshot.values.into(),
                        snapshot.layout_version,
                    )?
                });
                receipt_identities.push(alias);
                return_transactions.push(true);
            }
        }
        let mut descriptors = direct_storage
            .iter()
            .map(|lease| *lease.descriptor())
            .collect::<Vec<_>>();
        let receipts = direct_storage
            .iter()
            .zip(&receipt_identities)
            .map(|(lease, identity)| lease.receipt_with_identity(*identity))
            .collect::<Vec<_>>();
        let _direct_storage_index = if !descriptors.is_empty() {
            let descriptor_count = descriptors.len();
            let mut index_by_slot = [direct_storage::DYNAMIC_STORAGE_INDEX_EMPTY;
                crate::adaptive_v2::handles::NATIVE_HANDLE_CAPACITY];
            for (index, descriptor) in descriptors.iter().enumerate() {
                let Some(slot) = direct_storage::dynamic_storage_slot(descriptor.alias) else {
                    continue;
                };
                let entry = index_by_slot
                    .get_mut(slot)
                    .ok_or(NativeError::StaleDependency)?;
                if *entry != direct_storage::DYNAMIC_STORAGE_INDEX_EMPTY {
                    return Err(NativeError::StaleDependency);
                }
                *entry = u8::try_from(index).map_err(|_| NativeError::CountOverflow)?;
            }
            frame.direct_storage = descriptors.as_ptr();
            frame.direct_storage_receipts = receipts.as_ptr();
            frame.direct_storage_count =
                u32::try_from(descriptor_count).map_err(|_| NativeError::CountOverflow)?;
            frame.direct_storage_index = index_by_slot.as_ptr();
            Some(index_by_slot)
        } else {
            None
        };
        let mut helper_context = runtime.map(HelperContext::new);
        if let Some(context) = helper_context.as_mut() {
            frame.helper_context = std::ptr::from_mut(context).cast();
        }
        let raw_exit = self._owner.call(&mut frame);
        for (lease, descriptor) in direct_storage.iter().zip(&descriptors) {
            lease.validate_after_call_with_descriptor(descriptor)?;
        }
        let transactions = direct_storage
            .into_iter()
            .zip(descriptors.drain(..))
            .zip(return_transactions)
            .filter_map(|((lease, descriptor), return_transaction)| {
                return_transaction.then_some(lease.into_transaction_with_descriptor(descriptor))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(error) = helper_context
            .as_ref()
            .and_then(|context| context.error().cloned())
        {
            return Err(error);
        }
        if frame.magic != abi::NATIVE_ABI_MAGIC || frame.abi != abi::NATIVE_ABI_VERSION {
            return Err(NativeError::AbiMismatch);
        }
        if frame.snapshot_id != self.snapshot_id.as_bytes() {
            return Err(NativeError::SnapshotMismatch);
        }
        if raw_exit > 1 {
            return Err(NativeError::InvalidExit(raw_exit));
        }
        let values = outputs
            .into_iter()
            .map(NativeSlot::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((
            NativeOutcome {
                values,
                exit_id: frame.exit_id,
                guard_id: frame.guard_id,
                safepoint_id: frame.safepoint_id,
                deopt_id: frame.deopt_id,
                counters: NativeCounters {
                    machine_entries: frame.machine_entries,
                    generic_dispatch_calls: frame.generic_dispatch_calls,
                    helper_calls: frame.helper_calls,
                    deopts: frame.deopts,
                },
                receipt: NativeReceipt {
                    snapshot: self.snapshot_id,
                    tier: self._owner.tier(),
                },
            },
            transactions,
        ))
    }
}

#[derive(Debug, Default)]
pub(crate) struct NativeRuntime {
    objects: std::collections::BTreeMap<u64, std::collections::BTreeMap<i64, i64>>,
    lists: std::collections::BTreeMap<u64, Vec<i64>>,
    calls: std::collections::BTreeMap<u64, fn(i64, i64) -> i64>,
}

pub(crate) struct CachedNativeExecutor {
    compiler: NativeCompiler,
    cache: cache::SharedCodeCache<Rc<NativeCode>>,
}

impl CachedNativeExecutor {
    pub(crate) fn new(max_count: usize, max_bytes: usize) -> Self {
        Self {
            compiler: NativeCompiler::new(),
            cache: cache::SharedCodeCache::new(max_count, max_bytes),
        }
    }

    pub(crate) fn execute_tier1(
        &mut self,
        snapshot: &VerifiedSnapshot,
        values: &[NativeValue],
    ) -> Result<NativeOutcome, NativeError> {
        let key = cache::CacheKey::new(
            snapshot.id(),
            &snapshot.body().dependencies,
            cache::NativeTier::Cranelift,
            snapshot.id(),
        );
        let lease = if let Some(lease) = self.cache.lease(key) {
            lease
        } else {
            let code = self.compiler.compile_tier1(snapshot)?;
            self.cache.insert_and_lease(
                key,
                estimated_code_bytes(snapshot),
                snapshot.body().schema_epoch,
                Rc::new(code),
            )
        };
        let code = lease
            .cloned()
            .ok_or_else(|| NativeError::Backend("cache lease invalidated".into()))?;
        let outcome = code.execute(values)?;
        self.compiler.observe_tier1(&outcome)?;
        Ok(outcome)
    }

    #[cfg(feature = "inkwell")]
    pub(crate) fn execute_tier2(
        &mut self,
        snapshot: &VerifiedSnapshot,
        values: &[NativeValue],
    ) -> Result<NativeOutcome, NativeError> {
        let selected = self.compiler.selected_snapshot(snapshot).clone();
        let key = cache::CacheKey::new(
            selected.id(),
            &selected.body().dependencies,
            cache::NativeTier::Llvm,
            selected.id(),
        );
        let lease = if let Some(lease) = self.cache.lease(key) {
            lease
        } else {
            let code = self.compiler.compile_tier2(&selected)?;
            self.cache.insert_and_lease(
                key,
                estimated_code_bytes(&selected),
                selected.body().schema_epoch,
                Rc::new(code),
            )
        };
        lease
            .cloned()
            .ok_or_else(|| NativeError::Backend("cache lease invalidated".into()))?
            .execute(values)
    }

    pub(crate) fn invalidate(&self, snapshot: SnapshotId) {
        self.cache.invalidate_snapshot(snapshot);
        for (original, selected) in &self.compiler.selected_snapshots {
            if *original != snapshot && selected.id() != snapshot {
                continue;
            }
            self.cache.invalidate_snapshot(*original);
            self.cache.invalidate_snapshot(selected.id());
        }
    }

    pub(crate) fn cached_tiers(&self, snapshot: &VerifiedSnapshot) -> (bool, bool) {
        let tier1 = cache::CacheKey::new(
            snapshot.id(),
            &snapshot.body().dependencies,
            cache::NativeTier::Cranelift,
            snapshot.id(),
        );
        #[cfg(feature = "inkwell")]
        let selected = self.compiler.selected_snapshot(snapshot);
        #[cfg(feature = "inkwell")]
        let tier2 = cache::CacheKey::new(
            selected.id(),
            &selected.body().dependencies,
            cache::NativeTier::Llvm,
            selected.id(),
        );
        (self.cache.contains(tier1), {
            #[cfg(feature = "inkwell")]
            {
                self.cache.contains(tier2)
            }
            #[cfg(not(feature = "inkwell"))]
            {
                false
            }
        })
    }
}

fn estimated_code_bytes(snapshot: &VerifiedSnapshot) -> usize {
    snapshot
        .body()
        .blocks
        .iter()
        .map(|block| {
            block
                .instructions
                .len()
                .saturating_mul(64)
                .saturating_add(64)
        })
        .sum()
}

impl NativeRuntime {
    pub(crate) fn insert_object(
        &mut self,
        handle: u64,
        fields: impl IntoIterator<Item = (i64, i64)>,
    ) {
        self.objects.insert(handle, fields.into_iter().collect());
    }

    pub(crate) fn insert_list(&mut self, handle: u64, values: Vec<i64>) {
        self.lists.insert(handle, values);
    }

    pub(crate) fn insert_call(&mut self, callee: u64, function: fn(i64, i64) -> i64) {
        self.calls.insert(callee, function);
    }
}

impl NativeValue {
    const fn matches(self, expected: crate::adaptive_v2::wxir_v2::ir::ValueType) -> bool {
        matches!(
            (self, expected),
            (
                Self::Integer(_),
                crate::adaptive_v2::wxir_v2::ir::ValueType::I64
            ) | (
                Self::FloatBits(_),
                crate::adaptive_v2::wxir_v2::ir::ValueType::F64
            ) | (
                Self::Boolean(_),
                crate::adaptive_v2::wxir_v2::ir::ValueType::Bool
            ) | (
                Self::Handle(_),
                crate::adaptive_v2::wxir_v2::ir::ValueType::Handle
            )
        )
    }
}

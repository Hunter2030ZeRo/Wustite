use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::executable::{ExecutableFunction, ExecutableId};
use crate::jit::CompilerBackend;
use crate::runtime::{AdaptiveRegionReport, AdaptiveReport};
use crate::value::Value;

use super::handles::{NATIVE_HANDLE_CAPACITY, RuntimeId, StableHandle, StableHandleTable};
use super::heap::GcConfig;
use super::native::{
    AdaptiveNativeContext, DynamicSnapshotInput, NativeCode, NativeError, NativeOutcome,
    NativeValue, SnapshotInput,
};
use super::profile::{AdaptiveProfile, Lifecycle};
use super::public_heap::runtime::{AdaptiveHeapRuntime, RootedValue};
use super::value_word::ScalarValue;
use super::wxir_v2::VerifiedSnapshot;
use super::wxir_v2::ir::SnapshotDraft;

mod entry_trace;
mod loop_osr;
mod loop_snapshot;
mod object_osr;
mod observation;
mod tiered;
#[cfg(test)]
pub(crate) use tiered::bridge::{BridgeSite, execute_cached as execute_cached_bridge};

pub(crate) use loop_osr::LoopExecution;

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);

// Synchronization order is site/shard state, compiler, bridge registry, code cache, then report.
// Registry maps are held only long enough to clone an Arc, and generated/helper code runs after
// every state, compiler, bridge, cache, and report guard has been released.
pub(crate) struct AdaptiveVm {
    functions: Mutex<HashMap<ExecutableId, Arc<Mutex<FunctionState>>>>,
    report: Mutex<AdaptiveReport>,
    backend: Option<CompilerBackend>,
    loops: loop_osr::LoopOsr,
    heap: AdaptiveHeapRuntime,
    object_sites: Arc<object_osr::ObjectSites>,
    objects: Mutex<HashMap<u64, Arc<ObjectShard>>>,
    native_handles: Mutex<StableHandleTable<crate::object::ObjectRef>>,
    runtime_id: u64,
}

struct NativeHandleScope<'a> {
    table: std::sync::MutexGuard<'a, StableHandleTable<crate::object::ObjectRef>>,
    references: HashMap<crate::object::ObjectRef, StableHandle>,
    live: Vec<StableHandle>,
}

impl<'a> NativeHandleScope<'a> {
    fn new(table: std::sync::MutexGuard<'a, StableHandleTable<crate::object::ObjectRef>>) -> Self {
        Self {
            table,
            references: HashMap::new(),
            live: Vec::new(),
        }
    }

    fn encode(&mut self, reference: crate::object::ObjectRef) -> Result<u64, String> {
        if let Some(handle) = self.references.get(&reference) {
            return Ok(handle.packed_local());
        }
        let handle = self
            .table
            .allocate(reference)
            .map_err(|error| error.to_string())?;
        self.references.insert(reference, handle);
        self.live.push(handle);
        Ok(handle.packed_local())
    }

    fn resolve(&self, packed: u64) -> Result<crate::object::ObjectRef, String> {
        self.table
            .resolve(self.table.local_handle(packed))
            .copied()
            .map_err(|error| error.to_string())
    }
}

impl Drop for NativeHandleScope<'_> {
    fn drop(&mut self) {
        for handle in self.live.drain(..) {
            let _ = self.table.release(handle);
        }
    }
}

struct FunctionState {
    profile: AdaptiveProfile,
    observation: Option<observation::ClassifiedObservation>,
    native: Option<Arc<tiered::TieredSite>>,
    draft: Option<SnapshotDraft>,
    supported: bool,
}

struct ObjectShard {
    state: Mutex<Option<ObjectShardState>>,
}

struct ObjectShardState {
    osr: object_osr::ObjectOsr,
    report: AdaptiveReport,
}

struct ObjectShardLease<'a> {
    shard: &'a ObjectShard,
    state: Option<ObjectShardState>,
}

impl ObjectShard {
    fn checkout(&self) -> Option<ObjectShardLease<'_>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()?;
        Some(ObjectShardLease {
            shard: self,
            state: Some(state),
        })
    }

    fn snapshot_report(&self) -> Option<AdaptiveReport> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|state| state.report.clone())
    }

    fn root_binding(
        &self,
        reference: crate::object::ObjectRef,
    ) -> Option<crate::adaptive_v2::public_heap::runtime::RootedValue> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|state| state.osr.root_binding(reference))
    }
}

impl std::ops::Deref for ObjectShardLease<'_> {
    type Target = ObjectShardState;

    fn deref(&self) -> &Self::Target {
        self.state.as_ref().expect("checked-out object shard")
    }
}

impl std::ops::DerefMut for ObjectShardLease<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.state.as_mut().expect("checked-out object shard")
    }
}

impl Drop for ObjectShardLease<'_> {
    fn drop(&mut self) {
        *self
            .shard
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.state.take();
    }
}

pub(crate) struct ObjectTicket {
    shard: Arc<ObjectShard>,
    ticket: object_osr::ObjectTicket,
}

impl ObjectTicket {
    pub(crate) fn output(&self) -> Option<(u16, Value)> {
        self.ticket.output()
    }
}

pub(super) struct SharedTier1Code {
    code: NativeCode,
    resume_pcs: BTreeMap<u32, usize>,
}

impl SharedTier1Code {
    pub(crate) fn new(code: NativeCode) -> Result<Self, String> {
        if code.is_cranelift() {
            Ok(Self {
                code,
                resume_pcs: BTreeMap::new(),
            })
        } else {
            Err("shared adaptive-v2 code must be Cranelift tier-1".to_owned())
        }
    }

    pub(super) fn execute_with_adaptive_heap(
        &self,
        values: &[NativeValue],
        runtime: &mut AdaptiveNativeContext,
    ) -> Result<NativeOutcome, NativeError> {
        self.code.execute_with_adaptive_heap(values, runtime)
    }

    pub(super) fn direct_storage_inputs(
        &self,
    ) -> Vec<(usize, bool, super::wxir_v2::ir::ValueType)> {
        self.code.direct_storage_inputs()
    }

    pub(super) fn uses_dynamic_storage(&self) -> bool {
        self.code.uses_dynamic_storage()
    }

    pub(super) fn accepts_inputs(&self, values: &[NativeValue]) -> bool {
        self.code.accepts_inputs(values)
    }

    pub(super) fn allows_owned_split_pair(&self) -> bool {
        self.code.allows_owned_split_pair()
    }

    pub(super) fn execute_with_integer_storages(
        &self,
        values: &[NativeValue],
        snapshots: Vec<SnapshotInput>,
    ) -> Result<(NativeOutcome, super::native::IntegerStorageTransactions), NativeError> {
        self.code.execute_with_integer_storages(values, snapshots)
    }

    pub(super) fn execute_with_storages(
        &self,
        values: &[NativeValue],
        snapshots: Vec<SnapshotInput>,
        dynamic_snapshots: Vec<DynamicSnapshotInput>,
    ) -> Result<(NativeOutcome, super::native::IntegerStorageTransactions), NativeError> {
        self.code
            .execute_with_storages(values, snapshots, dynamic_snapshots)
    }

    fn with_loop_resumes(mut self, snapshot: &VerifiedSnapshot) -> Self {
        self.resume_pcs = snapshot
            .body()
            .deopts
            .iter()
            .filter_map(|recipe| {
                usize::try_from(recipe.resume_pc)
                    .ok()
                    .map(|pc| (recipe.id, pc))
            })
            .collect();
        self
    }

    fn resume_pc(&self, exit: u32) -> Option<usize> {
        self.resume_pcs.get(&exit).copied()
    }
}

// SAFETY: SharedTier1Code can only be constructed after checking that NativeCode owns a
// Cranelift JITModule. LLVM/Inkwell owners are rejected and cannot enter this type.
unsafe impl Send for SharedTier1Code {}
// SAFETY: after compilation the Cranelift module is retained only to keep the immutable entry
// point alive. Each invocation owns its NativeFrame and helper context, and does not mutate the
// module or generated code.
unsafe impl Sync for SharedTier1Code {}

impl AdaptiveVm {
    pub(crate) fn new(backend: Option<CompilerBackend>) -> Self {
        let runtime_id = NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed);
        let heap = AdaptiveHeapRuntime::new(GcConfig::default());
        Self {
            functions: Mutex::new(HashMap::new()),
            report: Mutex::new(AdaptiveReport::new()),
            backend,
            loops: loop_osr::LoopOsr::new(backend, runtime_id),
            heap,
            object_sites: Arc::new(object_osr::ObjectSites::new()),
            objects: Mutex::new(HashMap::new()),
            native_handles: Mutex::new(StableHandleTable::new(
                RuntimeId::new(runtime_id),
                NATIVE_HANDLE_CAPACITY,
            )),
            runtime_id,
        }
    }

    pub(crate) fn report(&self) -> AdaptiveReport {
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let loop_report = self.loops.report();
        let loop_native_success = loop_report.machine_entries > 0
            && loop_report.compile_failure.is_none()
            && loop_report.deopts == 0;
        merge_report(&mut report, loop_report);
        if loop_native_success {
            report.compile_failure = None;
        }
        let shards = self
            .objects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for shard in shards {
            if let Some(shard_report) = shard.snapshot_report() {
                merge_report(&mut report, shard_report);
            }
        }
        report
    }

    fn sync_heap_metrics(&self) {
        let metrics = self.heap.heap_metrics();
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        report.gc_allocations = metrics.allocations;
        report.gc_minor_collections = metrics.minor_collections;
        report.gc_major_collections = metrics.major_collections;
        report.gc_promotions = metrics.promotions;
        report.gc_bytes = metrics.allocated_bytes;
        report.gc_pause_micros = metrics.pause_micros;
    }

    pub(crate) fn root_public_value(
        &self,
        execution_id: u64,
        value: crate::runtime::RuntimeValue,
    ) -> Result<Option<RootedValue>, String> {
        if let crate::runtime::RuntimeValue::Object(reference) = value {
            let shard = self
                .objects
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&execution_id)
                .cloned();
            if let Some(shard) = shard {
                let rooted = shard.root_binding(reference);
                self.finish_public_execution(execution_id);
                return Ok(rooted);
            }
            return Ok(None);
        }
        let scalar = match value {
            crate::runtime::RuntimeValue::SmallInt(value) => Some(ScalarValue::Integer(value)),
            crate::runtime::RuntimeValue::Float(value) => {
                Some(ScalarValue::FloatBits(value.to_bits()))
            }
            crate::runtime::RuntimeValue::Bool(_) | crate::runtime::RuntimeValue::None => None,
            crate::runtime::RuntimeValue::Object(_) => unreachable!("handled above"),
        };
        let rooted = scalar
            .map(|value| self.heap.scalar(value).map_err(|error| error.to_string()))
            .transpose()?;
        self.finish_public_execution(execution_id);
        self.sync_heap_metrics();
        Ok(rooted)
    }

    fn finish_public_execution(&self, execution_id: u64) {
        let shard = self
            .objects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&execution_id);
        if let Some(shard_report) = shard.and_then(|shard| shard.snapshot_report()) {
            merge_report(
                &mut self
                    .report
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                shard_report,
            );
        }
    }

    pub(crate) fn collect_public_heap(&self) -> Result<(), String> {
        self.heap
            .collect_major()
            .map_err(|error| error.to_string())?;
        self.sync_heap_metrics();
        Ok(())
    }

    pub(crate) fn validate_public_root(&self, value: &RootedValue) -> Result<(), String> {
        self.heap
            .root(value.value())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn object_before(
        &self,
        execution_id: u64,
        executable: &ExecutableFunction,
        pc: usize,
        instruction: &crate::bytecode::Instruction,
        registers: &[Value],
        heap: &mut crate::object::ObjectHeap,
    ) -> Option<ObjectTicket> {
        let block = executable.structure_map().block_by_pc(pc)?;
        if matches!(
            instruction,
            crate::bytecode::Instruction::GetItem { .. }
                | crate::bytecode::Instruction::SetItem { .. }
                | crate::bytecode::Instruction::ListAppend { .. }
                | crate::bytecode::Instruction::ListInsert { .. }
                | crate::bytecode::Instruction::ListPop { .. }
        ) && executable
            .structure_map()
            .regions()
            .iter()
            .any(|region| region.blocks.contains(&block.id))
        {
            return None;
        }
        let shard = self
            .objects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(execution_id)
            .or_insert_with(|| {
                Arc::new(ObjectShard {
                    state: Mutex::new(Some(ObjectShardState {
                        osr: object_osr::ObjectOsr::new(
                            self.heap.clone(),
                            self.backend,
                            Arc::clone(&self.object_sites),
                        ),
                        report: AdaptiveReport::new(),
                    })),
                })
            })
            .clone();
        let mut state = shard.checkout()?;
        let ObjectShardState { osr, report } = &mut *state;
        let ticket = osr
            .before(executable, pc, instruction, registers, heap, report)
            .filter(object_osr::ObjectTicket::handled);
        drop(state);
        self.sync_heap_metrics();
        ticket.map(|ticket| ObjectTicket { shard, ticket })
    }

    pub(crate) fn object_after(&self, ticket: ObjectTicket, registers: &[Value]) {
        let Some(mut state) = ticket.shard.checkout() else {
            return;
        };
        let ObjectShardState { osr, report } = &mut *state;
        osr.after(ticket.ticket, registers, report);
        drop(state);
        self.sync_heap_metrics();
    }

    pub(crate) fn try_execute_entry(
        &self,
        execution_id: u64,
        executable: &ExecutableFunction,
        arguments: &[Value],
        heap: &mut crate::object::ObjectHeap,
    ) -> Option<Result<Value, String>> {
        let id = executable.id();
        let runtime_id = self.runtime_id;
        let state = self
            .functions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(id)
            .or_insert_with(|| {
                Arc::new(Mutex::new(FunctionState {
                    profile: AdaptiveProfile::new(id.as_u64()),
                    observation: None,
                    native: None,
                    draft: None,
                    supported: true,
                }))
            })
            .clone();
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.supported
            || arguments
                .iter()
                .any(|value| matches!(value, Value::Uninitialized))
        {
            let mut report = self
                .report
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            report.guest_calls = report.guest_calls.saturating_add(1);
            sync_region(&mut report, id, &state, "unsupported WVM entry");
            return None;
        }
        if let Some(native) = state.native.clone() {
            drop(state);
            let mut delta = AdaptiveReport::new();
            let result = if arguments
                .iter()
                .any(|value| matches!(value, Value::Object(_)))
            {
                self.execute_object_entry(
                    execution_id,
                    executable,
                    &native,
                    arguments,
                    heap,
                    &mut delta,
                )
            } else {
                execute_tiered(&native, arguments, &mut delta)
            };
            let mut report = self
                .report
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            merge_report(&mut report, delta);
            return result;
        }
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let observation = *state
            .observation
            .get_or_insert_with(|| observation::entry(executable, arguments));
        state.profile.observe_live(observation.live);
        report.readiness.live = report.readiness.live.saturating_add(1);
        report.static_fact_matches = report
            .static_fact_matches
            .saturating_add(observation.static_facts);
        match state.profile.lifecycle() {
            Lifecycle::ReadyToRecord => {
                if let Some(permit) = state.profile.take_record_permit() {
                    match entry_trace::record_entry(executable, arguments, permit) {
                        Ok(draft) => {
                            state.draft = Some(draft);
                            report.traces = report.traces.saturating_add(1);
                            let _ = state.profile.finish_recording();
                        }
                        Err(error) => {
                            state.supported = false;
                            report.compile_failure = Some(error);
                        }
                    }
                }
            }
            Lifecycle::ReadyToCompile => {
                compile_entry(&mut state, self.backend, runtime_id, &mut report);
            }
            Lifecycle::Profiling | Lifecycle::Recording | Lifecycle::Compiled => {}
        }
        sync_region(
            &mut report,
            id,
            &state,
            "collecting live entry observations",
        );
        None
    }

    fn execute_object_entry(
        &self,
        execution_id: u64,
        executable: &ExecutableFunction,
        native: &tiered::TieredSite,
        arguments: &[Value],
        heap: &mut crate::object::ObjectHeap,
        report: &mut AdaptiveReport,
    ) -> Option<Result<Value, String>> {
        let shard = self
            .objects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&execution_id)
            .cloned()?;
        let mut state = shard.checkout()?;
        let inputs = match state.osr.entry_inputs(executable, arguments, heap) {
            Ok(inputs) => inputs,
            Err(_) => return None,
        };
        let result = execute_tiered_inputs(native, &inputs, report);
        if let Err(error) = state.osr.finish_entry(arguments, heap) {
            return Some(Err(error));
        }
        result
    }

    pub(crate) fn try_execute_loop(
        &self,
        executable: &ExecutableFunction,
        region: crate::structure_map::RegionId,
        registers: &[Value],
        heap: &mut crate::object::ObjectHeap,
    ) -> Option<Result<LoopExecution, String>> {
        self.try_execute_loop_mode(executable, region, registers, heap, None)
    }

    pub(crate) fn try_execute_preheader_loop(
        &self,
        executable: &ExecutableFunction,
        edge_pc: usize,
        body_pc: usize,
        registers: &[Value],
        heap: &mut crate::object::ObjectHeap,
    ) -> Option<Result<LoopExecution, String>> {
        let mut candidates =
            executable
                .structure_map()
                .loop_regions()
                .filter_map(|(region_id, region)| {
                    let preheader = loop_snapshot::verified_preheader_entry(executable, region_id)?;
                    preheader
                        .matches(edge_pc, body_pc)
                        .then_some((region_id, region, preheader))
                });
        let (region_id, region, preheader) = candidates.next()?;
        if candidates.next().is_some() || !loop_will_enter_body(executable, region, registers) {
            return None;
        }
        self.try_execute_loop_mode(executable, region_id, registers, heap, Some(preheader))
    }

    fn try_execute_loop_mode(
        &self,
        executable: &ExecutableFunction,
        region: crate::structure_map::RegionId,
        registers: &[Value],
        heap: &mut crate::object::ObjectHeap,
        required_preheader: Option<crate::adaptive_v2::trace::LoopPreheader>,
    ) -> Option<Result<LoopExecution, String>> {
        let region_description = executable.structure_map().region(region)?;
        let prepared = prepare_nested_loop_inputs(executable, region_description, registers, heap);
        if let Some(preheader) = required_preheader
            && let Some(result) = self.loops.try_execute_compiled_preheader(
                executable,
                region,
                registers,
                prepared.as_ref(),
                preheader,
                |native, inputs, region, report| {
                    self.execute_object_loop(
                        native,
                        inputs,
                        executable,
                        (region, prepared.as_ref()),
                        (heap, report),
                        true,
                    )
                },
            )
        {
            return Some(result);
        }
        let append_targets = region_description
            .blocks
            .iter()
            .filter_map(|block_id| executable.structure_map().block(*block_id))
            .flat_map(|block| &executable.bytecode().code[block.start_pc..block.end_pc])
            .filter_map(|instruction| match instruction {
                crate::bytecode::Instruction::ListAppend { list, .. } => Some(*list),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut element_types = BTreeMap::new();
        let mut element_paths = BTreeMap::new();
        let mut indexed_element_types = BTreeMap::new();
        let mut storage_cases = Vec::new();
        let mut missing_owned_destinations = Vec::new();
        let mut empty_append_case = None;
        let mut call_targets = BTreeMap::new();
        let mut constant_call_targets = loop_snapshot::ConstantCallTargets::new();
        let mut pending_functions = vec![executable];
        let mut seen_functions = std::collections::BTreeSet::new();
        while let Some(function) = pending_functions.pop() {
            if !seen_functions.insert(function.id().as_u64()) {
                continue;
            }
            for instruction in &function.bytecode().code {
                let crate::bytecode::Instruction::LoadConstant { dst, constant } = instruction
                else {
                    continue;
                };
                let Some(crate::executable::ExecutableConstant::Function(target)) =
                    function.constants().get(constant.0)
                else {
                    continue;
                };
                let reference = heap.function_reference(target)?;
                constant_call_targets.insert(
                    (function.id().as_u64(), *dst),
                    loop_snapshot::CallTarget {
                        function: (**target).clone(),
                        handle: reference.slot(),
                        argument_element_paths: Vec::new(),
                        argument_indexed_element_types: Vec::new(),
                    },
                );
                pending_functions.push(target.as_ref());
            }
        }
        let storage_live = loop_snapshot::storage_live_destinations(executable, region);
        for register in region_description
            .entry_summary
            .iter()
            .map(|slot| slot.register)
            .chain(storage_live.iter().copied())
            .chain(
                prepared
                    .iter()
                    .flat_map(|prepared| prepared.values.iter().map(|(register, _)| *register)),
            )
        {
            let observed = prepared
                .iter()
                .flat_map(|prepared| &prepared.values)
                .find_map(|(candidate, value)| (*candidate == register).then_some(value))
                .or_else(|| registers.get(usize::from(register)));
            let Some(Value::Object(reference)) = observed else {
                if required_preheader.is_some() && storage_live.contains(&register) {
                    missing_owned_destinations.push((storage_cases.len(), register));
                    storage_cases.push(u32::from(register) << 8);
                }
                continue;
            };
            if let Some(path) = sequence_element_path(heap, *reference, 0) {
                element_paths.insert(register, path);
            }
            let mut indexed = BTreeMap::new();
            sequence_indexed_element_types(heap, *reference, &mut Vec::new(), &mut indexed, 0);
            if !indexed.is_empty() {
                indexed_element_types.insert(register, indexed);
            }
            let Ok(object) = heap.get(*reference) else {
                continue;
            };
            let crate::object::Object::List(list) = object else {
                if let crate::object::Object::Function(function) = object {
                    let identity = function.id().as_u64();
                    storage_cases.push((u32::from(register) << 24) | 0x00f0_0000);
                    storage_cases.push(identity as u32);
                    storage_cases.push((identity >> 32) as u32);
                    storage_cases.push(reference.slot());
                    storage_cases.push(reference.generation());
                    storage_cases.push(reference.heap_id() as u32);
                    storage_cases.push((reference.heap_id() >> 32) as u32);
                    call_targets.insert(
                        register,
                        loop_snapshot::CallTarget {
                            function: function.clone(),
                            handle: reference.slot(),
                            argument_element_paths: Vec::new(),
                            argument_indexed_element_types: Vec::new(),
                        },
                    );
                }
                continue;
            };
            let (tag, ty) = match list.strategy() {
                crate::object::SequenceStrategy::I64 => {
                    (1, Some(crate::adaptive_v2::wxir_v2::ir::ValueType::I64))
                }
                crate::object::SequenceStrategy::F64 => {
                    (2, Some(crate::adaptive_v2::wxir_v2::ir::ValueType::F64))
                }
                crate::object::SequenceStrategy::Empty if append_targets.contains(&register) => {
                    if empty_append_case
                        .replace((storage_cases.len(), register))
                        .is_some()
                    {
                        return None;
                    }
                    (0, None)
                }
                crate::object::SequenceStrategy::Empty => (5, None),
                crate::object::SequenceStrategy::Bool => (3, None),
                crate::object::SequenceStrategy::Object => (4, None),
            };
            storage_cases.push((u32::from(register) << 8) | tag);
            if let Some(ty) = ty {
                element_types.insert(register, ty);
            }
        }
        for instruction in &executable.bytecode().code {
            match instruction {
                crate::bytecode::Instruction::Move { dst, src } => {
                    if let Some(path) = element_paths.get(src).cloned() {
                        element_paths.insert(*dst, path);
                    }
                }
                crate::bytecode::Instruction::GetItem { dst, object, .. } => {
                    let Some(path) = element_paths.get(object).cloned() else {
                        continue;
                    };
                    if let Some(element_type) = path.first().copied() {
                        element_types.insert(*object, element_type);
                    }
                    if path.first() == Some(&super::wxir_v2::ir::ValueType::Handle)
                        && path.len() > 1
                    {
                        element_paths.insert(*dst, path[1..].to_vec());
                    }
                }
                _ => {}
            }
        }
        let index_constants = executable
            .bytecode()
            .code
            .iter()
            .filter_map(|instruction| match instruction {
                crate::bytecode::Instruction::ConstSmallInt { dst, value }
                | crate::bytecode::Instruction::ConstI64 { dst, value } => {
                    usize::try_from(*value).ok().map(|value| (*dst, value))
                }
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let mut indexed_paths = indexed_element_types
            .keys()
            .copied()
            .map(|register| (register, (register, Vec::new())))
            .collect::<BTreeMap<_, (crate::bytecode::Register, Vec<Option<usize>>)>>();
        for instruction in &executable.bytecode().code {
            match instruction {
                crate::bytecode::Instruction::Move { dst, src } => {
                    if let Some(path) = indexed_paths.get(src).cloned() {
                        indexed_paths.insert(*dst, path);
                    }
                }
                crate::bytecode::Instruction::GetItem { dst, object, key } => {
                    let Some((root, mut path)) = indexed_paths.get(object).cloned() else {
                        continue;
                    };
                    path.push(index_constants.get(key).copied());
                    let Some(types) = indexed_element_types.get(&root) else {
                        continue;
                    };
                    if let Some(ty) = indexed_sequence_type(types, &path) {
                        element_types.insert(*dst, ty);
                        if ty == super::wxir_v2::ir::ValueType::Handle {
                            indexed_paths.insert(*dst, (root, path));
                        }
                    }
                }
                _ => {}
            }
        }
        for (register, path) in &element_paths {
            for (depth, element_type) in path.iter().enumerate() {
                let tag = match element_type {
                    super::wxir_v2::ir::ValueType::I64 => 1,
                    super::wxir_v2::ir::ValueType::F64 => 2,
                    super::wxir_v2::ir::ValueType::Bool => 3,
                    super::wxir_v2::ir::ValueType::Handle => 4,
                    super::wxir_v2::ir::ValueType::BorrowedView => return None,
                };
                storage_cases.push(
                    0x8000_0000
                        | (u32::from(*register) << 16)
                        | (u32::try_from(depth).ok()? << 8)
                        | tag,
                );
            }
        }
        for block_id in &region_description.blocks {
            let Some(block) = executable.structure_map().block(*block_id) else {
                continue;
            };
            for (offset, instruction) in executable.bytecode().code[block.start_pc..block.end_pc]
                .iter()
                .enumerate()
            {
                let crate::bytecode::Instruction::LoadConstant { dst, constant } = instruction
                else {
                    continue;
                };
                let Some(crate::executable::ExecutableConstant::Function(expected)) =
                    executable.constants().get(constant.0)
                else {
                    continue;
                };
                let pc = block.start_pc.saturating_add(offset);
                if !executable.bytecode().code[pc.saturating_add(1)..]
                    .iter()
                    .any(|operation| {
                        matches!(operation, crate::bytecode::Instruction::Call { args, .. } if args.contains(dst))
                    })
                {
                    continue;
                }
                let reference = heap.function_reference(expected).or_else(|| {
                    registers
                        .get(usize::from(*dst))
                        .and_then(|value| match value {
                            Value::Object(reference)
                                if matches!(
                                    heap.get(*reference),
                                    Ok(crate::object::Object::Function(observed))
                                        if observed.id() == expected.id()
                                ) =>
                            {
                                Some(*reference)
                            }
                            _ => None,
                        })
                })?;
                let identity = expected.id().as_u64();
                storage_cases.push((u32::from(*dst) << 24) | 0x00f1_0000);
                storage_cases.push(identity as u32);
                storage_cases.push((identity >> 32) as u32);
                storage_cases.push(reference.slot());
                storage_cases.push(reference.generation());
                storage_cases.push(reference.heap_id() as u32);
                storage_cases.push((reference.heap_id() >> 32) as u32);
                call_targets.insert(
                    *dst,
                    loop_snapshot::CallTarget {
                        function: (**expected).clone(),
                        handle: reference.slot(),
                        argument_element_paths: Vec::new(),
                        argument_indexed_element_types: Vec::new(),
                    },
                );
            }
        }
        for instruction in &executable.bytecode().code {
            let crate::bytecode::Instruction::Call { callable, args, .. } = instruction else {
                continue;
            };
            let paths = args
                .iter()
                .map(|argument| {
                    registers
                        .get(usize::from(*argument))
                        .and_then(|value| match value {
                            Value::Object(reference) => sequence_element_path(heap, *reference, 0),
                            _ => None,
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>();
            let indexed = args
                .iter()
                .map(|argument| {
                    let mut types = BTreeMap::new();
                    if let Some(Value::Object(reference)) = registers.get(usize::from(*argument)) {
                        sequence_indexed_element_types(
                            heap,
                            *reference,
                            &mut Vec::new(),
                            &mut types,
                            0,
                        );
                    }
                    types
                })
                .collect::<Vec<_>>();
            if let Some(target) = call_targets.get_mut(callable) {
                target.argument_element_paths = paths.clone();
                target.argument_indexed_element_types = indexed.clone();
            }
            if let Some(target) =
                constant_call_targets.get_mut(&(executable.id().as_u64(), *callable))
            {
                target.argument_element_paths = paths;
                target.argument_indexed_element_types = indexed;
            }
        }
        if let Some((case_index, register)) = empty_append_case {
            if !loop_will_enter_body(executable, region_description, registers) {
                return None;
            }
            let mut integer = storage_cases.clone();
            integer[case_index] = (u32::from(register) << 8) | 1;
            integer.push(0x8000_0000 | (u32::from(register) << 16) | 1);
            let mut float = storage_cases.clone();
            float[case_index] = (u32::from(register) << 8) | 2;
            float.push(0x8000_0000 | (u32::from(register) << 16) | 2);
            return self.loops.try_execute_existing(
                executable,
                region,
                registers,
                prepared.as_ref(),
                &[integer, float],
                required_preheader,
                |native, inputs, region, report| {
                    self.execute_object_loop(
                        native,
                        inputs,
                        executable,
                        (region, prepared.as_ref()),
                        (heap, report),
                        required_preheader.is_some(),
                    )
                },
            );
        }
        if let Some(preheader) = required_preheader {
            let mut candidates = vec![storage_cases];
            for (case_index, register) in missing_owned_destinations {
                candidates = candidates
                    .into_iter()
                    .flat_map(|candidate| {
                        [1_u32, 2_u32].map(|tag| {
                            let mut extended = candidate.clone();
                            extended[case_index] = (u32::from(register) << 8) | tag;
                            extended.push(0x8000_0000 | (u32::from(register) << 16) | tag);
                            extended
                        })
                    })
                    .collect();
            }
            return self.loops.try_execute_existing(
                executable,
                region,
                registers,
                prepared.as_ref(),
                &candidates,
                Some(preheader),
                |native, inputs, region, report| {
                    self.execute_object_loop(
                        native,
                        inputs,
                        executable,
                        (region, prepared.as_ref()),
                        (heap, report),
                        true,
                    )
                },
            );
        }
        self.loops.try_execute(
            executable,
            region,
            registers,
            prepared.as_ref(),
            &storage_cases,
            &element_types,
            &call_targets,
            &constant_call_targets,
            |native, inputs, region, report| {
                self.execute_object_loop(
                    native,
                    inputs,
                    executable,
                    (region, prepared.as_ref()),
                    (heap, report),
                    false,
                )
            },
        )
    }

    fn execute_object_loop(
        &self,
        native: &SharedTier1Code,
        inputs: &[Value],
        executable: &ExecutableFunction,
        selection: (
            &crate::structure_map::Region,
            Option<&loop_snapshot::PreparedLoop>,
        ),
        runtime: (&mut crate::object::ObjectHeap, &mut AdaptiveReport),
        allow_integer_widening: bool,
    ) -> Option<Result<LoopExecution, String>> {
        let (region, prepared) = selection;
        let (heap, report) = runtime;
        let mut inputs = inputs.to_vec();
        let mut handles = NativeHandleScope::new(
            self.native_handles
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let mut prepared_registers = prepared
            .into_iter()
            .flat_map(|prepared| prepared.values.iter().map(|(register, _)| *register))
            .collect::<Vec<_>>();
        let mut widening = allow_integer_widening;
        loop {
            let execution = match self.execute_object_loop_once(
                native,
                &inputs,
                region,
                widening,
                (heap, report, &mut handles),
            )? {
                Ok(execution) => execution,
                Err(error) => return Some(Err(error)),
            };
            let LoopExecution::Resume { target, registers } = &execution else {
                return Some(Ok(execution));
            };
            if *target != region.entry {
                return Some(Ok(execution));
            }
            let mut resumed = vec![Value::Uninitialized; executable.bytecode().register_count];
            for (register, value) in registers {
                resumed[usize::from(*register)] = *value;
            }
            for (register, value) in prepared_registers
                .iter()
                .zip(&inputs[region.entry_summary.len()..])
            {
                resumed[usize::from(*register)] = *value;
            }
            if !loop_will_enter_body(executable, region, &resumed) {
                return Some(Ok(execution));
            }
            let Some(prepared) = prepare_nested_loop_inputs(executable, region, &resumed, heap)
            else {
                return Some(Ok(execution));
            };
            let Some(mut next) = region
                .entry_summary
                .iter()
                .map(|slot| resumed.get(usize::from(slot.register)).copied())
                .collect::<Option<Vec<_>>>()
            else {
                return Some(Ok(execution));
            };
            next.extend(prepared.values.iter().map(|(_, value)| *value));
            let Some(native_next) = next
                .iter()
                .map(|value| match value {
                    Value::SmallInt(value) => Some(NativeValue::Integer(*value)),
                    Value::Float(value) => Some(NativeValue::FloatBits(value.to_bits())),
                    Value::Bool(value) => Some(NativeValue::Boolean(*value)),
                    Value::Object(reference) => {
                        handles.encode(*reference).ok().map(NativeValue::Handle)
                    }
                    Value::None | Value::Uninitialized => None,
                })
                .collect::<Option<Vec<_>>>()
            else {
                return Some(Ok(execution));
            };
            if !native.accepts_inputs(&native_next) {
                return Some(Ok(execution));
            }
            inputs = next;
            prepared_registers = prepared
                .values
                .iter()
                .map(|(register, _)| *register)
                .collect();
            widening = false;
        }
    }

    fn execute_object_loop_once(
        &self,
        native: &SharedTier1Code,
        inputs: &[Value],
        region: &crate::structure_map::Region,
        allow_integer_widening: bool,
        runtime: (
            &mut crate::object::ObjectHeap,
            &mut AdaptiveReport,
            &mut NativeHandleScope<'_>,
        ),
    ) -> Option<Result<LoopExecution, String>> {
        let (heap, report, handles) = runtime;
        if native.uses_dynamic_storage() {
            return self.execute_dynamic_object_loop_once(
                native,
                inputs,
                region,
                (heap, report, handles),
            );
        }
        struct StorageState {
            reference: crate::object::ObjectRef,
            values: Arc<[i64]>,
            layout_version: u64,
            float_storage: bool,
            widened_from_integer: bool,
            mutates: bool,
        }

        let storage_inputs = native.direct_storage_inputs();
        if storage_inputs.is_empty()
            || storage_inputs.len() > 2
            || storage_inputs
                .iter()
                .filter(|(_, mutates, _)| *mutates)
                .count()
                > 2
        {
            return None;
        }
        let mut execution_inputs = inputs.to_vec();
        let mut split_reference = None;
        if native.allows_owned_split_pair()
            && let [
                (first_index, true, super::wxir_v2::ir::ValueType::F64),
                (second_index, true, super::wxir_v2::ir::ValueType::F64),
            ] = storage_inputs.as_slice()
            && let (Some(Value::Object(first)), Some(Value::Object(second))) = (
                execution_inputs.get(*first_index),
                execution_inputs.get(*second_index),
            )
            && first == second
        {
            let values = heap.float_sequence_snapshot(*first).ok().flatten()?.0;
            let reference = heap
                .allocate(crate::object::Object::List(
                    crate::object::SequenceObject::from_values(
                        values.iter().copied().map(Value::Float).collect(),
                    ),
                ))
                .ok()?;
            execution_inputs[*second_index] = Value::Object(reference);
            split_reference = Some(reference);
        }
        let native_inputs = execution_inputs
            .iter()
            .map(|value| match value {
                Value::SmallInt(value) => Some(NativeValue::Integer(*value)),
                Value::Float(value) => Some(NativeValue::FloatBits(value.to_bits())),
                Value::Bool(value) => Some(NativeValue::Boolean(*value)),
                Value::Object(reference) => {
                    handles.encode(*reference).ok().map(NativeValue::Handle)
                }
                Value::None | Value::Uninitialized => None,
            })
            .collect::<Option<Vec<_>>>()?;
        if !native.accepts_inputs(&native_inputs) {
            return None;
        }
        let states = storage_inputs
            .iter()
            .map(|(input_index, mutates, element_type)| {
                let Value::Object(reference) = *execution_inputs.get(*input_index)? else {
                    return None;
                };
                let (values, layout_version, float_storage, widened_from_integer) =
                    match element_type {
                        super::wxir_v2::ir::ValueType::I64 => {
                            let (values, version) =
                                heap.integer_sequence_snapshot(reference).ok().flatten()?;
                            (values, version, false, false)
                        }
                        super::wxir_v2::ir::ValueType::F64 => {
                            if let Some((values, version)) =
                                heap.float_sequence_snapshot(reference).ok().flatten()
                            {
                                (
                                    values
                                        .iter()
                                        .map(|value| value.to_bits() as i64)
                                        .collect::<Vec<_>>()
                                        .into(),
                                    version,
                                    true,
                                    false,
                                )
                            } else if allow_integer_widening {
                                let (values, version) =
                                    heap.integer_sequence_snapshot(reference).ok().flatten()?;
                                (
                                    values
                                        .iter()
                                        .map(|value| (*value as f64).to_bits() as i64)
                                        .collect::<Vec<_>>()
                                        .into(),
                                    version,
                                    true,
                                    true,
                                )
                            } else {
                                return None;
                            }
                        }
                        _ => return None,
                    };
                Some(StorageState {
                    reference,
                    values,
                    layout_version,
                    float_storage,
                    widened_from_integer,
                    mutates: *mutates,
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(states) = states else {
            if let Some(reference) = split_reference.take() {
                let _ = heap.remove(reference);
            }
            return None;
        };
        let snapshots = states
            .iter()
            .map(|state| {
                if state.mutates {
                    SnapshotInput::Transaction(
                        state.values.iter().copied().collect(),
                        state.layout_version,
                    )
                } else {
                    SnapshotInput::Read(state.values.clone(), state.layout_version)
                }
            })
            .collect();
        let (outcome, transactions) =
            match native.execute_with_integer_storages(&native_inputs, snapshots) {
                Ok(result) => result,
                Err(
                    NativeError::AbiMismatch
                    | NativeError::SnapshotMismatch
                    | NativeError::StaleDependency
                    | NativeError::MalformedValue
                    | NativeError::InvalidExit(_)
                    | NativeError::Helper,
                ) => {
                    report.deopts = report.deopts.saturating_add(1);
                    if let Some(reference) = split_reference.take() {
                        let _ = heap.remove(reference);
                    }
                    return None;
                }
                Err(error) => {
                    if let Some(reference) = split_reference.take() {
                        let _ = heap.remove(reference);
                    }
                    return Some(Err(error.to_string()));
                }
            };
        report.cache_hits = report.cache_hits.saturating_add(1);
        record_loop_native(&outcome, report);
        if outcome.counters.deopts > 0 {
            if let Some(reference) = split_reference.take() {
                let _ = heap.remove(reference);
            }
            return None;
        }
        for state in states.iter().filter(|state| !state.mutates) {
            let current = if state.widened_from_integer {
                heap.integer_sequence_snapshot(state.reference)
                    .ok()
                    .flatten()
                    .map(|(_, version)| version)
            } else if state.float_storage {
                heap.float_sequence_snapshot(state.reference)
                    .ok()
                    .flatten()
                    .map(|(_, version)| version)
            } else {
                heap.integer_sequence_snapshot(state.reference)
                    .ok()
                    .flatten()
                    .map(|(_, version)| version)
            };
            if current != Some(state.layout_version) {
                report.deopts = report.deopts.saturating_add(1);
                if let Some(reference) = split_reference.take() {
                    let _ = heap.remove(reference);
                }
                return None;
            }
        }
        let execution = match loop_execution(&outcome, native, region, &execution_inputs, handles) {
            Ok(execution) => execution,
            Err(error) => {
                if let Some(reference) = split_reference.take() {
                    let _ = heap.remove(reference);
                }
                return Some(Err(error));
            }
        };
        let mut pending_integer_commits = Vec::new();
        let mut pending_float_commits = Vec::new();
        for (state, transaction) in states.iter().zip(transactions) {
            let Some(transaction) = transaction else {
                continue;
            };
            if state.float_storage {
                pending_float_commits.push((
                    state.reference,
                    state.layout_version,
                    state.widened_from_integer,
                    transaction
                        .into_iter()
                        .map(|bits| f64::from_bits(bits as u64))
                        .collect(),
                ));
            } else {
                pending_integer_commits.push((state.reference, state.layout_version, transaction));
            }
        }
        if !pending_integer_commits.is_empty() && !pending_float_commits.is_empty() {
            report.deopts = report.deopts.saturating_add(1);
            if let Some(reference) = split_reference.take() {
                let _ = heap.remove(reference);
            }
            return None;
        }
        let committed = match pending_integer_commits.as_mut_slice() {
            [] => Ok(true),
            [single] => heap.commit_integer_sequence_snapshot(
                single.0,
                single.1,
                std::mem::take(&mut single.2),
            ),
            [first, second] => heap.commit_integer_sequence_snapshot_pair(
                (first.0, first.1, std::mem::take(&mut first.2)),
                (second.0, second.1, std::mem::take(&mut second.2)),
            ),
            _ => unreachable!("direct storage plan is capped at two slots"),
        };
        if !matches!(committed, Ok(true)) {
            report.deopts = report.deopts.saturating_add(1);
            if let Some(reference) = split_reference.take() {
                let _ = heap.remove(reference);
            }
            return None;
        }
        let committed = match pending_float_commits.as_mut_slice() {
            [] => Ok(true),
            [single] if single.2 => heap.commit_widened_float_sequence_snapshot(
                single.0,
                single.1,
                std::mem::take(&mut single.3),
            ),
            [single] => heap.commit_float_sequence_snapshot(
                single.0,
                single.1,
                std::mem::take(&mut single.3),
            ),
            [first, second] if first.2 || second.2 => Ok(false),
            [first, second] => heap.commit_float_sequence_snapshot_pair(
                (first.0, first.1, std::mem::take(&mut first.3)),
                (second.0, second.1, std::mem::take(&mut second.3)),
            ),
            _ => unreachable!("direct storage plan is capped at two slots"),
        };
        if !matches!(committed, Ok(true)) {
            report.deopts = report.deopts.saturating_add(1);
            if let Some(reference) = split_reference.take() {
                let _ = heap.remove(reference);
            }
            return None;
        }
        report.compile_failure = None;
        Some(Ok(execution))
    }

    fn execute_dynamic_object_loop_once(
        &self,
        native: &SharedTier1Code,
        inputs: &[Value],
        region: &crate::structure_map::Region,
        runtime: (
            &mut crate::object::ObjectHeap,
            &mut AdaptiveReport,
            &mut NativeHandleScope<'_>,
        ),
    ) -> Option<Result<LoopExecution, String>> {
        #[derive(Clone)]
        struct State {
            reference: crate::object::ObjectRef,
            values: Vec<i64>,
            layout_version: u64,
            strategy: crate::object::SequenceStrategy,
            list: bool,
        }

        let (heap, report, handles) = runtime;
        let mut queue = inputs
            .iter()
            .filter_map(|value| match value {
                Value::Object(reference) => Some(*reference),
                _ => None,
            })
            .collect::<std::collections::VecDeque<_>>();
        let mut seen = std::collections::HashSet::new();
        let mut states = Vec::new();
        while let Some(reference) = queue.pop_front() {
            if !seen.insert(reference) {
                continue;
            }
            let (sequence, list) = match heap.get(reference).ok()? {
                crate::object::Object::List(sequence) => (sequence, true),
                crate::object::Object::Tuple(sequence) => (sequence, false),
                _ => continue,
            };
            let strategy = sequence.strategy();
            let layout_version = sequence.layout_version();
            let raw_values = sequence.to_vec();
            let mut values = Vec::with_capacity(raw_values.len());
            for value in raw_values {
                values.push(match value {
                    Value::SmallInt(value) => value,
                    Value::Float(value) => value.to_bits() as i64,
                    Value::Bool(value) => i64::from(value),
                    Value::Object(child) => {
                        queue.push_back(child);
                        handles.encode(child).ok()? as i64
                    }
                    Value::None | Value::Uninitialized => return None,
                });
            }
            states.push(State {
                reference,
                values,
                layout_version,
                strategy,
                list,
            });
        }
        let native_inputs = inputs
            .iter()
            .map(|value| match value {
                Value::SmallInt(value) => Some(NativeValue::Integer(*value)),
                Value::Float(value) => Some(NativeValue::FloatBits(value.to_bits())),
                Value::Bool(value) => Some(NativeValue::Boolean(*value)),
                Value::Object(reference) => {
                    handles.encode(*reference).ok().map(NativeValue::Handle)
                }
                Value::None | Value::Uninitialized => None,
            })
            .collect::<Option<Vec<_>>>()?;
        if !native.accepts_inputs(&native_inputs) {
            return None;
        }
        let fixed = native
            .direct_storage_inputs()
            .into_iter()
            .map(|(input, mutates, _)| {
                let Value::Object(reference) = *inputs.get(input)? else {
                    return None;
                };
                let state = states
                    .iter()
                    .find(|state| state.reference == reference)?
                    .clone();
                Some((state, mutates))
            })
            .collect::<Option<Vec<_>>>()?;
        let fixed_references = fixed
            .iter()
            .map(|(state, _)| state.reference)
            .collect::<std::collections::HashSet<_>>();
        let dynamic_states = states
            .iter()
            .filter(|state| !fixed_references.contains(&state.reference))
            .cloned()
            .collect::<Vec<_>>();
        let static_snapshots = fixed
            .iter()
            .map(|(state, mutates)| {
                if *mutates {
                    SnapshotInput::Transaction(state.values.clone(), state.layout_version)
                } else {
                    SnapshotInput::Read(state.values.clone().into(), state.layout_version)
                }
            })
            .collect();
        let snapshots = dynamic_states
            .iter()
            .map(|state| DynamicSnapshotInput {
                alias: handles
                    .encode(state.reference)
                    .expect("rooted reachable object"),
                values: state.values.clone(),
                layout_version: state.layout_version,
                mutates: state.list,
            })
            .collect();
        let (outcome, transactions) =
            match native.execute_with_storages(&native_inputs, static_snapshots, snapshots) {
                Ok(result) => result,
                Err(
                    NativeError::AbiMismatch
                    | NativeError::SnapshotMismatch
                    | NativeError::StaleDependency
                    | NativeError::MalformedValue
                    | NativeError::InvalidExit(_)
                    | NativeError::Helper
                    | NativeError::CountOverflow,
                ) => {
                    report.deopts = report.deopts.saturating_add(1);
                    return None;
                }
                Err(error) => return Some(Err(error.to_string())),
            };
        report.cache_hits = report.cache_hits.saturating_add(1);
        record_loop_native(&outcome, report);
        if outcome.counters.deopts > 0 {
            return None;
        }
        for state in &states {
            let (sequence, list) = match heap.get(state.reference).ok()? {
                crate::object::Object::List(sequence) => (sequence, true),
                crate::object::Object::Tuple(sequence) => (sequence, false),
                _ => return None,
            };
            if list != state.list
                || sequence.strategy() != state.strategy
                || sequence.layout_version() != state.layout_version
            {
                report.deopts = report.deopts.saturating_add(1);
                return None;
            }
        }
        let execution = match loop_execution(&outcome, native, region, inputs, handles) {
            Ok(execution) => execution,
            Err(error) => return Some(Err(error)),
        };
        let mut floats = Vec::new();
        let mut integers = Vec::new();
        let transaction_states = fixed
            .iter()
            .map(|(state, _)| state)
            .chain(dynamic_states.iter());
        for (state, transaction) in transaction_states.zip(transactions) {
            let Some(transaction) = transaction else {
                continue;
            };
            if transaction == state.values {
                continue;
            }
            match state.strategy {
                crate::object::SequenceStrategy::F64 => floats.push((
                    state.reference,
                    state.layout_version,
                    transaction
                        .into_iter()
                        .map(|bits| f64::from_bits(bits as u64))
                        .collect(),
                )),
                crate::object::SequenceStrategy::I64 => {
                    integers.push((state.reference, state.layout_version, transaction))
                }
                crate::object::SequenceStrategy::Empty
                | crate::object::SequenceStrategy::Bool
                | crate::object::SequenceStrategy::Object => {
                    report.deopts = report.deopts.saturating_add(1);
                    return None;
                }
            }
        }
        let committed = if !floats.is_empty() && integers.is_empty() {
            heap.commit_float_sequence_snapshots(floats)
        } else if floats.is_empty() && integers.len() == 1 {
            let (reference, version, values) = integers.pop().expect("one integer transaction");
            heap.commit_integer_sequence_snapshot(reference, version, values)
        } else if floats.is_empty() && integers.is_empty() {
            Ok(true)
        } else {
            Ok(false)
        };
        if !matches!(committed, Ok(true)) {
            report.deopts = report.deopts.saturating_add(1);
            return None;
        }
        report.compile_failure = None;
        Some(Ok(execution))
    }
}

fn sequence_element_path(
    heap: &crate::object::ObjectHeap,
    reference: crate::object::ObjectRef,
    depth: usize,
) -> Option<Vec<super::wxir_v2::ir::ValueType>> {
    if depth >= 8 {
        return None;
    }
    let sequence = match heap.get(reference).ok()? {
        crate::object::Object::List(sequence) | crate::object::Object::Tuple(sequence) => sequence,
        _ => return None,
    };
    let mut path = None;
    for value in sequence.to_vec() {
        let candidate = match value {
            Value::SmallInt(_) => vec![super::wxir_v2::ir::ValueType::I64],
            Value::Float(_) => vec![super::wxir_v2::ir::ValueType::F64],
            Value::Bool(_) => vec![super::wxir_v2::ir::ValueType::Bool],
            Value::Object(child) => {
                let mut candidate = vec![super::wxir_v2::ir::ValueType::Handle];
                if let Some(nested) = sequence_element_path(heap, child, depth.saturating_add(1)) {
                    candidate.extend(nested);
                }
                candidate
            }
            Value::None | Value::Uninitialized => return None,
        };
        if path.as_ref().is_some_and(|current| current != &candidate) {
            return None;
        }
        path = Some(candidate);
    }
    path
}

fn sequence_indexed_element_types(
    heap: &crate::object::ObjectHeap,
    reference: crate::object::ObjectRef,
    path: &mut Vec<usize>,
    types: &mut BTreeMap<Vec<usize>, super::wxir_v2::ir::ValueType>,
    depth: usize,
) {
    if depth >= 8 {
        return;
    }
    let sequence = match heap.get(reference) {
        Ok(crate::object::Object::List(sequence) | crate::object::Object::Tuple(sequence)) => {
            sequence
        }
        _ => return,
    };
    for (index, value) in sequence.to_vec().into_iter().enumerate() {
        path.push(index);
        let ty = match value {
            Value::SmallInt(_) => super::wxir_v2::ir::ValueType::I64,
            Value::Float(_) => super::wxir_v2::ir::ValueType::F64,
            Value::Bool(_) => super::wxir_v2::ir::ValueType::Bool,
            Value::Object(child) => {
                sequence_indexed_element_types(heap, child, path, types, depth + 1);
                super::wxir_v2::ir::ValueType::Handle
            }
            Value::None | Value::Uninitialized => {
                path.pop();
                continue;
            }
        };
        types.insert(path.clone(), ty);
        path.pop();
    }
}

fn indexed_sequence_type(
    types: &BTreeMap<Vec<usize>, super::wxir_v2::ir::ValueType>,
    path: &[Option<usize>],
) -> Option<super::wxir_v2::ir::ValueType> {
    let mut matches = types.iter().filter_map(|(candidate, ty)| {
        (candidate.len() == path.len()
            && candidate.iter().zip(path).all(|(candidate, expected)| {
                expected.is_none_or(|expected| *candidate == expected)
            }))
        .then_some(*ty)
    });
    let first = matches.next()?;
    matches.all(|ty| ty == first).then_some(first)
}

fn prepare_nested_loop_inputs(
    executable: &ExecutableFunction,
    region: &crate::structure_map::Region,
    registers: &[Value],
    heap: &crate::object::ObjectHeap,
) -> Option<loop_snapshot::PreparedLoop> {
    let header = executable.structure_map().block_by_pc(region.entry)?;
    let starts = region
        .blocks
        .iter()
        .filter_map(|block| executable.structure_map().block(*block))
        .map(|block| block.start_pc)
        .collect::<std::collections::BTreeSet<_>>();
    let body = executable.bytecode().code[header.start_pc..header.end_pc]
        .iter()
        .find_map(|instruction| match instruction {
            crate::bytecode::Instruction::Branch { yes, no, .. }
                if starts.contains(yes) && !starts.contains(no) =>
            {
                executable.structure_map().block_by_pc(*yes)
            }
            _ => None,
        })?;
    let mut values = registers.to_vec();
    let mut defined = std::collections::BTreeSet::new();
    let mut prefix_end = body.start_pc;
    for (offset, instruction) in executable.bytecode().code
        [body.start_pc..body.end_pc.saturating_sub(1)]
        .iter()
        .enumerate()
    {
        let pc = body.start_pc + offset;
        let (dst, value) = match instruction {
            crate::bytecode::Instruction::ConstSmallInt { dst, value }
            | crate::bytecode::Instruction::ConstI64 { dst, value } => {
                (*dst, Value::SmallInt(*value))
            }
            crate::bytecode::Instruction::ConstFloat { dst, value } => (*dst, Value::Float(*value)),
            crate::bytecode::Instruction::ConstBool { dst, value } => (*dst, Value::Bool(*value)),
            crate::bytecode::Instruction::Move { dst, src } => {
                (*dst, *values.get(usize::from(*src))?)
            }
            crate::bytecode::Instruction::GetItem { dst, object, key } => {
                let Value::Object(reference) = *values.get(usize::from(*object))? else {
                    return None;
                };
                let Value::SmallInt(index) = *values.get(usize::from(*key))? else {
                    return None;
                };
                let index = usize::try_from(index).ok()?;
                let sequence = match heap.get(reference).ok()? {
                    crate::object::Object::List(sequence)
                    | crate::object::Object::Tuple(sequence) => sequence,
                    _ => return None,
                };
                (*dst, sequence.get(index)?)
            }
            _ => {
                prefix_end = pc;
                break;
            }
        };
        *values.get_mut(usize::from(dst))? = value;
        defined.insert(dst);
        prefix_end = pc.saturating_add(1);
    }
    if prefix_end <= body.start_pc || prefix_end >= body.end_pc {
        return None;
    }
    let prepared = defined
        .into_iter()
        .filter(|register| {
            !region
                .entry_summary
                .iter()
                .any(|slot| slot.register == *register)
                && loop_snapshot::prepared_value_is_live(
                    &executable.bytecode().code,
                    prefix_end,
                    *register,
                )
        })
        .map(|register| Some((register, *values.get(usize::from(register))?)))
        .collect::<Option<Vec<_>>>()?;
    let numeric_storages = prepared
        .iter()
        .filter(|(_, value)| {
            let Value::Object(reference) = value else {
                return false;
            };
            matches!(
                heap.get(*reference),
                Ok(crate::object::Object::List(sequence))
                    if matches!(sequence.strategy(), crate::object::SequenceStrategy::I64 | crate::object::SequenceStrategy::F64)
            )
        })
        .count();
    (1..=2)
        .contains(&numeric_storages)
        .then_some(loop_snapshot::PreparedLoop {
            values: prepared,
            prefix: (body.start_pc, prefix_end),
        })
}

fn loop_will_enter_body(
    executable: &ExecutableFunction,
    region: &crate::structure_map::Region,
    registers: &[Value],
) -> bool {
    let Some(header) = executable.structure_map().block(region.blocks[0]) else {
        return false;
    };
    let code = &executable.bytecode().code[header.start_pc..header.end_pc];
    let branch = code.iter().find_map(|instruction| match instruction {
        crate::bytecode::Instruction::Branch { cond, yes, no } => Some((*cond, *yes, *no)),
        _ => None,
    });
    let Some((condition, yes, no)) = branch else {
        return false;
    };
    let body_starts = region
        .blocks
        .iter()
        .filter_map(|block| executable.structure_map().block(*block))
        .map(|block| block.start_pc)
        .collect::<std::collections::BTreeSet<_>>();
    if !body_starts.contains(&yes) || body_starts.contains(&no) {
        return false;
    }
    code.iter().any(|instruction| match instruction {
        crate::bytecode::Instruction::CompareOp {
            dst,
            op: crate::bytecode::CompareOperator::Lt,
            lhs,
            rhs,
            ..
        }
        | crate::bytecode::Instruction::LtI64 { dst, lhs, rhs }
            if *dst == condition =>
        {
            matches!(
                (registers.get(usize::from(*lhs)), registers.get(usize::from(*rhs))),
                (Some(Value::SmallInt(left)), Some(Value::SmallInt(right))) if left < right
            )
        }
        _ => false,
    })
}

fn loop_execution(
    outcome: &NativeOutcome,
    native: &SharedTier1Code,
    region: &crate::structure_map::Region,
    inputs: &[Value],
    handles: &NativeHandleScope<'_>,
) -> Result<LoopExecution, String> {
    if outcome.exit_id == 0 {
        return native_value(outcome).map(LoopExecution::Return);
    }
    if outcome.values.len() != region.entry_summary.len() || inputs.len() < outcome.values.len() {
        return Err("adaptive-v2 loop side exit shape changed".to_owned());
    }
    let registers = region
        .entry_summary
        .iter()
        .zip(&outcome.values)
        .map(|(slot, value)| {
            let value = match value {
                NativeValue::Integer(value) => Value::SmallInt(*value),
                NativeValue::FloatBits(value) => Value::Float(f64::from_bits(*value)),
                NativeValue::Boolean(value) => Value::Bool(*value),
                NativeValue::Handle(alias) => Value::Object(handles.resolve(*alias)?),
            };
            Ok((slot.register, value))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let target = native
        .resume_pc(outcome.exit_id)
        .or_else(|| region.exits.first().map(|exit| exit.target))
        .ok_or_else(|| "adaptive-v2 loop side exit target is missing".to_owned())?;
    Ok(LoopExecution::Resume { target, registers })
}

fn execute_tiered_loop(
    native: &tiered::TieredSite,
    arguments: &[Value],
    region: &crate::structure_map::Region,
    report: &mut AdaptiveReport,
) -> Option<Result<LoopExecution, String>> {
    let inputs = native_inputs(arguments).ok()?;
    let execution = match native.execute(&inputs) {
        Ok(execution) => execution,
        Err(NativeError::MalformedValue) => return None,
        Err(error) => return Some(Err(error.to_string())),
    };
    report.cache_hits = report
        .cache_hits
        .saturating_add(u64::from(!execution.cache_miss));
    report.cache_misses = report
        .cache_misses
        .saturating_add(u64::from(execution.cache_miss));
    report.cache_bytes = report
        .cache_bytes
        .saturating_add(execution.cache_added_bytes)
        .saturating_sub(execution.evicted_bytes);
    report.cache_evictions = report.cache_evictions.saturating_add(execution.evictions);
    report.compile_latency_micros = report
        .compile_latency_micros
        .saturating_add(execution.compile_micros);
    record_loop_native(&execution.attempted, report);
    if let Some(bridge) = execution.bridge.as_ref() {
        record_loop_native(bridge, report);
    }
    let outcome = execution.bridge.as_ref().unwrap_or(&execution.attempted);
    if execution.replay || outcome.counters.deopts > 0 {
        return None;
    }
    if outcome.exit_id == 0 {
        return Some(native_value(outcome).map(LoopExecution::Return));
    }
    let target = native.resume_pc(outcome.exit_id)?;
    let registers = region
        .entry_summary
        .iter()
        .zip(&outcome.values)
        .map(|(slot, value)| {
            let value = match value {
                NativeValue::Integer(value) => Value::SmallInt(*value),
                NativeValue::FloatBits(value) => Value::Float(f64::from_bits(*value)),
                NativeValue::Boolean(value) => Value::Bool(*value),
                NativeValue::Handle(_) => {
                    return Err("adaptive-v2 scalar loop returned a handle".to_owned());
                }
            };
            Ok((slot.register, value))
        })
        .collect::<Result<Vec<_>, String>>();
    Some(registers.map(|registers| LoopExecution::Resume { target, registers }))
}

fn execute_tiered(
    native: &tiered::TieredSite,
    arguments: &[Value],
    report: &mut AdaptiveReport,
) -> Option<Result<Value, String>> {
    let inputs = match native_inputs(arguments) {
        Ok(inputs) => inputs,
        Err(_) => return None,
    };
    execute_tiered_inputs(native, &inputs, report)
}

fn execute_tiered_inputs(
    native: &tiered::TieredSite,
    inputs: &[NativeValue],
    report: &mut AdaptiveReport,
) -> Option<Result<Value, String>> {
    let execution = match native.execute(inputs) {
        Ok(execution) => execution,
        Err(NativeError::MalformedValue) => return None,
        Err(error) => return Some(Err(error.to_string())),
    };
    report.cache_hits = report
        .cache_hits
        .saturating_add(u64::from(!execution.cache_miss));
    if execution.cache_miss {
        report.cache_misses = report.cache_misses.saturating_add(1);
        report.cache_bytes = report
            .cache_bytes
            .saturating_add(execution.cache_added_bytes);
    }
    report.cache_evictions = report.cache_evictions.saturating_add(execution.evictions);
    report.cache_bytes = report.cache_bytes.saturating_sub(execution.evicted_bytes);
    report.compile_latency_micros = report
        .compile_latency_micros
        .saturating_add(execution.compile_micros);
    if execution.tier2 {
        let id = snapshot_id(native.snapshot());
        report.tier2_snapshot_id = Some(id.clone());
        report.selected_snapshot_id = Some(id);
        report.compile_tier = Some("llvm-o3".to_owned());
    }
    record_native(&execution.attempted, report);
    if execution.bridge_linked {
        report.bridges = report.bridges.saturating_add(1);
    }
    if let Some(bridge) = execution.bridge {
        record_native(&bridge, report);
        let id: String = bridge
            .snapshot_id()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        report.selected_snapshot_id = Some(id.clone());
        report.tier1_snapshot_id = Some(id);
        report.compile_tier = Some("cranelift".to_owned());
        return Some(native_value(&bridge));
    }
    if execution.replay {
        return None;
    }
    Some(native_value(&execution.attempted))
}

fn native_inputs(arguments: &[Value]) -> Result<Vec<NativeValue>, String> {
    arguments
        .iter()
        .map(|value| match value {
            Value::SmallInt(value) => Ok(NativeValue::Integer(*value)),
            Value::Float(value) => Ok(NativeValue::FloatBits(value.to_bits())),
            Value::Bool(value) => Ok(NativeValue::Boolean(*value)),
            Value::None | Value::Object(_) | Value::Uninitialized => {
                Err("adaptive-v2 entry type changed".to_owned())
            }
        })
        .collect()
}

fn record_native(outcome: &NativeOutcome, report: &mut AdaptiveReport) {
    report.machine_entries = report
        .machine_entries
        .saturating_add(outcome.counters.machine_entries);
    report.helper_calls = report
        .helper_calls
        .saturating_add(outcome.counters.helper_calls);
    report.generic_dispatch_calls = report
        .generic_dispatch_calls
        .saturating_add(outcome.counters.generic_dispatch_calls);
    report.deopts = report.deopts.saturating_add(outcome.counters.deopts);
    if outcome.counters.deopts > 0 || outcome.exit_id != 0 {
        report.exits = report.exits.saturating_add(1);
        if outcome.guard_id != 0 {
            let failures = report.guard_failures.entry(outcome.guard_id).or_default();
            *failures = failures.saturating_add(1);
        }
        return;
    }
    report.native_executions = report.native_executions.saturating_add(1);
}

fn record_loop_native(outcome: &NativeOutcome, report: &mut AdaptiveReport) {
    report.machine_entries = report
        .machine_entries
        .saturating_add(outcome.counters.machine_entries);
    report.helper_calls = report
        .helper_calls
        .saturating_add(outcome.counters.helper_calls);
    report.generic_dispatch_calls = report
        .generic_dispatch_calls
        .saturating_add(outcome.counters.generic_dispatch_calls);
    report.deopts = report.deopts.saturating_add(outcome.counters.deopts);
    if outcome.counters.deopts == 0 {
        report.native_executions = report.native_executions.saturating_add(1);
    } else {
        report.exits = report.exits.saturating_add(1);
    }
}

fn native_value(outcome: &NativeOutcome) -> Result<Value, String> {
    if outcome.counters.deopts > 0 || outcome.exit_id != 0 {
        return Err("adaptive-v2 native side exit requires WVM replay".to_owned());
    }
    match outcome.values.as_slice() {
        [NativeValue::Integer(value)] => Ok(Value::SmallInt(*value)),
        [NativeValue::FloatBits(value)] => Ok(Value::Float(f64::from_bits(*value))),
        [NativeValue::Boolean(value)] => Ok(Value::Bool(*value)),
        [NativeValue::Handle(_)] | [] | [_, _, ..] => {
            Err("adaptive-v2 native result has an unsupported shape".to_owned())
        }
    }
}

fn compile_entry(
    state: &mut FunctionState,
    backend: Option<CompilerBackend>,
    runtime_id: u64,
    report: &mut AdaptiveReport,
) {
    let Some(permit) = state.profile.take_compile_permit() else {
        return;
    };
    let started = Instant::now();
    let result = state
        .draft
        .take()
        .ok_or_else(|| "adaptive-v2 recording did not produce WXIR".to_owned())
        .and_then(|draft| VerifiedSnapshot::seal(draft, permit).map_err(|error| error.to_string()))
        .and_then(|snapshot| match backend {
            Some(backend @ (CompilerBackend::Cranelift | CompilerBackend::Tiered)) => {
                tiered::TieredSite::compile(snapshot, backend, runtime_id)
                    .map_err(|error| error.to_string())
                    .map(|code| {
                        let id = snapshot_id(code.snapshot());
                        let cache_bytes = snapshot_cache_bytes(code.snapshot());
                        (id, cache_bytes, code)
                    })
            }
            #[cfg(feature = "inkwell")]
            Some(CompilerBackend::Llvm) => {
                Err("adaptive-v2 LLVM requires an observed Cranelift tier-1 snapshot".to_owned())
            }
            None => Err("adaptive-v2 interpreter mode does not compile".to_owned()),
        });
    report.compile_latency_micros =
        u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    match result {
        Ok((id, cache_bytes, code)) => {
            report.selected_snapshot_id = Some(id.clone());
            report.tier1_snapshot_id = Some(id);
            report.compile_tier = Some("cranelift".to_owned());
            report.cache_misses = report.cache_misses.saturating_add(1);
            report.cache_bytes = report.cache_bytes.saturating_add(cache_bytes);
            state.native = Some(Arc::new(code));
        }
        Err(error) => {
            report.compile_failure = Some(error);
            state.supported = false;
        }
    }
}

fn merge_report(report: &mut AdaptiveReport, delta: AdaptiveReport) {
    for region in delta.regions {
        report.regions.retain(|existing| {
            existing.executable_id != region.executable_id || existing.entry_pc != region.entry_pc
        });
        report.regions.push(region);
    }
    report.traces = report.traces.saturating_add(delta.traces);
    report.cache_hits = report.cache_hits.saturating_add(delta.cache_hits);
    report.cache_misses = report.cache_misses.saturating_add(delta.cache_misses);
    report.cache_bytes = report.cache_bytes.saturating_add(delta.cache_bytes);
    report.cache_evictions = report.cache_evictions.saturating_add(delta.cache_evictions);
    report.compile_latency_micros = report
        .compile_latency_micros
        .saturating_add(delta.compile_latency_micros);
    report.machine_entries = report.machine_entries.saturating_add(delta.machine_entries);
    report.helper_calls = report.helper_calls.saturating_add(delta.helper_calls);
    report.generic_dispatch_calls = report
        .generic_dispatch_calls
        .saturating_add(delta.generic_dispatch_calls);
    report.deopts = report.deopts.saturating_add(delta.deopts);
    report.exits = report.exits.saturating_add(delta.exits);
    report.native_executions = report
        .native_executions
        .saturating_add(delta.native_executions);
    report.bridges = report.bridges.saturating_add(delta.bridges);
    report.guest_calls = report.guest_calls.saturating_add(delta.guest_calls);
    report.materializations = report
        .materializations
        .saturating_add(delta.materializations);
    report.invalidations = report.invalidations.saturating_add(delta.invalidations);
    report.static_fact_matches = report
        .static_fact_matches
        .saturating_add(delta.static_fact_matches);
    report.readiness.live = report.readiness.live.saturating_add(delta.readiness.live);
    report.readiness.cached = report
        .readiness
        .cached
        .saturating_add(delta.readiness.cached);
    report.readiness.static_analysis = report
        .readiness
        .static_analysis
        .saturating_add(delta.readiness.static_analysis);
    for (guard, failures) in delta.guard_failures {
        let total = report.guard_failures.entry(guard).or_default();
        *total = total.saturating_add(failures);
    }
    let selected_snapshot_changed =
        delta.tier1_snapshot_id.is_some() || delta.tier2_snapshot_id.is_some();
    if delta.tier2_snapshot_id.is_some() {
        report.tier2_snapshot_id = delta.tier2_snapshot_id;
    }
    if delta.tier1_snapshot_id.is_some() {
        report.tier1_snapshot_id = delta.tier1_snapshot_id;
    }
    if selected_snapshot_changed {
        report.selected_snapshot_id = delta.selected_snapshot_id;
        report.compile_tier = delta.compile_tier;
    }
    if delta.compile_failure.is_some() {
        report.compile_failure = delta.compile_failure;
    }
}

fn sync_region(report: &mut AdaptiveReport, id: ExecutableId, state: &FunctionState, reason: &str) {
    let region = AdaptiveRegionReport {
        executable_id: id.as_u64(),
        entry_pc: 0,
        lifecycle: lifecycle_name(state.profile.lifecycle()).to_owned(),
        reason: reason.to_owned(),
        live_entries: state.profile.live_entries(),
        stable_observations: state.profile.stable_live(),
        specialized_cases: state.profile.case_count(),
        generic: state.profile.is_generic(),
    };
    report
        .regions
        .retain(|existing| existing.executable_id != id.as_u64());
    report.regions.push(region);
}

const fn lifecycle_name(lifecycle: Lifecycle) -> &'static str {
    match lifecycle {
        Lifecycle::Profiling => "profiling",
        Lifecycle::ReadyToRecord => "ready_to_record",
        Lifecycle::Recording => "recording",
        Lifecycle::ReadyToCompile => "ready_to_compile",
        Lifecycle::Compiled => "compiled",
    }
}

pub(super) fn snapshot_id(snapshot: &VerifiedSnapshot) -> String {
    snapshot
        .id()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn snapshot_cache_bytes(snapshot: &VerifiedSnapshot) -> u64 {
    snapshot.body().blocks.iter().fold(0_u64, |bytes, block| {
        let instructions = u64::try_from(block.instructions.len()).unwrap_or(u64::MAX);
        bytes.saturating_add(instructions.saturating_mul(64).saturating_add(64))
    })
}

#[cfg(test)]
mod concurrency_tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::object::{Object, ObjectHeap};

    #[test]
    fn object_shard_checkout_model_releases_mutex_restores_owned_state() {
        let shard = Arc::new(ObjectShard {
            state: Mutex::new(Some(ObjectShardState {
                osr: object_osr::ObjectOsr::new(
                    AdaptiveHeapRuntime::new(GcConfig::default()),
                    Some(CompilerBackend::Cranelift),
                    Arc::new(object_osr::ObjectSites::new()),
                ),
                report: AdaptiveReport::new(),
            })),
        });
        let checkout = shard.checkout().expect("initial shard checkout");
        let observer = Arc::clone(&shard);
        let (sent, received) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(move || sent.send(observer.snapshot_report().is_none()).unwrap());
            assert_eq!(received.recv_timeout(Duration::from_secs(1)), Ok(true));
        });
        drop(checkout);
        assert!(shard.snapshot_report().is_some());
    }

    #[test]
    fn object_shard_restores_state_after_panic() {
        // Given: a shard whose owned adapter state has been checked out without retaining its lock.
        let shard = ObjectShard {
            state: Mutex::new(Some(ObjectShardState {
                osr: object_osr::ObjectOsr::new(
                    AdaptiveHeapRuntime::new(GcConfig::default()),
                    Some(CompilerBackend::Cranelift),
                    Arc::new(object_osr::ObjectSites::new()),
                ),
                report: AdaptiveReport::new(),
            })),
        };

        // When: native/helper execution unwinds while the RAII checkout owns the adapter state.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _checkout = shard.checkout().expect("initial shard checkout");
            panic!("simulated helper unwind");
        }));

        // Then: Drop restored the state, so repeated execution can immediately check it out again.
        assert!(unwound.is_err());
        assert!(shard.checkout().is_some());
    }

    #[test]
    fn handle_scope_separates_heap_slots() {
        // Given: two authoritative heaps whose first objects have the same numeric slot.
        let mut first_heap = ObjectHeap::new();
        let mut second_heap = ObjectHeap::new();
        let first = first_heap
            .allocate(Object::list(Vec::new()))
            .expect("first object");
        let second = second_heap
            .allocate(Object::list(Vec::new()))
            .expect("second object");
        assert_eq!(first.slot(), second.slot());
        assert_ne!(first.heap_id(), second.heap_id());
        let table = Mutex::new(StableHandleTable::new(RuntimeId::new(71), 4));
        let mut scope = NativeHandleScope::new(
            table
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );

        // When: both full references cross the same invocation-scoped native boundary.
        let first_token = scope.encode(first).expect("first stable token");
        let second_token = scope.encode(second).expect("second stable token");

        // Then: equal heap slots cannot alias, and each token resolves to its exact owner.
        assert_ne!(first_token, second_token);
        assert_eq!(scope.resolve(first_token), Ok(first));
        assert_eq!(scope.resolve(second_token), Ok(second));
    }

    #[test]
    fn handle_scope_rejects_expired_tokens() {
        // Given: one stable token minted for a completed native invocation.
        let mut heap = ObjectHeap::new();
        let reference = heap.allocate(Object::list(Vec::new())).expect("object");
        let table = Mutex::new(StableHandleTable::new(RuntimeId::new(72), 1));
        let stale = {
            let mut scope = NativeHandleScope::new(
                table
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
            scope.encode(reference).expect("first invocation token")
        };

        // When: a later invocation reuses the bounded table slot.
        let mut next = NativeHandleScope::new(
            table
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let current = next.encode(reference).expect("next invocation token");

        // Then: generation advancement rejects the prior invocation and preserves the current one.
        assert_ne!(stale, current);
        assert!(next.resolve(stale).is_err());
        assert_eq!(next.resolve(current), Ok(reference));
    }

    #[test]
    fn handle_scope_overflow_keeps_live_roots() {
        // Given: a one-slot invocation scope and two distinct authoritative objects.
        let mut heap = ObjectHeap::new();
        let first = heap
            .allocate(Object::list(Vec::new()))
            .expect("first object");
        let second = heap
            .allocate(Object::list(Vec::new()))
            .expect("second object");
        let table = Mutex::new(StableHandleTable::new(RuntimeId::new(73), 1));
        let mut scope = NativeHandleScope::new(
            table
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );

        // When: native preparation exceeds the bounded owner table.
        let first_token = scope.encode(first).expect("first stable token");
        let overflow = scope.encode(second);

        // Then: preparation rejects safely while the accepted root remains exact and live.
        assert!(overflow.is_err());
        assert_eq!(scope.resolve(first_token), Ok(first));
        assert!(heap.get(first).is_ok());
    }
}

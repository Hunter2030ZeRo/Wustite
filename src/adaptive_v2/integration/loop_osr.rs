use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::executable::{ExecutableFunction, ExecutableId};
use crate::jit::CompilerBackend;
use crate::runtime::{AdaptiveRegionReport, AdaptiveReport};
use crate::structure_map::{Region, RegionId, SlotType};
use crate::value::Value;

use super::{SharedTier1Code, execute_tiered_loop, snapshot_cache_bytes, snapshot_id, tiered};
use crate::adaptive_v2::native::{NativeCompiler, clif_artifact_path, tier1_symbol};
use crate::adaptive_v2::profile::{AdaptiveProfile, Lifecycle, ProfileCase};
use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
use crate::adaptive_v2::wxir_v2::ir::SnapshotDraft;
use crate::adaptive_v2::wxir_v2::ir::ValueType;

pub(crate) enum LoopExecution {
    Return(Value),
    Resume {
        target: usize,
        registers: Vec<(u16, Value)>,
    },
}

pub(super) struct LoopOsr {
    states: Mutex<LoopRegistry>,
    report: Mutex<AdaptiveReport>,
    backend: Option<CompilerBackend>,
    runtime_id: u64,
}

#[derive(Default)]
struct LoopRegistry {
    states: HashMap<(ExecutableId, RegionId, ProfileCase), Arc<Mutex<LoopState>>>,
    sites: HashMap<(ExecutableId, RegionId), LoopCases>,
}

#[derive(Default)]
struct LoopCases {
    cases: Vec<ProfileCase>,
    generic: bool,
}

struct LoopState {
    profile: AdaptiveProfile,
    draft: Option<SnapshotDraft>,
    native: Option<Arc<LoopCode>>,
    preheader: Option<crate::adaptive_v2::trace::LoopPreheader>,
    storage_cases: Vec<u32>,
    supported: bool,
}

enum LoopCode {
    Scalar(Box<tiered::TieredSite>),
    Object(Box<SharedTier1Code>),
}

impl LoopOsr {
    pub(super) fn new(backend: Option<CompilerBackend>, runtime_id: u64) -> Self {
        Self {
            states: Mutex::new(LoopRegistry::default()),
            report: Mutex::new(AdaptiveReport::new()),
            backend,
            runtime_id,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_execute<F>(
        &self,
        executable: &ExecutableFunction,
        region_id: RegionId,
        registers: &[Value],
        prepared: Option<&super::loop_snapshot::PreparedLoop>,
        storage_cases: &[u32],
        element_types: &BTreeMap<u16, ValueType>,
        call_targets: &BTreeMap<u16, super::loop_snapshot::CallTarget>,
        constant_call_targets: &super::loop_snapshot::ConstantCallTargets,
        execute: F,
    ) -> Option<Result<LoopExecution, String>>
    where
        F: FnOnce(
            &SharedTier1Code,
            &[Value],
            &Region,
            &mut AdaptiveReport,
        ) -> Option<Result<LoopExecution, String>>,
    {
        let site_key = (executable.id(), region_id);
        let epoch = executable
            .id()
            .as_u64()
            .wrapping_add(region_id.0 as u64 + 1);
        let region = executable.structure_map().region(region_id)?;
        let mut inputs = region
            .entry_summary
            .iter()
            .map(|slot| registers.get(usize::from(slot.register)).copied())
            .collect::<Option<Vec<_>>>()?;
        inputs.extend(
            prepared
                .into_iter()
                .flat_map(|prepared| prepared.values.iter().map(|(_, value)| *value)),
        );
        if !region
            .entry_summary
            .iter()
            .zip(&inputs[..region.entry_summary.len()])
            .all(|(slot, value)| input_matches(slot.ty, *value))
        {
            return None;
        }
        let observation =
            super::observation::loop_header(executable, region_id, &inputs, storage_cases);
        let case = observation.live.case();
        let (state, specialized_cases) = {
            let mut registry = self
                .states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let site = registry.sites.entry(site_key).or_default();
            if site.generic {
                return None;
            }
            if !site.cases.contains(&case) {
                if site.cases.len() == 4 {
                    site.generic = true;
                    drop(registry);
                    let mut report = self
                        .report
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    sync_generic(&mut report, executable.id(), region_id, region.entry, 5);
                    return None;
                }
                site.cases.push(case);
            }
            let specialized_cases = site.cases.len();
            let state = registry
                .states
                .entry((executable.id(), region_id, case))
                .or_insert_with(|| {
                    Arc::new(Mutex::new(LoopState {
                        profile: AdaptiveProfile::new(epoch),
                        draft: None,
                        native: None,
                        preheader: None,
                        storage_cases: storage_cases.to_vec(),
                        supported: true,
                    }))
                })
                .clone();
            (state, specialized_cases)
        };
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.supported {
            return None;
        }
        if let Some(native) = state.native.clone() {
            drop(state);
            if self.has_recording_enclosing_region(executable, region_id) {
                return None;
            }
            let mut delta = AdaptiveReport::new();
            let result = match native.as_ref() {
                LoopCode::Scalar(native) => {
                    execute_tiered_loop(native, &inputs, region, &mut delta)
                }
                LoopCode::Object(native) => execute(native, &inputs, region, &mut delta),
            };
            let mut report = self
                .report
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            super::merge_report(&mut report, delta);
            if matches!(result, Some(Ok(_))) {
                report.compile_failure = None;
            }
            return result;
        }
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.profile.observe_live(observation.live);
        report.readiness.live = report.readiness.live.saturating_add(1);
        report.static_fact_matches = report
            .static_fact_matches
            .saturating_add(observation.static_facts);
        match state.profile.lifecycle() {
            Lifecycle::ReadyToRecord => record(
                executable,
                region_id,
                &inputs,
                prepared,
                element_types,
                call_targets,
                constant_call_targets,
                &mut state,
                &mut report,
            ),
            Lifecycle::ReadyToCompile => compile(
                &mut state,
                self.backend,
                self.runtime_id,
                executable.id(),
                region.entry,
                &mut report,
            ),
            Lifecycle::Profiling | Lifecycle::Recording | Lifecycle::Compiled => {}
        }
        sync_report(
            &mut report,
            executable.id(),
            region_id,
            region.entry,
            &state,
            specialized_cases,
            false,
        );
        None
    }

    pub(super) fn try_execute_compiled_preheader<F>(
        &self,
        executable: &ExecutableFunction,
        region_id: RegionId,
        registers: &[Value],
        prepared: Option<&super::loop_snapshot::PreparedLoop>,
        required_preheader: crate::adaptive_v2::trace::LoopPreheader,
        execute: F,
    ) -> Option<Result<LoopExecution, String>>
    where
        F: FnOnce(
            &SharedTier1Code,
            &[Value],
            &Region,
            &mut AdaptiveReport,
        ) -> Option<Result<LoopExecution, String>>,
    {
        let region = executable.structure_map().region(region_id)?;
        let mut inputs = region
            .entry_summary
            .iter()
            .map(|slot| registers.get(usize::from(slot.register)).copied())
            .collect::<Option<Vec<_>>>()?;
        inputs.extend(
            prepared
                .into_iter()
                .flat_map(|prepared| prepared.values.iter().map(|(_, value)| *value)),
        );
        if !region
            .entry_summary
            .iter()
            .zip(&inputs[..region.entry_summary.len()])
            .all(|(slot, value)| input_matches(slot.ty, *value))
        {
            return None;
        }
        let candidates = {
            let registry = self
                .states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            registry
                .states
                .iter()
                .filter_map(|((candidate_executable, candidate_region, case), state)| {
                    if *candidate_executable != executable.id() || *candidate_region != region_id {
                        return None;
                    }
                    let state = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if !state.supported || state.preheader != Some(required_preheader) {
                        return None;
                    }
                    let observed = super::observation::loop_header(
                        executable,
                        region_id,
                        &inputs,
                        &state.storage_cases,
                    )
                    .live
                    .case();
                    (observed == *case).then(|| state.native.clone()).flatten()
                })
                .collect::<Vec<_>>()
        };
        let [native] = candidates.as_slice() else {
            return None;
        };
        let mut delta = AdaptiveReport::new();
        let result = match native.as_ref() {
            LoopCode::Scalar(native) => execute_tiered_loop(native, &inputs, region, &mut delta),
            LoopCode::Object(native) => execute(native, &inputs, region, &mut delta),
        };
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        super::merge_report(&mut report, delta);
        if matches!(result, Some(Ok(_))) {
            report.compile_failure = None;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_execute_existing<F>(
        &self,
        executable: &ExecutableFunction,
        region_id: RegionId,
        registers: &[Value],
        prepared: Option<&super::loop_snapshot::PreparedLoop>,
        storage_case_candidates: &[Vec<u32>],
        required_preheader: Option<crate::adaptive_v2::trace::LoopPreheader>,
        execute: F,
    ) -> Option<Result<LoopExecution, String>>
    where
        F: FnOnce(
            &SharedTier1Code,
            &[Value],
            &Region,
            &mut AdaptiveReport,
        ) -> Option<Result<LoopExecution, String>>,
    {
        let region = executable.structure_map().region(region_id)?;
        let mut inputs = region
            .entry_summary
            .iter()
            .map(|slot| registers.get(usize::from(slot.register)).copied())
            .collect::<Option<Vec<_>>>()?;
        inputs.extend(
            prepared
                .into_iter()
                .flat_map(|prepared| prepared.values.iter().map(|(_, value)| *value)),
        );
        if !region
            .entry_summary
            .iter()
            .zip(&inputs[..region.entry_summary.len()])
            .all(|(slot, value)| input_matches(slot.ty, *value))
        {
            return None;
        }
        let mut candidate_cases = Vec::new();
        for storage_cases in storage_case_candidates {
            let case =
                super::observation::loop_header(executable, region_id, &inputs, storage_cases)
                    .live
                    .case();
            if !candidate_cases.contains(&case) {
                candidate_cases.push(case);
            }
        }
        let (candidates, specialized_cases) = {
            let registry = self
                .states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let candidates = if required_preheader.is_some() {
                registry
                    .states
                    .iter()
                    .filter_map(|((candidate_executable, candidate_region, _), state)| {
                        if *candidate_executable != executable.id()
                            || *candidate_region != region_id
                        {
                            return None;
                        }
                        let observed = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        storage_case_candidates
                            .iter()
                            .any(|candidate| {
                                preheader_storage_matches(candidate, &observed.storage_cases)
                            })
                            .then(|| Arc::clone(state))
                    })
                    .collect::<Vec<_>>()
            } else {
                candidate_cases
                    .iter()
                    .filter_map(|case| {
                        registry
                            .states
                            .get(&(executable.id(), region_id, *case))
                            .cloned()
                    })
                    .collect::<Vec<_>>()
            };
            let specialized_cases = registry
                .sites
                .get(&(executable.id(), region_id))
                .map_or(candidate_cases.len(), |site| site.cases.len());
            (candidates, specialized_cases)
        };
        let selected = candidates
            .iter()
            .filter_map(|candidate| {
                let state = candidate
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (state.supported
                    && required_preheader.is_none_or(|required| {
                        state.preheader.is_some_and(|actual| actual == required)
                    }))
                .then(|| Arc::clone(candidate))
            })
            .collect::<Vec<_>>();
        let [selected] = selected.as_slice() else {
            return None;
        };
        let mut state = selected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.native.is_none() {
            let observation = super::observation::loop_header(
                executable,
                region_id,
                &inputs,
                &state.storage_cases,
            );
            let mut report = self
                .report
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.profile.observe_live(observation.live);
            report.readiness.live = report.readiness.live.saturating_add(1);
            report.static_fact_matches = report
                .static_fact_matches
                .saturating_add(observation.static_facts);
            if state.profile.lifecycle() == Lifecycle::ReadyToCompile {
                compile(
                    &mut state,
                    self.backend,
                    self.runtime_id,
                    executable.id(),
                    region.entry,
                    &mut report,
                );
            }
            sync_report(
                &mut report,
                executable.id(),
                region_id,
                region.entry,
                &state,
                specialized_cases,
                false,
            );
        }
        let native = state.native.clone()?;
        drop(state);
        let mut delta = AdaptiveReport::new();
        let result = match native.as_ref() {
            LoopCode::Scalar(native) => execute_tiered_loop(native, &inputs, region, &mut delta),
            LoopCode::Object(native) => execute(native, &inputs, region, &mut delta),
        };
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        super::merge_report(&mut report, delta);
        if matches!(result, Some(Ok(_))) {
            report.compile_failure = None;
        }
        result
    }

    pub(super) fn report(&self) -> AdaptiveReport {
        self.report
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn has_recording_enclosing_region(
        &self,
        executable: &ExecutableFunction,
        region_id: RegionId,
    ) -> bool {
        let structure = executable.structure_map();
        let Some(region) = structure.region(region_id) else {
            return false;
        };
        let enclosing = structure
            .loop_regions()
            .filter_map(|(candidate_id, candidate)| {
                (candidate_id != region_id
                    && candidate.blocks.len() > region.blocks.len()
                    && region
                        .blocks
                        .iter()
                        .all(|block| candidate.blocks.contains(block)))
                .then_some(candidate_id)
            })
            .collect::<Vec<_>>();
        if enclosing.is_empty() {
            return false;
        }
        let registry = self
            .states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry
            .states
            .iter()
            .any(|((candidate_executable, candidate_region, _), candidate)| {
                *candidate_executable == executable.id()
                    && enclosing.contains(candidate_region)
                    && {
                        let state = candidate
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.supported
                            && state.draft.is_some()
                            && state.native.is_none()
                            && matches!(
                                state.profile.lifecycle(),
                                Lifecycle::Recording | Lifecycle::ReadyToCompile
                            )
                    }
            })
    }
}

fn preheader_storage_matches(current: &[u32], recorded: &[u32]) -> bool {
    current.len() == recorded.len()
        && current.iter().zip(recorded).all(|(current, recorded)| {
            current == recorded
                || (current & !0xff == recorded & !0xff
                    && current & 0xff == 1
                    && recorded & 0xff == 2)
        })
}

const fn input_matches(expected: SlotType, value: Value) -> bool {
    match expected {
        SlotType::SmallInt => matches!(value, Value::SmallInt(_)),
        SlotType::Float => matches!(value, Value::Float(_)),
        SlotType::Bool => matches!(value, Value::Bool(_)),
        SlotType::Object(_) => matches!(value, Value::Object(_)),
        SlotType::Any => !matches!(value, Value::None | Value::Uninitialized),
    }
}

#[allow(clippy::too_many_arguments)]
fn record(
    executable: &ExecutableFunction,
    region_id: RegionId,
    inputs: &[Value],
    prepared: Option<&super::loop_snapshot::PreparedLoop>,
    element_types: &BTreeMap<u16, ValueType>,
    call_targets: &BTreeMap<u16, super::loop_snapshot::CallTarget>,
    constant_call_targets: &super::loop_snapshot::ConstantCallTargets,
    state: &mut LoopState,
    report: &mut AdaptiveReport,
) {
    let Some(permit) = state.profile.take_record_permit() else {
        return;
    };
    match super::loop_snapshot::loop_draft(
        executable,
        region_id,
        inputs,
        prepared,
        element_types,
        call_targets,
        constant_call_targets,
        permit.schema_epoch(),
    ) {
        Ok(draft) => {
            state.preheader = match draft.body.entry_kind {
                crate::adaptive_v2::trace::EntryKind::LoopHeader { preheader, .. } => preheader,
                crate::adaptive_v2::trace::EntryKind::FunctionEntry => None,
            };
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

fn compile(
    state: &mut LoopState,
    backend: Option<CompilerBackend>,
    runtime_id: u64,
    executable: ExecutableId,
    entry: usize,
    report: &mut AdaptiveReport,
) {
    let Some(permit) = state.profile.take_compile_permit() else {
        return;
    };
    let started = Instant::now();
    let result = state
        .draft
        .take()
        .ok_or_else(|| "adaptive-v2 loop recording produced no WXIR".to_owned())
        .and_then(|draft| VerifiedSnapshot::seal(draft, permit).map_err(|error| error.to_string()))
        .and_then(|snapshot| {
            let selected_id = snapshot_id(&snapshot);
            let cache_bytes = snapshot_cache_bytes(&snapshot);
            let has_handles = snapshot.body().blocks.iter().any(|block| {
                block.parameters.iter().any(|parameter| {
                    parameter.ty == crate::adaptive_v2::wxir_v2::ir::ValueType::Handle
                })
            });
            match (backend, has_handles) {
                (Some(CompilerBackend::Cranelift | CompilerBackend::Tiered), true) => {
                    let mut compiler = NativeCompiler::new();
                    let code = compiler
                        .compile_tier1(&snapshot)
                        .map_err(|error| error.to_string())?;
                    let tier1_id = code.snapshot_id();
                    let code = SharedTier1Code::new(code)
                        .map(|code| code.with_loop_resumes(&snapshot))
                        .map_err(|error| error.to_string())?;
                    Ok((
                        selected_id,
                        tier1_id,
                        cache_bytes,
                        LoopCode::Object(Box::new(code)),
                    ))
                }
                (Some(backend @ (CompilerBackend::Cranelift | CompilerBackend::Tiered)), false) => {
                    tiered::TieredSite::compile(snapshot, backend, runtime_id)
                        .map_err(|error| error.to_string())
                        .map(|code| {
                            let tier1_id = code.snapshot().id();
                            (
                                selected_id,
                                tier1_id,
                                cache_bytes,
                                LoopCode::Scalar(Box::new(code)),
                            )
                        })
                }
                #[cfg(feature = "inkwell")]
                (Some(CompilerBackend::Llvm), _) => {
                    Err("adaptive-v2 LLVM loop OSR requires observed Cranelift tier-1".to_owned())
                }
                (None, _) => Err("adaptive-v2 interpreter mode does not compile loops".to_owned()),
            }
        });
    report.compile_latency_micros =
        u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    match result {
        Ok((selected_id, tier1_id, cache_bytes, code)) => {
            let symbol = tier1_symbol(tier1_id);
            let tier1_id = crate::adaptive_v2::native::snapshot_id_hex(tier1_id);
            report.selected_snapshot_id = Some(selected_id.clone());
            report.tier1_snapshot_id = Some(tier1_id.clone());
            if let Some(path) = clif_artifact_path(&symbol) {
                eprintln!(
                    "adaptive-v2 compile-artifact executable_id={} entry_pc={} selected_snapshot={} tier1_snapshot={} tier=cranelift-tier1 symbol={} clif_path={}",
                    executable.as_u64(),
                    entry,
                    selected_id,
                    tier1_id,
                    symbol,
                    path.display()
                );
            }
            report.compile_tier = Some("cranelift".to_owned());
            report.cache_misses = report.cache_misses.saturating_add(1);
            report.cache_bytes = report.cache_bytes.saturating_add(cache_bytes);
            state.native = Some(Arc::new(code));
        }
        Err(error) => {
            state.supported = false;
            report.compile_failure = Some(error);
        }
    }
}

fn sync_report(
    report: &mut AdaptiveReport,
    executable: ExecutableId,
    region: RegionId,
    entry: usize,
    state: &LoopState,
    specialized_cases: usize,
    generic: bool,
) {
    let id = executable.as_u64();
    report
        .regions
        .retain(|existing| !(existing.executable_id == id && existing.entry_pc == entry as u32));
    report.regions.push(AdaptiveRegionReport {
        executable_id: id,
        entry_pc: u32::try_from(entry).unwrap_or(u32::MAX),
        lifecycle: format!("{:?}", state.profile.lifecycle()).to_ascii_lowercase(),
        reason: format!("loop region {} live profiling", region.0),
        live_entries: state.profile.live_entries(),
        stable_observations: state.profile.stable_live(),
        specialized_cases,
        generic,
    });
}

fn sync_generic(
    report: &mut AdaptiveReport,
    executable: ExecutableId,
    region: RegionId,
    entry: usize,
    specialized_cases: usize,
) {
    let id = executable.as_u64();
    report
        .regions
        .retain(|existing| !(existing.executable_id == id && existing.entry_pc == entry as u32));
    report.regions.push(AdaptiveRegionReport {
        executable_id: id,
        entry_pc: u32::try_from(entry).unwrap_or(u32::MAX),
        lifecycle: "profiling".to_owned(),
        reason: format!("loop region {} exceeded specialization cap", region.0),
        live_entries: 0,
        stable_observations: 0,
        specialized_cases,
        generic: true,
    });
}

#[cfg(test)]
mod tests {
    use super::preheader_storage_matches;

    #[test]
    fn preheader_match_guards_int_to_float_widening() {
        // Given: one entry storage and one owned destination from a float snapshot.
        let recorded = [2, 3074];

        // When: the preheader observes the same identities with integer entry storage.
        let compatible = preheader_storage_matches(&[1, 3074], &recorded);

        // Then: the numeric widening is admitted without changing any identity word.
        assert!(compatible);
        assert!(!preheader_storage_matches(&[2, 3074], &[1, 3074]));
        assert!(!preheader_storage_matches(&[1, 3075], &recorded));
        assert!(!preheader_storage_matches(&[1], &recorded));
    }
}

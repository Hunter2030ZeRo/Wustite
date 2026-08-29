use std::sync::Arc;
use std::time::Instant;

use crate::adaptive_v2::native::NativeCompiler;
use crate::adaptive_v2::profile::{AdaptiveProfile, Lifecycle};
use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
use crate::bytecode::Instruction;
use crate::executable::ExecutableFunction;
use crate::jit::CompilerBackend;
use crate::object::ObjectHeap;
use crate::runtime::{AdaptiveRegionReport, AdaptiveReport};
use crate::value::Value;

use super::operations::{decode, invalidated_object, invalidates_all_bindings};
use super::{ObjectOsr, ObjectTicket, Operation, SiteKey, SiteState};
use crate::adaptive_v2::integration::SharedTier1Code;

impl ObjectOsr {
    pub(crate) fn before(
        &mut self,
        executable: &ExecutableFunction,
        pc: usize,
        instruction: &Instruction,
        registers: &[Value],
        heap: &mut ObjectHeap,
        report: &mut AdaptiveReport,
    ) -> Option<ObjectTicket> {
        let Some(operation) = decode(instruction, registers, heap) else {
            self.invalidate_binding(instruction, registers, report);
            return None;
        };
        if matches!(operation, Operation::DirectCall { .. }) {
            if !self.bindings.is_empty() {
                self.bindings.clear();
                report.invalidations = report.invalidations.saturating_add(1);
            }
            return None;
        }
        let source_pc = pc;
        let pc = u32::try_from(pc).ok()?;
        let key = operation.site_operation().map(|operation| SiteKey {
            executable: executable.id().as_u64(),
            pc,
            operation,
        });
        let key_ref = key.as_ref()?;
        let Some(native) = self.observe_site(
            executable,
            source_pc,
            Some(key_ref),
            &operation,
            heap,
            report,
        ) else {
            return Some(ObjectTicket {
                output: None,
                handled: false,
            });
        };
        operation.bind(&mut self.context, pc).ok()?;
        let receiver = operation.receiver();
        let binding = self.ensure_binding(receiver, heap).ok()?;
        report.cache_hits = report.cache_hits.saturating_add(1);
        let result =
            native.execute_with_adaptive_heap(&operation.inputs(binding, pc), &mut self.context);
        let output = match result {
            Ok(outcome) if outcome.counters.deopts == 0 && outcome.exit_id == 0 => {
                report.machine_entries = report
                    .machine_entries
                    .saturating_add(outcome.counters.machine_entries);
                report.native_executions = report.native_executions.saturating_add(1);
                report.helper_calls = report
                    .helper_calls
                    .saturating_add(outcome.counters.helper_calls);
                report.generic_dispatch_calls = report
                    .generic_dispatch_calls
                    .saturating_add(outcome.counters.generic_dispatch_calls);
                operation.output_from(&outcome.values)
            }
            Ok(_) | Err(_) => {
                report.deopts = report.deopts.saturating_add(1);
                operation.execute_authoritative(&mut self.context, binding, pc)
            }
        };
        if self.hand_back(receiver, &operation, heap).is_err() {
            report.invalidations = report.invalidations.saturating_add(1);
            return None;
        }
        Some(ObjectTicket {
            output: output.ok()?,
            handled: true,
        })
    }

    pub(crate) fn after(&mut self, _: ObjectTicket, _: &[Value], _: &mut AdaptiveReport) {}

    fn observe_site(
        &self,
        executable: &ExecutableFunction,
        source_pc: usize,
        key: Option<&SiteKey>,
        operation: &Operation,
        heap: &ObjectHeap,
        report: &mut AdaptiveReport,
    ) -> Option<Arc<SharedTier1Code>> {
        let key = key?;
        let profile_case = operation.profile_case(heap)?;
        let mut sites = self
            .sites
            .states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = sites.entry(key.clone()).or_insert_with(|| SiteState {
            profile: AdaptiveProfile::new(key.executable.wrapping_add(u64::from(key.pc))),
            classification: super::super::observation::instruction_classification(
                executable, source_pc,
            ),
            draft: None,
            native: None,
            cache_bytes: 0,
        });
        let was_generic = state.profile.is_generic();
        let observation = state.classification.observe_case(profile_case);
        state.profile.observe_live(observation.live);
        report.readiness.live = report.readiness.live.saturating_add(1);
        report.static_fact_matches = report
            .static_fact_matches
            .saturating_add(observation.static_facts);
        if !was_generic && state.profile.is_generic() {
            evict_generic(state, report);
        }
        let native = (!state.profile.is_generic())
            .then(|| state.native.clone())
            .flatten();
        advance(key, state, self.backend, report);
        sync_report(key, state, report);
        native
    }

    fn invalidate_binding(
        &mut self,
        instruction: &Instruction,
        registers: &[Value],
        report: &mut AdaptiveReport,
    ) {
        if invalidates_all_bindings(instruction) && !self.bindings.is_empty() {
            self.bindings.clear();
            report.invalidations = report.invalidations.saturating_add(1);
            return;
        }
        if let Some(reference) = invalidated_object(instruction, registers)
            && self.bindings.remove(&reference).is_some()
        {
            report.invalidations = report.invalidations.saturating_add(1);
        }
    }
}

fn advance(
    key: &SiteKey,
    state: &mut SiteState,
    backend: Option<CompilerBackend>,
    report: &mut AdaptiveReport,
) {
    match state.profile.lifecycle() {
        Lifecycle::ReadyToRecord => {
            if let Some(permit) = state.profile.take_record_permit() {
                state.draft = Some(super::snapshot::draft(
                    key.executable,
                    key.pc,
                    key.operation,
                    permit,
                ));
                report.traces = report.traces.saturating_add(1);
                let _ = state.profile.finish_recording();
            }
        }
        Lifecycle::ReadyToCompile => compile(state, backend, report),
        Lifecycle::Profiling | Lifecycle::Recording | Lifecycle::Compiled => {}
    }
}

fn evict_generic(state: &mut SiteState, report: &mut AdaptiveReport) {
    if state.native.take().is_some() {
        report.cache_bytes = report.cache_bytes.saturating_sub(state.cache_bytes);
        report.cache_evictions = report.cache_evictions.saturating_add(1);
    }
    state.cache_bytes = 0;
    state.draft = None;
    report.invalidations = report.invalidations.saturating_add(1);
}

fn compile(state: &mut SiteState, backend: Option<CompilerBackend>, report: &mut AdaptiveReport) {
    let Some(permit) = state.profile.take_compile_permit() else {
        return;
    };
    let started = Instant::now();
    let result = state
        .draft
        .take()
        .ok_or_else(|| "object trace missing".to_owned())
        .and_then(|draft| VerifiedSnapshot::seal(draft, permit).map_err(|error| error.to_string()))
        .and_then(|snapshot| {
            let id = super::super::snapshot_id(&snapshot);
            let cache_bytes = super::super::snapshot_cache_bytes(&snapshot);
            match backend {
                Some(CompilerBackend::Cranelift | CompilerBackend::Tiered) => NativeCompiler::new()
                    .compile_tier1(&snapshot)
                    .map_err(|error| error.to_string())
                    .and_then(SharedTier1Code::new)
                    .map(|native| (id, cache_bytes, native)),
                #[cfg(feature = "inkwell")]
                Some(CompilerBackend::Llvm) => {
                    Err("object trace requires Cranelift tier 1".to_owned())
                }
                None => Err("object trace backend disabled".to_owned()),
            }
        });
    report.compile_latency_micros =
        u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    match result {
        Ok((id, cache_bytes, native)) => {
            report.selected_snapshot_id = Some(id.clone());
            report.tier1_snapshot_id = Some(id);
            report.compile_tier = Some("cranelift".to_owned());
            report.cache_misses = report.cache_misses.saturating_add(1);
            report.cache_bytes = report.cache_bytes.saturating_add(cache_bytes);
            state.cache_bytes = cache_bytes;
            state.native = Some(Arc::new(native));
        }
        Err(error) => report.compile_failure = Some(error),
    }
}

fn sync_report(key: &SiteKey, state: &SiteState, report: &mut AdaptiveReport) {
    if let Some(region) = report
        .regions
        .iter_mut()
        .find(|region| region.executable_id == key.executable && region.entry_pc == key.pc)
    {
        let lifecycle = super::super::lifecycle_name(state.profile.lifecycle());
        if region.lifecycle != lifecycle {
            region.lifecycle = lifecycle.to_owned();
        }
        region.live_entries = state.profile.live_entries();
        region.stable_observations = state.profile.stable_live();
        region.specialized_cases = state.profile.case_count();
        region.generic = state.profile.is_generic();
        return;
    }
    report.regions.push(AdaptiveRegionReport {
        executable_id: key.executable,
        entry_pc: key.pc,
        lifecycle: format!("{:?}", state.profile.lifecycle()).to_ascii_lowercase(),
        reason: format!("adaptive {:?} instruction site", key.operation),
        live_entries: state.profile.live_entries(),
        stable_observations: state.profile.stable_live(),
        specialized_cases: state.profile.case_count(),
        generic: state.profile.is_generic(),
    });
}

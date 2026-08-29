use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCore {
    Legacy,
    AdaptiveV2,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AdaptiveReadinessSourceCounts {
    pub live: u64,
    pub cached: u64,
    pub static_analysis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdaptiveRegionReport {
    pub executable_id: u64,
    pub entry_pc: u32,
    pub lifecycle: String,
    pub reason: String,
    pub live_entries: u64,
    pub stable_observations: u64,
    pub specialized_cases: usize,
    pub generic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdaptiveReport {
    pub schema_version: u32,
    pub runtime_core: RuntimeCore,
    pub default_core: RuntimeCore,
    pub qualified_for_default: bool,
    pub rollback_available: bool,
    pub regions: Vec<AdaptiveRegionReport>,
    pub traces: u64,
    pub bridges: u64,
    pub selected_snapshot_id: Option<String>,
    pub tier1_snapshot_id: Option<String>,
    pub tier2_snapshot_id: Option<String>,
    pub machine_entries: u64,
    pub native_executions: u64,
    pub helper_calls: u64,
    pub generic_dispatch_calls: u64,
    pub guest_calls: u64,
    pub exits: u64,
    pub deopts: u64,
    pub materializations: u64,
    pub guard_failures: BTreeMap<u32, u64>,
    pub compile_latency_micros: u64,
    pub compile_tier: Option<String>,
    pub compile_failure: Option<String>,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_bytes: u64,
    pub cache_evictions: u64,
    pub gc_allocations: u64,
    pub gc_minor_collections: u64,
    pub gc_major_collections: u64,
    pub gc_pause_micros: u64,
    pub gc_bytes: u64,
    pub gc_promotions: u64,
    pub invalidations: u64,
    pub static_fact_matches: u64,
    pub readiness: AdaptiveReadinessSourceCounts,
}

impl AdaptiveReport {
    pub(crate) fn new() -> Self {
        Self {
            schema_version: 1,
            runtime_core: RuntimeCore::AdaptiveV2,
            default_core: RuntimeCore::Legacy,
            qualified_for_default: false,
            rollback_available: true,
            regions: Vec::new(),
            traces: 0,
            bridges: 0,
            selected_snapshot_id: None,
            tier1_snapshot_id: None,
            tier2_snapshot_id: None,
            machine_entries: 0,
            native_executions: 0,
            helper_calls: 0,
            generic_dispatch_calls: 0,
            guest_calls: 0,
            exits: 0,
            deopts: 0,
            materializations: 0,
            guard_failures: BTreeMap::new(),
            compile_latency_micros: 0,
            compile_tier: None,
            compile_failure: None,
            cache_hits: 0,
            cache_misses: 0,
            cache_bytes: 0,
            cache_evictions: 0,
            gc_allocations: 0,
            gc_minor_collections: 0,
            gc_major_collections: 0,
            gc_pause_micros: 0,
            gc_bytes: 0,
            gc_promotions: 0,
            invalidations: 0,
            static_fact_matches: 0,
            readiness: AdaptiveReadinessSourceCounts::default(),
        }
    }
}

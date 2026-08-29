use std::collections::HashMap;
use std::sync::Arc;

use crate::jit::CompilerBackend;
use crate::object::ObjectHeap;
use crate::planner::JitPolicy;

use super::{DEFAULT_HOT_THRESHOLD, DEFAULT_TIER2_THRESHOLD, JitReport, Vm};

impl Vm {
    pub fn new() -> Self {
        Self::with_hot_threshold(DEFAULT_HOT_THRESHOLD)
    }

    /// Creates a VM that tiers up after this many observed region entries.
    pub fn with_hot_threshold(hot_threshold: u64) -> Self {
        Self::with_tier_thresholds(hot_threshold, DEFAULT_TIER2_THRESHOLD)
    }

    pub fn with_tier_thresholds(hot_threshold: u64, tier2_threshold: u64) -> Self {
        Self::with_compiler_backend(hot_threshold, tier2_threshold, CompilerBackend::Tiered)
    }

    pub fn with_compiler_backend(
        hot_threshold: u64,
        tier2_threshold: u64,
        compiler_backend: CompilerBackend,
    ) -> Self {
        Self {
            hot_threshold,
            tier2_threshold,
            compiler_backend: Some(compiler_backend),
            jit_report: JitReport::default(),
            jit_policy: JitPolicy::Profile,
            dump_wxir: false,
            runtimes: HashMap::new(),
            last_executed: None,
            object_heap: ObjectHeap::new(),
            call_depth: 0,
            frame_pool: HashMap::new(),
            verified_functions: Default::default(),
            adaptive_v2: None,
            adaptive_execution_id: super::next_execution_id(),
            last_adaptive_report: None,
            defer_adaptive_report_sync: false,
        }
    }

    pub fn new_adaptive_v2() -> Self {
        Self::with_adaptive_v2_backend(
            DEFAULT_HOT_THRESHOLD,
            DEFAULT_TIER2_THRESHOLD,
            CompilerBackend::Tiered,
        )
    }

    pub(crate) fn with_adaptive_v2_backend(
        hot_threshold: u64,
        tier2_threshold: u64,
        compiler_backend: CompilerBackend,
    ) -> Self {
        let mut vm = Self::interpreter();
        vm.hot_threshold = hot_threshold;
        vm.tier2_threshold = tier2_threshold;
        vm.adaptive_v2 = Some(Arc::new(crate::adaptive_v2::integration::AdaptiveVm::new(
            Some(compiler_backend),
        )));
        vm
    }

    pub(crate) fn with_shared_adaptive_v2_backend(
        hot_threshold: u64,
        tier2_threshold: u64,
        adaptive_v2: Arc<crate::adaptive_v2::integration::AdaptiveVm>,
    ) -> Self {
        let mut vm = Self::interpreter();
        vm.hot_threshold = hot_threshold;
        vm.tier2_threshold = tier2_threshold;
        vm.adaptive_v2 = Some(adaptive_v2);
        vm
    }

    pub(crate) fn adaptive_v2_interpreter() -> Self {
        let mut vm = Self::interpreter();
        vm.adaptive_v2 = Some(Arc::new(crate::adaptive_v2::integration::AdaptiveVm::new(
            None,
        )));
        vm
    }

    pub fn interpreter() -> Self {
        Self {
            hot_threshold: u64::MAX,
            tier2_threshold: u64::MAX,
            compiler_backend: None,
            jit_report: JitReport::default(),
            jit_policy: JitPolicy::Profile,
            dump_wxir: false,
            runtimes: HashMap::new(),
            last_executed: None,
            object_heap: ObjectHeap::new(),
            call_depth: 0,
            frame_pool: HashMap::new(),
            verified_functions: Default::default(),
            adaptive_v2: None,
            adaptive_execution_id: super::next_execution_id(),
            last_adaptive_report: None,
            defer_adaptive_report_sync: false,
        }
    }

    pub fn set_hot_threshold(&mut self, hot_threshold: u64) {
        self.hot_threshold = hot_threshold;
    }

    pub fn set_tier2_threshold(&mut self, tier2_threshold: u64) {
        self.tier2_threshold = tier2_threshold;
    }

    pub fn set_jit_policy(&mut self, policy: JitPolicy) {
        self.jit_policy = policy;
    }

    pub fn set_dump_wxir(&mut self, enabled: bool) {
        self.dump_wxir = enabled;
    }
}

use std::collections::HashMap;

use crate::executable::ExecutableFunction;
use crate::jit::CompilerBackend;
use crate::planner::{RegionPlanRequest, plan_region};
use crate::structure_map::RegionId;
use crate::wxir::{WxBuildError, build_profiled_region, print_function};

use super::{Frame, FunctionRuntime, JitFailure, JitFailureStage, Vm};

mod backend;
mod execution;
#[cfg(feature = "inkwell")]
mod promotion;

use self::backend::{BackendCompiler, InitialRegion};

pub(super) struct JitRuntime {
    regions: HashMap<RegionId, RegionState>,
    entry_mismatch_streaks: HashMap<RegionId, u8>,
    compiler: BackendCompiler,
    function: Option<String>,
}

enum RegionState {
    Cranelift {
        region: Box<crate::jit::CompiledRegion>,
    },
    #[cfg(feature = "inkwell")]
    Tier1 {
        region: Box<crate::jit::CompiledRegion>,
        function: Box<crate::wxir::VerifiedWxFunction>,
        native_executions: u64,
        tier2_available: bool,
    },
    #[cfg(feature = "inkwell")]
    Llvm {
        region: Box<crate::jit::CompiledRegion>,
    },
    Disabled {
        reason: String,
    },
}

impl JitRuntime {
    pub(super) fn new(executable: &ExecutableFunction, backend: CompilerBackend) -> Self {
        Self {
            regions: HashMap::new(),
            entry_mismatch_streaks: HashMap::new(),
            compiler: BackendCompiler::new(executable.id(), backend),
            function: executable.name().map(str::to_string),
        }
    }
}

impl Vm {
    pub(super) fn try_execute_region(
        &mut self,
        executable: &ExecutableFunction,
        frame: &mut Frame,
        runtime: &mut FunctionRuntime,
    ) -> bool {
        let Some(mut jit) = runtime.jit.take() else {
            return false;
        };
        let executed = self.try_execute_region_with_jit(executable, frame, runtime, &mut jit);
        runtime.jit = Some(jit);
        executed
    }

    fn try_execute_region_with_jit(
        &mut self,
        executable: &ExecutableFunction,
        frame: &mut Frame,
        runtime: &mut FunctionRuntime,
        jit: &mut JitRuntime,
    ) -> bool {
        if frame.suppress_osr_pc == Some(frame.pc) {
            frame.suppress_osr_pc = None;
            return false;
        }
        let Some(region_id) = executable.structure_map().region_by_entry_pc(frame.pc) else {
            return false;
        };
        if frame.suppressed_regions.contains(&region_id) {
            return false;
        }
        if jit.regions.contains_key(&region_id) {
            return self.execute_cached_region(region_id, executable, frame, runtime, jit);
        }
        let Some(ready_profile) =
            runtime
                .profile
                .ready_region(executable.structure_map(), region_id, self.hot_threshold)
        else {
            return false;
        };
        let Some(plan) = plan_region(
            executable.structure_map(),
            ready_profile,
            RegionPlanRequest {
                policy: self.jit_policy,
                threshold: self.hot_threshold,
                region_id,
            },
        ) else {
            return false;
        };
        self.jit_report.compilation_attempts += 1;
        let function = match build_profiled_region(executable, &plan, ready_profile) {
            Ok(function) => function,
            Err(error @ WxBuildError::TypeMismatch { .. }) => {
                let failure =
                    JitFailure::from_wxir(region_id, &error).at_function(jit.function.clone());
                if !self.jit_report.failures.iter().any(|existing| {
                    existing.region_id == failure.region_id
                        && existing.stage == failure.stage
                        && existing.reason == failure.reason
                }) {
                    self.jit_report.failures.push(failure);
                }
                runtime.profile.invalidate_region(region_id);
                self.jit_report.call_sites.call_guard_miss =
                    self.jit_report.call_sites.call_guard_miss.saturating_add(1);
                return false;
            }
            Err(error) => {
                let failure =
                    JitFailure::from_wxir(region_id, &error).at_function(jit.function.clone());
                self.disable_region(jit, failure);
                return false;
            }
        };
        let initial_tier_is_llvm = jit.compiler.initial_tier_is_llvm();
        if initial_tier_is_llvm {
            self.jit_report.tier2_compilation_attempts += 1;
        }
        if self.dump_wxir {
            eprint!("{}", print_function(function.as_function()));
        }
        let region = match jit.compiler.compile_initial(function) {
            Ok(region) => region,
            Err(error) => {
                let failure = JitFailure::new(
                    region_id,
                    if initial_tier_is_llvm {
                        JitFailureStage::CompileTier2
                    } else {
                        JitFailureStage::Compile
                    },
                    error.to_string(),
                )
                .at_function(jit.function.clone());
                self.disable_region(jit, failure);
                return false;
            }
        };
        self.jit_report.compiled_regions += 1;
        if initial_tier_is_llvm {
            self.jit_report.tier2_compiled_regions += 1;
        }
        let state = match region {
            InitialRegion::Cranelift(region) => RegionState::Cranelift { region },
            #[cfg(feature = "inkwell")]
            InitialRegion::Tier1 { region, function } => RegionState::Tier1 {
                region,
                function,
                native_executions: 0,
                tier2_available: true,
            },
            #[cfg(feature = "inkwell")]
            InitialRegion::Llvm(region) => RegionState::Llvm { region },
        };
        jit.regions.insert(region_id, state);
        jit.entry_mismatch_streaks.remove(&region_id);
        self.execute_cached_region(region_id, executable, frame, runtime, jit)
    }
}

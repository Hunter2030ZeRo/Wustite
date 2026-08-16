use std::collections::HashMap;

use crate::executable::ExecutableFunction;
use crate::jit::{CompilerBackend, ExecuteError};
use crate::planner::plan_hot_region;
use crate::structure_map::RegionId;
use crate::wxir::{WxExitKind, build_verified_region};

use super::{Frame, FunctionRuntime, JitFailure, JitFailureStage, Vm};

mod backend;

use self::backend::{BackendCompiler, InitialRegion};

pub(super) struct JitRuntime {
    regions: HashMap<RegionId, RegionState>,
    compiler: BackendCompiler,
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
            compiler: BackendCompiler::new(executable.id(), backend),
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
        let Some(jit) = runtime.jit.as_mut() else {
            return false;
        };
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
            return self.execute_cached_region(region_id, frame, jit);
        }
        let Some(plan) = plan_hot_region(
            executable.structure_map(),
            &runtime.profile,
            self.hot_threshold,
            region_id,
        ) else {
            return false;
        };
        self.jit_report.compilation_attempts += 1;
        let function = match build_verified_region(executable, &plan) {
            Ok(function) => function,
            Err(error) => {
                self.disable_region(
                    jit,
                    JitFailure {
                        region_id,
                        stage: JitFailureStage::BuildWxir,
                        reason: error.to_string(),
                    },
                );
                return false;
            }
        };
        let initial_tier_is_llvm = jit.compiler.initial_tier_is_llvm();
        if initial_tier_is_llvm {
            self.jit_report.tier2_compilation_attempts += 1;
        }
        let region = match jit.compiler.compile_initial(function) {
            Ok(region) => region,
            Err(error) => {
                self.disable_region(
                    jit,
                    JitFailure {
                        region_id,
                        stage: if initial_tier_is_llvm {
                            JitFailureStage::CompileTier2
                        } else {
                            JitFailureStage::Compile
                        },
                        reason: error.to_string(),
                    },
                );
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
        self.execute_cached_region(region_id, frame, jit)
    }

    fn execute_cached_region(
        &mut self,
        region_id: RegionId,
        frame: &mut Frame,
        jit: &mut JitRuntime,
    ) -> bool {
        #[cfg(feature = "inkwell")]
        self.promote_region_if_ready(region_id, jit);

        let (execution, tier2) = match jit.regions.get_mut(&region_id) {
            Some(RegionState::Cranelift { region }) => {
                (region.execute(&mut frame.registers), false)
            }
            #[cfg(feature = "inkwell")]
            Some(RegionState::Tier1 {
                region,
                function: _,
                native_executions,
                tier2_available: _,
            }) => {
                let execution = region.execute(&mut frame.registers);
                if execution.is_ok() {
                    *native_executions = native_executions.saturating_add(1);
                }
                (execution, false)
            }
            #[cfg(feature = "inkwell")]
            Some(RegionState::Llvm { region }) => (region.execute(&mut frame.registers), true),
            Some(RegionState::Disabled { reason }) => {
                let _ = reason;
                return false;
            }
            None => return false,
        };
        match execution {
            Ok(execution) => {
                self.jit_report.native_executions += 1;
                if tier2 {
                    self.jit_report.tier2_native_executions += 1;
                }
                self.jit_report.last_resume_pc = Some(execution.resume_pc);
                self.jit_report.last_exit_kind = Some(execution.kind);
                frame.pc = execution.resume_pc;
                frame.suppress_osr_pc = match execution.kind {
                    WxExitKind::ReplayInstruction => Some(execution.resume_pc),
                    WxExitKind::RegionExit | WxExitKind::Deopt => None,
                };
                true
            }
            Err(ExecuteError::EntryTypeMismatch { .. }) => {
                frame.suppressed_regions.insert(region_id);
                false
            }
            Err(error) => {
                self.disable_region(
                    jit,
                    JitFailure {
                        region_id,
                        stage: JitFailureStage::Execute,
                        reason: error.to_string(),
                    },
                );
                false
            }
        }
    }

    #[cfg(feature = "inkwell")]
    fn promote_region_if_ready(&mut self, region_id: RegionId, jit: &mut JitRuntime) {
        let (regions, compiler) = (&mut jit.regions, &mut jit.compiler);
        let Some(state) = regions.get_mut(&region_id) else {
            return;
        };
        let RegionState::Tier1 {
            function,
            native_executions,
            tier2_available,
            ..
        } = state
        else {
            return;
        };
        if !*tier2_available || *native_executions < self.tier2_threshold.max(1) {
            return;
        }

        let Some(compilation) = compiler.compile_tier2(function) else {
            return;
        };
        self.jit_report.tier2_compilation_attempts += 1;
        match compilation {
            Ok(region) => {
                *state = RegionState::Llvm {
                    region: Box::new(region),
                };
                self.jit_report.tier2_compiled_regions += 1;
            }
            Err(error) => {
                *tier2_available = false;
                self.jit_report.failures.push(JitFailure {
                    region_id,
                    stage: JitFailureStage::CompileTier2,
                    reason: error.to_string(),
                });
            }
        }
    }

    fn disable_region(&mut self, jit: &mut JitRuntime, failure: JitFailure) {
        self.jit_report.disabled_regions += 1;
        jit.regions.insert(
            failure.region_id,
            RegionState::Disabled {
                reason: failure.reason.clone(),
            },
        );
        self.jit_report.failures.push(failure);
    }
}

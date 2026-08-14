use std::collections::HashMap;

use crate::executable::ExecutableFunction;
#[cfg(feature = "inkwell")]
use crate::jit::LlvmRegionCompiler;
use crate::jit::{CompiledRegion, CraneliftRegionCompiler, ExecuteError};
use crate::planner::plan_hot_region;
use crate::structure_map::RegionId;
#[cfg(feature = "inkwell")]
use crate::wxir::VerifiedWxFunction;
use crate::wxir::{WxExitKind, build_verified_region};

use super::{Frame, FunctionRuntime, JitFailure, JitFailureStage, Vm};

pub(super) struct JitRuntime {
    regions: HashMap<RegionId, RegionState>,
    region_by_header: Vec<Option<RegionId>>,
    tier1_compiler: Box<CraneliftRegionCompiler>,
    #[cfg(feature = "inkwell")]
    tier2_compiler: Box<LlvmRegionCompiler>,
}

enum RegionState {
    Tier1 {
        region: Box<CompiledRegion>,
        #[cfg(feature = "inkwell")]
        function: Box<VerifiedWxFunction>,
        #[cfg(feature = "inkwell")]
        native_executions: u64,
        #[cfg(feature = "inkwell")]
        tier2_available: bool,
    },
    #[cfg(feature = "inkwell")]
    Tier2 {
        region: Box<CompiledRegion>,
    },
    Disabled {
        reason: String,
    },
}

impl JitRuntime {
    pub(super) fn new(executable: &ExecutableFunction) -> Self {
        let mut region_by_header = vec![None; executable.bytecode().code.len()];
        for (index, region) in executable.structure_map().loops.iter().enumerate() {
            region_by_header[region.header] = Some(RegionId(index));
        }
        Self {
            regions: HashMap::new(),
            region_by_header,
            tier1_compiler: Box::new(CraneliftRegionCompiler::new(executable.id())),
            #[cfg(feature = "inkwell")]
            tier2_compiler: Box::new(LlvmRegionCompiler::new(executable.id())),
        }
    }

    pub(super) fn region_at(&self, pc: usize) -> Option<RegionId> {
        self.region_by_header.get(pc).copied().flatten()
    }
}

impl Vm {
    pub(super) fn try_execute_region(
        &mut self,
        executable: &ExecutableFunction,
        frame: &mut Frame,
        runtime: &mut FunctionRuntime,
    ) -> bool {
        if frame.suppress_osr_pc == Some(frame.pc) {
            frame.suppress_osr_pc = None;
            return false;
        }
        let Some(region_id) = runtime.jit.region_at(frame.pc) else {
            return false;
        };
        if frame.suppressed_regions.contains(&region_id) {
            return false;
        }
        if runtime.jit.regions.contains_key(&region_id) {
            return self.execute_cached_region(region_id, frame, &mut runtime.jit);
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
                    &mut runtime.jit,
                    JitFailure {
                        region_id,
                        stage: JitFailureStage::BuildWxir,
                        reason: error.to_string(),
                    },
                );
                return false;
            }
        };
        let region = match runtime.jit.tier1_compiler.compile_verified(&function) {
            Ok(region) => region,
            Err(error) => {
                self.disable_region(
                    &mut runtime.jit,
                    JitFailure {
                        region_id,
                        stage: JitFailureStage::Compile,
                        reason: error.to_string(),
                    },
                );
                return false;
            }
        };
        self.jit_report.compiled_regions += 1;
        runtime.jit.regions.insert(
            region_id,
            RegionState::Tier1 {
                region: Box::new(region),
                #[cfg(feature = "inkwell")]
                function: Box::new(function),
                #[cfg(feature = "inkwell")]
                native_executions: 0,
                #[cfg(feature = "inkwell")]
                tier2_available: cfg!(feature = "inkwell"),
            },
        );
        self.execute_cached_region(region_id, frame, &mut runtime.jit)
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
            Some(RegionState::Tier1 {
                region,
                #[cfg(feature = "inkwell")]
                    function: _,
                #[cfg(feature = "inkwell")]
                native_executions,
                #[cfg(feature = "inkwell")]
                    tier2_available: _,
            }) => {
                let execution = region.execute(&mut frame.registers);
                #[cfg(feature = "inkwell")]
                if execution.is_ok() {
                    *native_executions = native_executions.saturating_add(1);
                }
                (execution, false)
            }
            #[cfg(feature = "inkwell")]
            Some(RegionState::Tier2 { region }) => (region.execute(&mut frame.registers), true),
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
        let (regions, compiler) = (&mut jit.regions, &mut jit.tier2_compiler);
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

        self.jit_report.tier2_compilation_attempts += 1;
        match compiler.compile_verified(function) {
            Ok(region) => {
                *state = RegionState::Tier2 {
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

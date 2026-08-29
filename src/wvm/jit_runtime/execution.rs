use crate::executable::ExecutableFunction;
use crate::jit::ExecuteError;
use crate::structure_map::RegionId;
use crate::wvm::native_jit::NativeDispatcher;
use crate::wvm::{Frame, FunctionRuntime, JitFailure, JitFailureStage, Vm};
use crate::wxir::WxExitKind;

use super::{JitRuntime, RegionState};

impl Vm {
    pub(super) fn execute_cached_region(
        &mut self,
        region_id: RegionId,
        executable: &ExecutableFunction,
        frame: &mut Frame,
        runtime: &mut FunctionRuntime,
        jit: &mut JitRuntime,
    ) -> bool {
        #[cfg(feature = "inkwell")]
        self.promote_region_if_ready(region_id, jit);

        let (execution, tier2) = match jit.regions.get_mut(&region_id) {
            Some(RegionState::Cranelift { region }) => {
                let mut dispatch = NativeDispatcher::new(self, executable, runtime, &mut frame.pc);
                (
                    region.execute_with_dispatch(&mut frame.registers, &mut dispatch),
                    false,
                )
            }
            #[cfg(feature = "inkwell")]
            Some(RegionState::Tier1 {
                region,
                function: _,
                native_executions,
                tier2_available: _,
            }) => {
                let mut dispatch = NativeDispatcher::new(self, executable, runtime, &mut frame.pc);
                let execution = region.execute_with_dispatch(&mut frame.registers, &mut dispatch);
                if execution.is_ok() {
                    *native_executions = native_executions.saturating_add(1);
                }
                (execution, false)
            }
            #[cfg(feature = "inkwell")]
            Some(RegionState::Llvm { region }) => {
                let mut dispatch = NativeDispatcher::new(self, executable, runtime, &mut frame.pc);
                (
                    region.execute_with_dispatch(&mut frame.registers, &mut dispatch),
                    true,
                )
            }
            Some(RegionState::Disabled { reason }) => {
                let _ = reason;
                return false;
            }
            None => return false,
        };
        match execution {
            Ok(execution) => {
                jit.entry_mismatch_streaks.remove(&region_id);
                self.jit_report.native_executions += 1;
                self.jit_report
                    .record_native_execution(jit.function.as_deref());
                if tier2 {
                    self.jit_report.tier2_native_executions += 1;
                }
                self.jit_report.last_resume_pc = Some(execution.resume_pc);
                self.jit_report.last_exit_kind = Some(execution.kind);
                self.jit_report.record_exit(execution.kind);
                frame.pc = execution.resume_pc;
                frame.suppress_osr_pc = match execution.kind {
                    WxExitKind::ReplayInstruction => Some(execution.resume_pc),
                    WxExitKind::RegionExit | WxExitKind::Deopt => None,
                };
                true
            }
            Err(ExecuteError::EntryTypeMismatch { .. }) => {
                let streak = jit.entry_mismatch_streaks.entry(region_id).or_default();
                *streak = streak.saturating_add(1);
                if *streak >= crate::profiler::READY_ENTRY_SAMPLES {
                    jit.entry_mismatch_streaks.remove(&region_id);
                    jit.regions.remove(&region_id);
                    runtime.profile.invalidate_region(region_id);
                }
                self.jit_report.call_sites.call_guard_miss =
                    self.jit_report.call_sites.call_guard_miss.saturating_add(1);
                false
            }
            Err(ExecuteError::Runtime(reason))
                if reason.starts_with("native JIT read type mismatch") =>
            {
                jit.regions.remove(&region_id);
                runtime.profile.invalidate_region(region_id);
                self.jit_report.last_resume_pc = Some(frame.pc);
                self.jit_report.last_exit_kind = Some(WxExitKind::Deopt);
                self.jit_report.record_exit(WxExitKind::Deopt);
                false
            }
            Err(error) => {
                let failure =
                    JitFailure::new(region_id, JitFailureStage::Execute, error.to_string())
                        .at_function(jit.function.clone());
                self.disable_region(jit, failure);
                false
            }
        }
    }

    pub(super) fn disable_region(&mut self, jit: &mut JitRuntime, failure: JitFailure) {
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

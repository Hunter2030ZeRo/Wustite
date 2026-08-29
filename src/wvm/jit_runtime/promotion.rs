use crate::structure_map::RegionId;
use crate::wxir::print_function;

use super::{JitRuntime, RegionState};
use crate::wvm::{JitFailure, JitFailureStage, Vm};

impl Vm {
    pub(super) fn promote_region_if_ready(&mut self, region_id: RegionId, jit: &mut JitRuntime) {
        let function_name = jit.function.clone();
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

        if self.dump_wxir {
            eprint!("{}", print_function(function.as_function()));
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
                self.jit_report.failures.push(
                    JitFailure::new(region_id, JitFailureStage::CompileTier2, error.to_string())
                        .at_function(function_name),
                );
            }
        }
    }
}

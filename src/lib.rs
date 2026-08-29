pub mod bytecode;
pub mod executable;
pub mod frontend;
pub mod jit;
pub mod metrics;
pub mod object;
pub mod planner;
pub mod profiler;
pub mod runtime;
pub mod structure_map;
pub mod value;
pub mod verifier;
pub mod wvm;
pub mod wxir;

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "adaptive v2 remains private until migration activation"
    )
)]
mod adaptive_v2;

pub use jit::CompilerBackend;
pub use metrics::{CompilationMetrics, ExecutionMetrics};
pub use object::{Object, ObjectRef};
pub use planner::JitPolicy;
pub use runtime::{
    AdaptiveReadinessSourceCounts, AdaptiveRegionReport, AdaptiveReport, ExecutableInfo,
    ExecutionMode, RegionInfo, RootedResult, Runtime, RuntimeConfig, RuntimeCore, RuntimeError,
    RuntimeValue, SharedRuntime,
};
pub use wvm::Vm;

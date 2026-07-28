use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompilationMetrics {
    pub frontend_time: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionMetrics {
    pub total_time: Duration,
    pub verification_time: Duration,

    pub wxir_build_time: Duration,
    pub native_compile_time: Duration,
    pub native_execution_time: Duration,

    pub interpreted_instructions: u64,
    pub region_entries: u64,
    pub interpreted_region_entries: u64,
    pub native_region_executions: u64,
    pub cached_region_dispatches: u64,

    pub region_exits: u64,
    pub replay_instruction_exits: u64,
    pub deopt_exits: u64,
}

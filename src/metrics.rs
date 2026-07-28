use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompilationMetrics {
    pub frontend_time: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionMetrics {
    pub total_time: Duration,
}

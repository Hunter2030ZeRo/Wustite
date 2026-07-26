#[derive(Debug)]
pub struct Profile {
    execution_counts: Vec<u64>,
}

impl Profile {
    pub fn new(instruction_count: usize) -> Self {
        Self {
            execution_counts: vec![0; instruction_count],
        }
    }

    pub fn record(&mut self, pc: usize) {
        if let Some(count) = self.execution_counts.get_mut(pc) {
            *count = count.saturating_add(1);
        }
    }

    pub fn count(&self, pc: usize) -> u64 {
        self.execution_counts.get(pc).copied().unwrap_or(0)
    }

    pub fn is_hot(&self, pc: usize, threshold: u64) -> bool {
        self.execution_counts
            .get(pc)
            .is_some_and(|count| *count >= threshold)
    }
}

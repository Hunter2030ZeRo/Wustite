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
        self.execution_counts[pc] += 1;
    }

    pub fn count(&self, pc: usize) -> u64 {
        self.execution_counts[pc]
    }

    pub fn is_hot(&self, pc: usize, threshold: u64) -> bool {
        self.count(pc) >= threshold
    }
}
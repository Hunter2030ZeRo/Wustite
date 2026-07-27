use crate::structure_map::RegionId;

#[derive(Debug)]
pub struct Profile {
    region_execution_counts: Vec<u64>,
}

impl Profile {
    pub fn new(region_count: usize) -> Self {
        Self {
            region_execution_counts: vec![0; region_count],
        }
    }

    pub fn record(&mut self, region_id: RegionId) {
        if let Some(count) = self.region_execution_counts.get_mut(region_id.0) {
            *count = count.saturating_add(1);
        }
    }

    pub fn count(&self, region_id: RegionId) -> u64 {
        self.region_execution_counts
            .get(region_id.0)
            .copied()
            .unwrap_or(0)
    }

    pub fn is_hot(&self, region_id: RegionId, threshold: u64) -> bool {
        self.region_execution_counts
            .get(region_id.0)
            .is_some_and(|count| *count >= threshold)
    }
}

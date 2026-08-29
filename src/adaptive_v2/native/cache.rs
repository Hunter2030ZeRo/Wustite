use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::adaptive_v2::wxir_v2::SnapshotId;
use crate::adaptive_v2::wxir_v2::dependency::Dependency;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeTier {
    Cranelift,
    Llvm,
    Bridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CacheKey {
    pub(crate) snapshot: SnapshotId,
    pub(crate) dependency_fingerprint: [u8; 32],
    pub(crate) tier: u8,
    pub(crate) derivation: SnapshotId,
}

impl CacheKey {
    pub(crate) fn new(
        snapshot: SnapshotId,
        dependencies: &[Dependency],
        tier: NativeTier,
        derivation: SnapshotId,
    ) -> Self {
        let mut hash = blake3::Hasher::new();
        for dependency in dependencies {
            hash.update(&[dependency.kind as u8]);
            hash.update(&dependency.identity.to_le_bytes());
            hash.update(&dependency.expected_epoch.to_le_bytes());
            hash.update(&dependency.observed_epoch.to_le_bytes());
        }
        Self {
            snapshot,
            dependency_fingerprint: *hash.finalize().as_bytes(),
            tier: tier as u8,
            derivation,
        }
    }
}

#[derive(Debug)]
struct CacheEntry<T> {
    value: T,
    bytes: usize,
    last_use: u64,
    active: u32,
    stale: bool,
}

#[derive(Debug)]
struct SharedState<T> {
    entries: BTreeMap<CacheKey, CacheEntry<T>>,
    evicted: Vec<T>,
    max_count: usize,
    max_bytes: usize,
    used_bytes: usize,
    clock: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SharedCodeCache<T> {
    state: Arc<Mutex<SharedState<T>>>,
}

impl<T> SharedCodeCache<T> {
    pub(crate) fn new(max_count: usize, max_bytes: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(SharedState {
                entries: BTreeMap::new(),
                evicted: Vec::new(),
                max_count,
                max_bytes,
                used_bytes: 0,
                clock: 0,
            })),
        }
    }

    pub(crate) fn insert(&self, key: CacheKey, bytes: usize, _epoch: u64, value: T) {
        let mut state = lock(&self.state);
        state.clock = state.clock.wrapping_add(1);
        let last_use = state.clock;
        state.used_bytes = state.used_bytes.saturating_add(bytes);
        if let Some(old) = state.entries.insert(
            key,
            CacheEntry {
                value,
                bytes,
                last_use,
                active: 0,
                stale: false,
            },
        ) {
            state.used_bytes = state.used_bytes.saturating_sub(old.bytes);
            state.evicted.push(old.value);
        }
        evict_shared(&mut state);
    }

    pub(crate) fn insert_and_lease(
        &self,
        key: CacheKey,
        bytes: usize,
        _epoch: u64,
        value: T,
    ) -> CacheLease<T> {
        let mut state = lock(&self.state);
        state.clock = state.clock.wrapping_add(1);
        let last_use = state.clock;
        state.used_bytes = state.used_bytes.saturating_add(bytes);
        if let Some(old) = state.entries.insert(
            key,
            CacheEntry {
                value,
                bytes,
                last_use,
                active: 1,
                stale: false,
            },
        ) {
            state.used_bytes = state.used_bytes.saturating_sub(old.bytes);
            state.evicted.push(old.value);
        }
        evict_shared(&mut state);
        CacheLease {
            key,
            state: Arc::clone(&self.state),
        }
    }

    pub(crate) fn lease(&self, key: CacheKey) -> Option<CacheLease<T>> {
        let mut state = lock(&self.state);
        state.clock = state.clock.wrapping_add(1);
        let clock = state.clock;
        let entry = state.entries.get_mut(&key)?;
        if entry.stale {
            return None;
        }
        entry.active = entry.active.saturating_add(1);
        entry.last_use = clock;
        Some(CacheLease {
            key,
            state: Arc::clone(&self.state),
        })
    }

    pub(crate) fn drain_evicted(&self) -> Vec<T> {
        std::mem::take(&mut lock(&self.state).evicted)
    }

    pub(crate) fn contains(&self, key: CacheKey) -> bool {
        lock(&self.state)
            .entries
            .get(&key)
            .is_some_and(|entry| !entry.stale)
    }

    pub(crate) fn invalidate_snapshot(&self, snapshot: SnapshotId) {
        let mut state = lock(&self.state);
        for (key, entry) in &mut state.entries {
            if key.snapshot == snapshot {
                entry.stale = true;
            }
        }
        evict_shared(&mut state);
    }
}

pub(crate) struct CacheLease<T> {
    key: CacheKey,
    state: Arc<Mutex<SharedState<T>>>,
}

impl<T> CacheLease<T> {
    pub(crate) fn with<R>(&self, use_value: impl FnOnce(&mut T) -> R) -> Option<R> {
        lock(&self.state)
            .entries
            .get_mut(&self.key)
            .map(|entry| use_value(&mut entry.value))
    }

    pub(crate) fn cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        lock(&self.state)
            .entries
            .get(&self.key)
            .map(|entry| entry.value.clone())
    }
}

impl<T> Drop for CacheLease<T> {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        if let Some(entry) = state.entries.get_mut(&self.key) {
            entry.active = entry.active.saturating_sub(1);
        }
        evict_shared(&mut state);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn evict_shared<T>(state: &mut SharedState<T>) {
    let stale = state
        .entries
        .iter()
        .filter(|(_, entry)| entry.stale && entry.active == 0)
        .map(|(key, _)| *key)
        .collect::<Vec<_>>();
    for key in stale {
        if let Some(entry) = state.entries.remove(&key) {
            state.used_bytes = state.used_bytes.saturating_sub(entry.bytes);
            state.evicted.push(entry.value);
        }
    }
    while state.entries.len() > state.max_count || state.used_bytes > state.max_bytes {
        let candidate = state
            .entries
            .iter()
            .filter(|(_, entry)| entry.active == 0)
            .min_by_key(|(key, entry)| (entry.last_use, **key))
            .map(|(key, _)| *key);
        let Some(key) = candidate else { break };
        if let Some(entry) = state.entries.remove(&key) {
            state.used_bytes = state.used_bytes.saturating_sub(entry.bytes);
            state.evicted.push(entry.value);
        }
    }
}

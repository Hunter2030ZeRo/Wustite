use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use super::cache::{CacheKey, NativeTier, SharedCodeCache};
use super::{NativeCode, NativeCompiler, NativeError, NativeOutcome, NativeValue};
use crate::adaptive_v2::wxir_v2::ir::SnapshotBody;
use crate::adaptive_v2::wxir_v2::{SnapshotId, VerifiedSnapshot};

const BRIDGE_THRESHOLD: u32 = 32;
const MAX_BRIDGES_PER_GUARD: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BridgeKey {
    pub(crate) parent: SnapshotId,
    pub(crate) guard: u32,
    pub(crate) observed_case: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureOrigin {
    Live,
    Cached,
    Static,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BridgeDecision {
    Profiling(u32),
    Compile { id: SnapshotId, key: BridgeKey },
    Existing(SnapshotId),
    Generic,
}

#[derive(Debug, Default)]
pub(crate) struct BridgeRegistry {
    failures: BTreeMap<BridgeKey, u32>,
    bridges: BTreeMap<BridgeKey, SnapshotId>,
    cases: BTreeMap<(SnapshotId, u32), BTreeSet<u8>>,
}

impl BridgeRegistry {
    pub(crate) fn observe(&mut self, key: BridgeKey, origin: FailureOrigin) -> BridgeDecision {
        if let Some(id) = self.bridges.get(&key) {
            return BridgeDecision::Existing(*id);
        }
        if !matches!(origin, FailureOrigin::Live) {
            return BridgeDecision::Profiling(*self.failures.get(&key).unwrap_or(&0));
        }
        let cases = self.cases.entry((key.parent, key.guard)).or_default();
        if !cases.contains(&key.observed_case) && cases.len() >= MAX_BRIDGES_PER_GUARD {
            return BridgeDecision::Generic;
        }
        let failures = self.failures.entry(key).or_default();
        *failures = failures.saturating_add(1);
        if *failures < BRIDGE_THRESHOLD {
            return BridgeDecision::Profiling(*failures);
        }
        cases.insert(key.observed_case);
        let id = bridge_id(key);
        BridgeDecision::Compile { id, key }
    }

    pub(crate) fn link(&mut self, key: BridgeKey, child: SnapshotId) {
        self.bridges.insert(key, child);
    }

    pub(crate) fn compilation_failed(&mut self, key: BridgeKey) {
        self.failures.remove(&key);
        if let Some(cases) = self.cases.get_mut(&(key.parent, key.guard)) {
            cases.remove(&key.observed_case);
        }
    }

    pub(crate) fn invalidate_parent(&mut self, parent: SnapshotId) {
        self.failures.retain(|key, _| key.parent != parent);
        self.bridges.retain(|key, _| key.parent != parent);
        self.cases.retain(|(candidate, _), _| *candidate != parent);
    }
}

fn bridge_id(key: BridgeKey) -> SnapshotId {
    let mut hash = blake3::Hasher::new();
    hash.update(&key.parent.as_bytes());
    hash.update(&key.guard.to_le_bytes());
    hash.update(&[key.observed_case]);
    SnapshotId::from_bytes(*hash.finalize().as_bytes())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BridgeLinkOutcome {
    Profiling(u32),
    Linked(SnapshotId),
    Existing(SnapshotId),
    Generic,
    Fallback,
}

pub(crate) struct BridgeRuntime {
    registry: BridgeRegistry,
    compiler: NativeCompiler,
    cache: SharedCodeCache<Rc<NativeCode>>,
    children: BTreeMap<BridgeKey, VerifiedSnapshot>,
}

impl BridgeRuntime {
    pub(crate) fn new(max_count: usize, max_bytes: usize) -> Self {
        Self {
            registry: BridgeRegistry::default(),
            compiler: NativeCompiler::new(),
            cache: SharedCodeCache::new(max_count, max_bytes),
            children: BTreeMap::new(),
        }
    }

    pub(crate) fn observe_and_link(
        &mut self,
        parent: &VerifiedSnapshot,
        guard: u32,
        observed_case: u8,
        origin: FailureOrigin,
        child_body: &SnapshotBody,
    ) -> BridgeLinkOutcome {
        let key = BridgeKey {
            parent: parent.id(),
            guard,
            observed_case,
        };
        match self.registry.observe(key, origin) {
            BridgeDecision::Profiling(count) => BridgeLinkOutcome::Profiling(count),
            BridgeDecision::Generic => BridgeLinkOutcome::Generic,
            BridgeDecision::Existing(id) => BridgeLinkOutcome::Existing(id),
            BridgeDecision::Compile { .. } => {
                let compiled = parent
                    .derive_bridge(guard, observed_case, child_body.clone())
                    .map_err(|_| NativeError::Backend("bridge verification".into()))
                    .and_then(|child| {
                        self.compiler
                            .compile_tier1(&child)
                            .map(|code| (child, code))
                    });
                let Ok((child, code)) = compiled else {
                    self.registry.compilation_failed(key);
                    return BridgeLinkOutcome::Fallback;
                };
                let child = self.compiler.selected_snapshot(&child).clone();
                let id = child.id();
                let cache_key =
                    CacheKey::new(id, &child.body().dependencies, NativeTier::Bridge, id);
                self.cache.insert(
                    cache_key,
                    super::estimated_code_bytes(&child),
                    child.body().schema_epoch,
                    Rc::new(code),
                );
                self.registry.link(key, id);
                self.children.insert(key, child);
                BridgeLinkOutcome::Linked(id)
            }
        }
    }

    pub(crate) fn execute_guard_target(
        &mut self,
        parent: SnapshotId,
        guard: u32,
        observed_case: u8,
        values: &[NativeValue],
    ) -> Result<Option<NativeOutcome>, NativeError> {
        let key = BridgeKey {
            parent,
            guard,
            observed_case,
        };
        let Some(child) = self.children.get(&key) else {
            return Ok(None);
        };
        let cache_key = CacheKey::new(
            child.id(),
            &child.body().dependencies,
            NativeTier::Bridge,
            child.id(),
        );
        let Some(lease) = self.cache.lease(cache_key) else {
            return Ok(None);
        };
        let code = lease
            .cloned()
            .ok_or_else(|| NativeError::Backend("bridge lease invalidated".into()))?;
        let outcome = code.execute(values)?;
        self.compiler.observe_tier1(&outcome)?;
        Ok(Some(outcome))
    }

    #[cfg(feature = "inkwell")]
    pub(crate) fn execute_tier2(
        &mut self,
        parent: SnapshotId,
        guard: u32,
        observed_case: u8,
        values: &[NativeValue],
    ) -> Result<NativeOutcome, NativeError> {
        let key = BridgeKey {
            parent,
            guard,
            observed_case,
        };
        let child = self
            .children
            .get(&key)
            .ok_or(NativeError::Tier1NotObserved)?;
        let cache_key = CacheKey::new(
            child.id(),
            &child.body().dependencies,
            NativeTier::Llvm,
            child.id(),
        );
        let lease = if let Some(lease) = self.cache.lease(cache_key) {
            lease
        } else {
            let code = self.compiler.compile_tier2(child)?;
            self.cache.insert_and_lease(
                cache_key,
                super::estimated_code_bytes(child),
                child.body().schema_epoch,
                Rc::new(code),
            )
        };
        lease
            .cloned()
            .ok_or_else(|| NativeError::Backend("bridge tier2 lease invalidated".into()))?
            .execute(values)
    }

    pub(crate) fn invalidate_parent(&mut self, parent: SnapshotId) {
        for child in self
            .children
            .iter()
            .filter(|(key, _)| key.parent == parent)
            .map(|(_, child)| child.id())
            .collect::<Vec<_>>()
        {
            self.cache.invalidate_snapshot(child);
        }
        self.children.retain(|key, _| key.parent != parent);
        self.registry.invalidate_parent(parent);
    }
}

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::adaptive_v2::native::bridge::{
    BridgeDecision, BridgeKey, BridgeRegistry, FailureOrigin,
};
use crate::adaptive_v2::native::cache::{CacheKey, NativeTier};
use crate::adaptive_v2::native::{NativeCompiler, NativeError, NativeOutcome, NativeValue};
use crate::adaptive_v2::wxir_v2::ir::{InstructionKind, SnapshotBody};
use crate::adaptive_v2::wxir_v2::{SnapshotId, VerifiedSnapshot};

use super::{CachedCode, TIER2_CODE};
use crate::adaptive_v2::integration::snapshot_cache_bytes;

pub(crate) struct BridgeSite {
    registry: BridgeRegistry,
    children: BTreeMap<BridgeKey, VerifiedSnapshot>,
    derivation: SnapshotId,
}

pub(crate) struct BridgeObservation {
    pub(crate) child: Option<CacheKey>,
    pub(super) replay: bool,
    pub(super) linked: bool,
    pub(super) cache_miss: bool,
    pub(super) cache_added_bytes: u64,
    pub(super) evictions: u64,
    pub(super) evicted_bytes: u64,
}

impl BridgeSite {
    pub(crate) fn new(derivation: SnapshotId) -> Self {
        Self {
            registry: BridgeRegistry::default(),
            children: BTreeMap::new(),
            derivation,
        }
    }

    pub(super) fn linked_child(
        &self,
        parent: &VerifiedSnapshot,
        values: &[NativeValue],
    ) -> Option<CacheKey> {
        let observed_case = observed_case(values);
        let (_, child) = self
            .children
            .iter()
            .find(|(key, _)| key.parent == parent.id() && key.observed_case == observed_case)?;
        Some(cache_key(self.derivation, child))
    }

    pub(crate) fn observe(
        &mut self,
        parent: &VerifiedSnapshot,
        compiler: &mut NativeCompiler,
        attempted: &NativeOutcome,
        values: &[NativeValue],
    ) -> Result<BridgeObservation, NativeError> {
        let key = BridgeKey {
            parent: parent.id(),
            guard: attempted.guard_id,
            observed_case: observed_case(values),
        };
        let mut linked = false;
        let mut cache_miss = false;
        let mut cache_added_bytes = 0;
        let mut evictions = 0;
        let mut evicted_bytes = 0;
        match self.registry.observe(key, FailureOrigin::Live) {
            BridgeDecision::Profiling(_) | BridgeDecision::Generic => {}
            BridgeDecision::Existing(_) => {}
            BridgeDecision::Compile { .. } => {
                let child = parent
                    .derive_bridge(key.guard, key.observed_case, bridge_body(parent, key.guard))
                    .map_err(|_| NativeError::Backend("bridge verification".into()))?;
                let code = compiler.compile_tier1(&child)?;
                let child = compiler.selected_snapshot(&child).clone();
                TIER2_CODE.with(|cache| {
                    cache.borrow().insert(
                        cache_key(self.derivation, &child),
                        usize::try_from(snapshot_cache_bytes(&child)).unwrap_or(usize::MAX),
                        child.body().schema_epoch,
                        CachedCode {
                            code: Rc::new(code),
                            bytes: snapshot_cache_bytes(&child),
                        },
                    );
                });
                let evicted = TIER2_CODE.with(|cache| cache.borrow().drain_evicted());
                evictions = u64::try_from(evicted.len()).unwrap_or(u64::MAX);
                evicted_bytes = evicted
                    .iter()
                    .fold(0_u64, |bytes, cached| bytes.saturating_add(cached.bytes));
                self.registry.link(key, child.id());
                cache_added_bytes = snapshot_cache_bytes(&child);
                self.children.insert(key, child);
                linked = true;
                cache_miss = true;
            }
        }
        let child = if self.children.contains_key(&key) {
            self.linked_child(parent, values)
        } else {
            None
        };
        Ok(BridgeObservation {
            replay: child.is_none(),
            child,
            linked,
            cache_miss,
            cache_added_bytes,
            evictions,
            evicted_bytes,
        })
    }
}

fn observed_case(values: &[NativeValue]) -> u8 {
    values
        .iter()
        .find_map(|value| match value {
            NativeValue::Boolean(value) => Some(u8::from(*value)),
            NativeValue::Integer(_) | NativeValue::FloatBits(_) | NativeValue::Handle(_) => None,
        })
        .unwrap_or(0)
}

fn bridge_body(snapshot: &VerifiedSnapshot, guard: u32) -> SnapshotBody {
    let mut body = snapshot.body().clone();
    for block in &mut body.blocks {
        block.instructions.retain(|instruction| {
            !matches!(
                instruction.kind.semantic(),
                InstructionKind::Guard { guard: candidate } if *candidate == guard
            )
        });
    }
    body
}

fn cache_key(derivation: SnapshotId, child: &VerifiedSnapshot) -> CacheKey {
    let mut hash = blake3::Hasher::new();
    hash.update(&derivation.as_bytes());
    hash.update(&child.id().as_bytes());
    CacheKey::new(
        child.id(),
        &child.body().dependencies,
        NativeTier::Bridge,
        SnapshotId::from_bytes(*hash.finalize().as_bytes()),
    )
}

pub(crate) fn execute_cached(
    key: CacheKey,
    values: &[NativeValue],
) -> Result<NativeOutcome, NativeError> {
    TIER2_CODE.with(|cache| {
        let cache = cache.borrow();
        let lease = cache
            .lease(key)
            .ok_or_else(|| NativeError::Backend("bridge cache entry missing".into()))?;
        let cached = lease
            .cloned()
            .ok_or_else(|| NativeError::Backend("bridge cache lease invalidated".into()))?;
        drop(lease);
        drop(cache);
        cached.code.execute(values)
    })
}

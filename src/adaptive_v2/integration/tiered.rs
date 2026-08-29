use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "inkwell")]
use std::time::Instant;

use crate::adaptive_v2::native::cache::SharedCodeCache;
#[cfg(feature = "inkwell")]
use crate::adaptive_v2::native::cache::{CacheKey, NativeTier};
use crate::adaptive_v2::native::{
    NativeCode, NativeCompiler, NativeError, NativeOutcome, NativeValue,
};
use crate::adaptive_v2::wxir_v2::{SnapshotId, VerifiedSnapshot};
use crate::jit::CompilerBackend;

use super::SharedTier1Code;
#[cfg(feature = "inkwell")]
use super::snapshot_cache_bytes;

pub(super) mod bridge;

const TIER2_THRESHOLD: u64 = crate::wvm::DEFAULT_TIER2_THRESHOLD;
const MAX_TIER2_CODE: usize = 64;
const MAX_TIER2_BYTES: usize = 64 * 1024 * 1024;

thread_local! {
    static TIER2_CODE: RefCell<SharedCodeCache<CachedCode>> =
        RefCell::new(SharedCodeCache::new(MAX_TIER2_CODE, MAX_TIER2_BYTES));
}

#[derive(Clone)]
pub(super) struct CachedCode {
    code: Rc<NativeCode>,
    bytes: u64,
}

pub(super) struct TieredSite {
    snapshot: VerifiedSnapshot,
    tier1: SharedTier1Code,
    compiler: Mutex<NativeCompiler>,
    backend: CompilerBackend,
    tier1_executions: AtomicU64,
    #[cfg(feature = "inkwell")]
    derivation: SnapshotId,
    bridges: Mutex<bridge::BridgeSite>,
    has_linked_bridge: AtomicBool,
}

pub(super) struct TieredExecution {
    pub(super) attempted: NativeOutcome,
    pub(super) bridge: Option<NativeOutcome>,
    pub(super) replay: bool,
    pub(super) bridge_linked: bool,
    pub(super) tier2: bool,
    pub(super) cache_miss: bool,
    pub(super) cache_added_bytes: u64,
    pub(super) evictions: u64,
    pub(super) evicted_bytes: u64,
    pub(super) compile_micros: u64,
}

impl TieredSite {
    pub(super) fn compile(
        snapshot: VerifiedSnapshot,
        backend: CompilerBackend,
        runtime_id: u64,
    ) -> Result<Self, NativeError> {
        if !matches!(
            backend,
            CompilerBackend::Cranelift | CompilerBackend::Tiered
        ) {
            return Err(NativeError::Unsupported(
                "adaptive-v2 tiering requires Cranelift tier 1",
            ));
        }
        let mut compiler = NativeCompiler::new();
        let tier1 = SharedTier1Code::new(compiler.compile_tier1(&snapshot)?)
            .map_err(NativeError::Backend)?;
        let snapshot = compiler.selected_snapshot(&snapshot).clone();
        let mut hash = blake3::Hasher::new();
        hash.update(&snapshot.id().as_bytes());
        hash.update(&runtime_id.to_le_bytes());
        Ok(Self {
            snapshot,
            tier1,
            compiler: Mutex::new(compiler),
            backend,
            tier1_executions: AtomicU64::new(0),
            #[cfg(feature = "inkwell")]
            derivation: SnapshotId::from_bytes(*hash.finalize().as_bytes()),
            bridges: Mutex::new(bridge::BridgeSite::new(SnapshotId::from_bytes(
                *hash.finalize().as_bytes(),
            ))),
            has_linked_bridge: AtomicBool::new(false),
        })
    }

    pub(super) const fn snapshot(&self) -> &VerifiedSnapshot {
        &self.snapshot
    }

    pub(super) fn resume_pc(&self, exit_id: u32) -> Option<usize> {
        self.snapshot
            .body()
            .deopts
            .iter()
            .find(|recipe| recipe.id == exit_id)
            .and_then(|recipe| usize::try_from(recipe.resume_pc).ok())
    }

    pub(super) fn execute(&self, values: &[NativeValue]) -> Result<TieredExecution, NativeError> {
        if self.has_linked_bridge.load(Ordering::Acquire) {
            let child = self
                .bridges
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .linked_child(&self.snapshot, values);
            if let Some(child) = child {
                let outcome = bridge::execute_cached(child, values)?;
                self.compiler
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .observe_tier1(&outcome)?;
                return Ok(bridge_execution(outcome));
            }
        }
        if self.backend == CompilerBackend::Tiered
            && self.tier1_executions.load(Ordering::Acquire) >= TIER2_THRESHOLD
        {
            #[cfg(feature = "inkwell")]
            {
                return self.execute_tier2(values);
            }
        }
        let outcome = self.tier1.code.execute(values)?;
        self.compiler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .observe_tier1(&outcome)?;
        self.tier1_executions.fetch_add(1, Ordering::AcqRel);
        if outcome.guard_id != 0 && (outcome.exit_id != 0 || outcome.counters.deopts != 0) {
            let mut compiler = self
                .compiler
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut bridges = self
                .bridges
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let observed = bridges.observe(&self.snapshot, &mut compiler, &outcome, values)?;
            if observed.linked {
                self.has_linked_bridge.store(true, Ordering::Release);
            }
            drop(bridges);
            drop(compiler);
            let bridge = observed
                .child
                .map(|child| bridge::execute_cached(child, values))
                .transpose()?;
            if let Some(bridge) = bridge.as_ref() {
                self.compiler
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .observe_tier1(bridge)?;
            }
            return Ok(TieredExecution {
                attempted: outcome,
                bridge,
                replay: observed.replay,
                bridge_linked: observed.linked,
                tier2: false,
                cache_miss: observed.cache_miss,
                cache_added_bytes: observed.cache_added_bytes,
                evictions: observed.evictions,
                evicted_bytes: observed.evicted_bytes,
                compile_micros: 0,
            });
        }
        Ok(TieredExecution {
            attempted: outcome,
            bridge: None,
            replay: false,
            bridge_linked: false,
            tier2: false,
            cache_miss: false,
            cache_added_bytes: 0,
            evictions: 0,
            evicted_bytes: 0,
            compile_micros: 0,
        })
    }

    #[cfg(feature = "inkwell")]
    fn execute_tier2(&self, values: &[NativeValue]) -> Result<TieredExecution, NativeError> {
        let key = CacheKey::new(
            self.snapshot.id(),
            &self.snapshot.body().dependencies,
            NativeTier::Llvm,
            self.derivation,
        );
        let mut cache_miss = false;
        let mut compile_micros = 0;
        let outcome = TIER2_CODE.with(|cache| {
            let cache = cache.borrow_mut();
            let lease = if let Some(lease) = cache.lease(key) {
                lease
            } else {
                let started = Instant::now();
                let code = self
                    .compiler
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .compile_tier2(&self.snapshot)?;
                compile_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                cache_miss = true;
                cache.insert_and_lease(
                    key,
                    usize::try_from(snapshot_cache_bytes(&self.snapshot)).unwrap_or(usize::MAX),
                    self.snapshot.body().schema_epoch,
                    CachedCode {
                        code: Rc::new(code),
                        bytes: snapshot_cache_bytes(&self.snapshot),
                    },
                )
            };
            let cached = lease
                .cloned()
                .ok_or_else(|| NativeError::Backend("tier-2 cache lease invalidated".into()))?;
            drop(lease);
            drop(cache);
            cached.code.execute(values)
        })?;
        let evicted = TIER2_CODE.with(|cache| cache.borrow().drain_evicted());
        let evictions = u64::try_from(evicted.len()).unwrap_or(u64::MAX);
        let evicted_bytes = evicted
            .iter()
            .fold(0_u64, |bytes, cached| bytes.saturating_add(cached.bytes));
        Ok(TieredExecution {
            attempted: outcome,
            bridge: None,
            replay: false,
            bridge_linked: false,
            tier2: true,
            cache_miss,
            cache_added_bytes: if cache_miss {
                snapshot_cache_bytes(&self.snapshot)
            } else {
                0
            },
            evictions,
            evicted_bytes,
            compile_micros,
        })
    }
}

fn bridge_execution(outcome: NativeOutcome) -> TieredExecution {
    TieredExecution {
        attempted: outcome,
        bridge: None,
        replay: false,
        bridge_linked: false,
        tier2: false,
        cache_miss: false,
        cache_added_bytes: 0,
        evictions: 0,
        evicted_bytes: 0,
        compile_micros: 0,
    }
}

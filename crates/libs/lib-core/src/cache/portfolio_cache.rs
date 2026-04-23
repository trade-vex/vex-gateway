//! Stale-while-revalidate cache over a `PortfolioRepository`.
//!
//! Semantics requested by the portfolio workload:
//!
//! * **Idle eviction.** If a (user, method, resolution) cache slot has not
//!   been read for `IDLE_TTL_SECONDS` (60s), drop it. No refresher runs for
//!   dead slots, no memory is held.
//! * **Active refresh.** While a slot is live, a single global sweeper
//!   recomputes its value every `REFRESH_INTERVAL` (1s) against the underlying
//!   ClickHouse repository. Request handlers always read from the cache — the
//!   background task eats the ClickHouse latency instead of the request.
//! * **First-hit fill with single-flight.** The first request for a new slot
//!   synchronously computes (so the client gets a real answer, not None).
//!   Concurrent first-hit callers for the same key serialize on a per-slot
//!   mutex so only one of them hits ClickHouse.
//!
//! Trade-off worth remembering: background load scales with *concurrent active
//! users*, not request rate. At ~1000 active users × 3 methods × 1Hz refresh
//! you're running ~3000 ClickHouse queries/sec in the background regardless
//! of whether clients are polling or idle. If that becomes the binding cost,
//! dial `REFRESH_INTERVAL` up (2-5s) — the SWR semantics still hold.

use std::hash::Hash;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::Semaphore;
use tracing::debug;

use crate::repository::repository::{
    BalancePoint, PnlPoint, PnlResolution, PortfolioMetrics, PortfolioRepository, Result,
    TradingError,
};

const IDLE_TTL_SECONDS: i64 = 60;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
/// Cap on concurrent background refreshes to avoid stampeding ClickHouse when
/// many slots tick at once. Tune against CH worker-thread capacity.
const REFRESH_CONCURRENCY: usize = 32;

/// One cache slot. Holds the most recently computed value (or `None` until
/// the first fill completes) plus the last-accessed timestamp used for idle
/// eviction. `fill_lock` serializes the first-fill path so a burst of
/// concurrent requests for a cold key only hits ClickHouse once.
struct Slot<V> {
    value: RwLock<Option<Arc<V>>>,
    fill_lock: AsyncMutex<()>,
    last_accessed: AtomicI64,
}

impl<V> Slot<V> {
    fn new(now: i64) -> Self {
        Self {
            value: RwLock::new(None),
            fill_lock: AsyncMutex::new(()),
            last_accessed: AtomicI64::new(now),
        }
    }

    fn touch(&self, now: i64) {
        self.last_accessed.store(now, Ordering::Relaxed);
    }

    fn peek(&self) -> Option<Arc<V>> {
        self.value.read().clone()
    }

    fn store(&self, v: V) {
        *self.value.write() = Some(Arc::new(v));
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Snapshot live entries of a DashMap and evict idle ones in-place. Returns
/// the live key/slot pairs (Arcs cloned so shards aren't held across awaits).
fn snapshot_and_evict<K, V>(
    map: &DashMap<K, Arc<Slot<V>>>,
    now: i64,
) -> Vec<(K, Arc<Slot<V>>)>
where
    K: Clone + Eq + Hash,
    V: Send + Sync,
{
    map.retain(|_, slot| now - slot.last_accessed.load(Ordering::Relaxed) <= IDLE_TTL_SECONDS);
    map.iter()
        .map(|r| (r.key().clone(), r.value().clone()))
        .collect()
}

/// Caching decorator over any `PortfolioRepository`. Hands out the same cached
/// value for a window while a background task keeps it fresh.
pub struct CachingPortfolioRepository {
    inner: Arc<dyn PortfolioRepository>,
    overview: Arc<DashMap<u64, Arc<Slot<PortfolioMetrics>>>>,
    portfolio: Arc<DashMap<(u64, PnlResolution), Arc<Slot<PortfolioMetrics>>>>,
    pnl_chart: Arc<DashMap<(u64, PnlResolution), Arc<Slot<Vec<PnlPoint>>>>>,
    balance_chart: Arc<DashMap<(u64, PnlResolution), Arc<Slot<Vec<BalancePoint>>>>>,
}

impl CachingPortfolioRepository {
    pub fn new(inner: Arc<dyn PortfolioRepository>) -> Arc<Self> {
        let this = Arc::new(Self {
            inner,
            overview: Arc::new(DashMap::new()),
            portfolio: Arc::new(DashMap::new()),
            pnl_chart: Arc::new(DashMap::new()),
            balance_chart: Arc::new(DashMap::new()),
        });
        Self::spawn_background_sweeper(this.clone());
        this
    }

    /// Spawn the single global sweeper that ticks every `REFRESH_INTERVAL`,
    /// refreshes live entries, and evicts entries not touched in
    /// `IDLE_TTL_SECONDS`. Runs for the lifetime of the process.
    fn spawn_background_sweeper(self_arc: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(REFRESH_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Skip the immediate first tick — nothing to refresh yet.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                self_arc.sweep_once(now_secs()).await;
            }
        });
    }

    async fn sweep_once(&self, now: i64) {
        // All four caches share one concurrency budget so a spike in one
        // method can't drain ClickHouse worker slots from the others.
        let sem = Arc::new(Semaphore::new(REFRESH_CONCURRENCY));

        let overview_targets = snapshot_and_evict(&self.overview, now);
        let portfolio_targets = snapshot_and_evict(&self.portfolio, now);
        let pnl_targets = snapshot_and_evict(&self.pnl_chart, now);
        let balance_targets = snapshot_and_evict(&self.balance_chart, now);

        let mut handles = Vec::new();

        for (user_id, slot) in overview_targets {
            let inner = self.inner.clone();
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await;
                match inner.get_portfolio_overview(user_id, now_secs()).await {
                    Ok(v) => slot.store(v),
                    Err(e) => debug!(
                        "portfolio-cache: overview refresh failed uid={} err={}",
                        user_id, e
                    ),
                }
            }));
        }

        for ((user_id, res), slot) in portfolio_targets {
            let inner = self.inner.clone();
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await;
                match inner.get_portfolio(user_id, now_secs(), res).await {
                    Ok(v) => slot.store(v),
                    Err(e) => debug!(
                        "portfolio-cache: portfolio refresh failed uid={} err={}",
                        user_id, e
                    ),
                }
            }));
        }

        for ((user_id, res), slot) in pnl_targets {
            let inner = self.inner.clone();
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await;
                match inner.get_pnl_chart(user_id, now_secs(), res).await {
                    Ok(v) => slot.store(v),
                    Err(e) => debug!(
                        "portfolio-cache: pnl-chart refresh failed uid={} err={}",
                        user_id, e
                    ),
                }
            }));
        }

        for ((user_id, res), slot) in balance_targets {
            let inner = self.inner.clone();
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await;
                match inner.get_balance_chart(user_id, now_secs(), res).await {
                    Ok(v) => slot.store(v),
                    Err(e) => debug!(
                        "portfolio-cache: balance-chart refresh failed uid={} err={}",
                        user_id, e
                    ),
                }
            }));
        }

        for h in handles {
            let _ = h.await;
        }
    }
}

/// Pop-or-fill the slot for `key` under a per-slot mutex. Concurrent
/// first-hit callers all see the same final `Arc<V>`, and the inner compute
/// runs at most once per cold slot.
async fn get_or_fill<K, V, F, Fut>(
    map: &DashMap<K, Arc<Slot<V>>>,
    key: K,
    compute: F,
) -> Result<Arc<V>>
where
    K: Eq + Hash + Clone,
    V: Send + Sync + 'static,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<V>>,
{
    let now = now_secs();
    let slot = map
        .entry(key)
        .or_insert_with(|| Arc::new(Slot::new(now)))
        .clone();
    slot.touch(now);

    if let Some(v) = slot.peek() {
        return Ok(v);
    }

    let _g = slot.fill_lock.lock().await;
    if let Some(v) = slot.peek() {
        return Ok(v);
    }

    let v = compute().await?;
    slot.store(v);
    slot.peek()
        .ok_or_else(|| TradingError::Database("cache slot empty after store".into()))
}

#[async_trait]
impl PortfolioRepository for CachingPortfolioRepository {
    async fn get_portfolio(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<PortfolioMetrics> {
        let inner = self.inner.clone();
        let arc = get_or_fill(&self.portfolio, (user_id, resolution), || async move {
            inner.get_portfolio(user_id, now_ts_secs, resolution).await
        })
        .await?;
        Ok((*arc).clone())
    }

    async fn get_portfolio_overview(
        &self,
        user_id: u64,
        now_ts_secs: i64,
    ) -> Result<PortfolioMetrics> {
        let inner = self.inner.clone();
        let arc = get_or_fill(&self.overview, user_id, || async move {
            inner.get_portfolio_overview(user_id, now_ts_secs).await
        })
        .await?;
        Ok((*arc).clone())
    }

    async fn get_pnl_chart(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<PnlPoint>> {
        let inner = self.inner.clone();
        let arc = get_or_fill(&self.pnl_chart, (user_id, resolution), || async move {
            inner.get_pnl_chart(user_id, now_ts_secs, resolution).await
        })
        .await?;
        Ok((*arc).clone())
    }

    async fn get_balance_chart(
        &self,
        user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>> {
        let inner = self.inner.clone();
        let arc = get_or_fill(&self.balance_chart, (user_id, resolution), || async move {
            inner.get_balance_chart(user_id, now_ts_secs, resolution).await
        })
        .await?;
        Ok((*arc).clone())
    }
}

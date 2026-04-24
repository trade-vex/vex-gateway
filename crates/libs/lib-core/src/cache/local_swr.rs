//! Small local stale-while-revalidate cache for user-facing read models.
//!
//! The hot path is a `DashMap` lookup plus an atomic `Arc` load. The only async
//! lock is the per-slot cold-fill lock, which prevents request stampedes for a
//! missing key. Refresh queries run outside any cache lock and atomically swap
//! the new value in when they finish.

use std::hash::Hash;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;
use dashmap::DashMap;
use tokio::sync::Mutex as AsyncMutex;

use crate::repository::repository::{Result, TradingError};

#[derive(Debug, Clone, Copy)]
pub struct LocalSwrConfig {
    pub idle_ttl: Duration,
    pub active_refresh: Duration,
    pub hot_refresh: Duration,
    pub hot_duration: Duration,
}

impl LocalSwrConfig {
    pub const fn trading_ui() -> Self {
        Self {
            idle_ttl: Duration::from_secs(60),
            active_refresh: Duration::from_millis(500),
            hot_refresh: Duration::from_millis(100),
            hot_duration: Duration::from_secs(2),
        }
    }
}

pub struct LocalSwrSlot<V> {
    value: ArcSwapOption<V>,
    fill_lock: AsyncMutex<()>,
    last_accessed_ms: AtomicI64,
    hot_until_ms: AtomicI64,
    next_refresh_ms: AtomicI64,
}

impl<V> LocalSwrSlot<V> {
    fn new(now_ms: i64) -> Self {
        Self {
            value: ArcSwapOption::from(None),
            fill_lock: AsyncMutex::new(()),
            last_accessed_ms: AtomicI64::new(now_ms),
            hot_until_ms: AtomicI64::new(0),
            next_refresh_ms: AtomicI64::new(now_ms),
        }
    }

    pub fn load(&self) -> Option<Arc<V>> {
        self.value.load_full()
    }

    pub fn store(&self, value: V) {
        self.value.store(Some(Arc::new(value)));
    }

    fn touch(&self, now_ms: i64) {
        self.last_accessed_ms.store(now_ms, Ordering::Relaxed);
    }

    fn is_idle(&self, now_ms: i64, idle_ttl_ms: i64) -> bool {
        now_ms - self.last_accessed_ms.load(Ordering::Relaxed) > idle_ttl_ms
    }

    fn boost(&self, now_ms: i64, hot_duration_ms: i64) {
        self.hot_until_ms
            .store(now_ms + hot_duration_ms, Ordering::Relaxed);
        self.next_refresh_ms.store(now_ms, Ordering::Relaxed);
    }

    fn due_for_refresh(&self, now_ms: i64, config: LocalSwrConfig) -> bool {
        let next = self.next_refresh_ms.load(Ordering::Relaxed);
        if now_ms < next {
            return false;
        }

        let interval = if now_ms < self.hot_until_ms.load(Ordering::Relaxed) {
            config.hot_refresh
        } else {
            config.active_refresh
        };
        self.next_refresh_ms
            .store(now_ms + duration_millis(interval), Ordering::Relaxed);
        true
    }
}

#[derive(Clone)]
pub struct LocalSwrCache<K, V> {
    config: LocalSwrConfig,
    slots: Arc<DashMap<K, Arc<LocalSwrSlot<V>>>>,
}

impl<K, V> LocalSwrCache<K, V>
where
    K: Clone + Eq + Hash,
    V: Send + Sync + 'static,
{
    pub fn new(config: LocalSwrConfig) -> Self {
        Self {
            config,
            slots: Arc::new(DashMap::new()),
        }
    }

    pub async fn get_or_fill<F, Fut>(&self, key: K, compute: F) -> Result<Arc<V>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<V>>,
    {
        let now_ms = now_millis();
        let slot = self
            .slots
            .entry(key)
            .or_insert_with(|| Arc::new(LocalSwrSlot::new(now_ms)))
            .clone();
        slot.touch(now_ms);

        if let Some(value) = slot.load() {
            return Ok(value);
        }

        let _guard = slot.fill_lock.lock().await;
        if let Some(value) = slot.load() {
            return Ok(value);
        }

        let fresh = compute().await?;
        slot.store(fresh);
        slot.load()
            .ok_or_else(|| TradingError::Database("local SWR slot empty after store".into()))
    }

    pub fn due_slots(&self) -> Vec<(K, Arc<LocalSwrSlot<V>>)> {
        let now_ms = now_millis();
        let idle_ttl_ms = duration_millis(self.config.idle_ttl);
        self.slots
            .retain(|_, slot| !slot.is_idle(now_ms, idle_ttl_ms));

        self.slots
            .iter()
            .filter_map(|entry| {
                let slot = entry.value();
                if slot.load().is_some() && slot.due_for_refresh(now_ms, self.config) {
                    Some((entry.key().clone(), slot.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn boost_where<P>(&self, predicate: P)
    where
        P: Fn(&K) -> bool,
    {
        let now_ms = now_millis();
        let hot_duration_ms = duration_millis(self.config.hot_duration);
        for entry in self.slots.iter() {
            if predicate(entry.key()) {
                entry.value().boost(now_ms, hot_duration_ms);
            }
        }
    }

    pub fn invalidate_where<P>(&self, predicate: P)
    where
        P: Fn(&K) -> bool,
    {
        self.slots.retain(|key, _| !predicate(key));
    }
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn duration_millis(duration: Duration) -> i64 {
    duration.as_millis() as i64
}

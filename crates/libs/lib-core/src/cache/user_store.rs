use crate::cache::local_swr::{LocalSwrCache, LocalSwrConfig};
use crate::cache::scaling_cache::ScalingCache;
use crate::model::user_balances::Model as UserBalanceModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::BasicKvStore;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

const BALANCE_REFRESH_CONCURRENCY: usize = 64;

#[derive(Clone)]
pub struct UserBalanceStore {
    cache: Option<RedisDb>,
    trading_repo: Option<Arc<dyn TradingRepository>>,
    config: UserBalanceCacheConfig,
    scaling_cache: Option<Arc<ScalingCache>>,
    local_cache: LocalSwrCache<u64, Vec<UserBalanceModel>>,
}

#[derive(Clone, Default)]
pub struct UserBalanceCacheConfig {
    pub enabled: bool,
}

impl UserBalanceStore {
    /// Create new store
    pub fn new(config: UserBalanceCacheConfig) -> Self {
        Self {
            cache: None,
            trading_repo: None,
            config,
            scaling_cache: None,
            local_cache: LocalSwrCache::new(LocalSwrConfig::trading_ui()),
        }
    }

    pub fn with_repository(
        config: UserBalanceCacheConfig,
        trading_repo: Arc<dyn TradingRepository>,
    ) -> Self {
        let store = Self {
            cache: None,
            trading_repo: Some(trading_repo),
            config,
            scaling_cache: None,
            local_cache: LocalSwrCache::new(LocalSwrConfig::trading_ui()),
        };
        store.spawn_local_refresh_loop();
        store
    }

    pub async fn with_redis_cache(
        config: UserBalanceCacheConfig,
        redis_url: String,
        trading_repo: Option<Arc<dyn TradingRepository>>,
    ) -> Result<Self, TradingError> {
        let cache = if config.enabled {
            let redis_db = RedisDb::connect(&redis_url).await.map_err(|e| {
                TradingError::Database(format!("Failed to connect to Redis: {}", e))
            })?;
            Some(redis_db)
        } else {
            None
        };

        let store = Self {
            cache,
            trading_repo,
            config,
            scaling_cache: None,
            local_cache: LocalSwrCache::new(LocalSwrConfig::trading_ui()),
        };
        store.spawn_local_refresh_loop();
        Ok(store)
    }

    pub fn with_scaling_cache(mut self, scaling_cache: Arc<ScalingCache>) -> Self {
        self.scaling_cache = Some(scaling_cache);
        self
    }

    /// Get user balances from cache: user:{user_id}:asset:{asset_id}:balance hashes
    pub async fn get_user_balances(
        &self,
        user_id: u64,
    ) -> Result<Vec<UserBalanceModel>, TradingError> {
        let this = self.clone();
        let balances = self
            .local_cache
            .get_or_fill(user_id, || async move {
                this.fetch_user_balances_source(user_id).await
            })
            .await?;
        Ok((*balances).clone())
    }

    async fn fetch_user_balances_source(
        &self,
        user_id: u64,
    ) -> Result<Vec<UserBalanceModel>, TradingError> {
        // Cache: Scan for user:{user_id}:asset:*:balance keys, then fetch each hash
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let pattern = format!("user:{}:asset:*:balance", user_id);

                // Use SCAN to find all balance keys for this user
                match cache.scan_keys(&pattern).await {
                    Ok(keys) if !keys.is_empty() => {
                        debug!(
                            "User balances cache hit for user: {} ({} assets)",
                            user_id,
                            keys.len()
                        );

                        let mut balances = Vec::new();
                        // Fetch each balance hash
                        for key in keys {
                            match cache.hgetall_string(&key).await {
                                Ok(hash) if !hash.is_empty() => {
                                    if let Some(balance) = Self::parse_balance_hash(hash) {
                                        balances.push(balance);
                                    }
                                }
                                _ => continue,
                            }
                        }

                        if !balances.is_empty() {
                            return Ok(balances);
                        }
                    }
                    Ok(_) => {
                        debug!("User balances cache miss for user: {}", user_id);
                    }
                    Err(e) => {
                        warn!("User balances cache error for user {}: {}", user_id, e);
                    }
                }
            }
        }

        // Database fallback
        if let Some(repo) = &self.trading_repo {
            match repo.get_user_balances(user_id).await {
                Ok(balances) => {
                    debug!("Database hit for user balances: {}", user_id);
                    return Ok(balances);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        debug!("User balances not found: {}", user_id);
        Ok(vec![])
    }

    pub async fn invalidate_user_balances(&self, user_id: u64) {
        self.local_cache.invalidate_where(|key| *key == user_id);

        if !self.config.enabled {
            return;
        }

        if let Some(cache) = &self.cache {
            let pattern = format!("user:{}:asset:*:balance", user_id);

            // Scan for all balance keys
            match cache.scan_keys(&pattern).await {
                Ok(keys) => {
                    // Delete each balance hash
                    for key in &keys {
                        if let Err(e) = cache.del_key(key).await {
                            warn!("Failed to delete balance hash {}: {}", key, e);
                        }
                    }
                    debug!(
                        "Invalidated {} balance(s) for user: {}",
                        keys.len(),
                        user_id
                    );
                }
                Err(e) => {
                    warn!("Failed to scan balance keys for user {}: {}", user_id, e);
                }
            }
        }
    }

    pub fn boost_user_activity(&self, user_id: u64) {
        self.local_cache.boost_where(|key| *key == user_id);
    }

    pub async fn warm_user_balances(&self, user_id: u64) {
        let _ = self.get_user_balances(user_id).await;
        self.boost_user_activity(user_id);
    }

    /// Convert UserBalanceModel to UserBalanceResponse with scaling applied
    pub fn to_response(&self, model: UserBalanceModel) -> crate::dto::trading::UserBalanceResponse {
        use crate::dto::trading::UserBalanceResponse;
        use rust_decimal::Decimal;

        let asset_id = model.asset_id as u16;

        // Apply scaling if scaling cache is available
        let (available, locked, total) = if let Some(scaling_cache) = &self.scaling_cache {
            let available_decimal =
                scaling_cache.scale_balance(asset_id, Decimal::from(model.available));
            let locked_decimal = scaling_cache.scale_balance(asset_id, Decimal::from(model.locked));
            let total_decimal = scaling_cache.scale_balance(asset_id, Decimal::from(model.total));
            (available_decimal, locked_decimal, total_decimal)
        } else {
            (
                Decimal::from(model.available),
                Decimal::from(model.locked),
                Decimal::from(model.total),
            )
        };

        UserBalanceResponse {
            user_id: model.user_id,
            asset_id: model.asset_id as u32,
            available,
            locked,
            total,
            timestamp: model.timestamp as u64,
        }
    }

    // Helper: Parse balance hash from Redis into UserBalanceModel
    // Kafka writes: user_id, asset_id, available, locked, total, timestamp
    fn parse_balance_hash(hash: HashMap<String, String>) -> Option<UserBalanceModel> {
        let user_id = hash.get("user_id")?.parse::<i64>().ok()?;
        let asset_id = hash.get("asset_id")?.parse::<i32>().ok()?;
        let available = hash.get("available")?.parse::<i64>().ok()?;
        let locked = hash.get("locked")?.parse::<i64>().ok()?;
        let total = hash.get("total")?.parse::<i64>().ok()?;
        let timestamp = hash.get("timestamp")?.parse::<i64>().ok()?;

        Some(UserBalanceModel {
            id: 0, // Auto-generated by DB
            user_id,
            asset_id,
            available,
            locked,
            total,
            timestamp,
            created_at: None,
        })
    }

    fn spawn_local_refresh_loop(&self) {
        if self.trading_repo.is_none() && self.cache.is_none() {
            return;
        }

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };

        let this = self.clone();
        handle.spawn(async move {
            let mut ticker = tokio::time::interval(LocalSwrConfig::trading_ui().hot_refresh);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;

            loop {
                ticker.tick().await;
                let due = this.local_cache.due_slots();
                if due.is_empty() {
                    continue;
                }

                let sem = Arc::new(Semaphore::new(BALANCE_REFRESH_CONCURRENCY));
                let mut handles = Vec::with_capacity(due.len());
                for (user_id, slot) in due {
                    let this = this.clone();
                    let sem = sem.clone();
                    handles.push(tokio::spawn(async move {
                        let Ok(_permit) = sem.acquire_owned().await else {
                            return;
                        };
                        match this.fetch_user_balances_source(user_id).await {
                            Ok(fresh) => slot.store(fresh),
                            Err(e) => debug!(
                                user_id = user_id,
                                error = %e,
                                "balance local SWR refresh failed; serving previous snapshot"
                            ),
                        }
                    }));
                }

                for handle in handles {
                    let _ = handle.await;
                }
            }
        });
    }
}

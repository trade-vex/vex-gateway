use crate::model::user_balances::Model as UserBalanceModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::BasicKvStore;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, warn};

#[derive(Clone)]
pub struct UserBalanceStore {
    cache: Option<RedisDb>,
    trading_repo: Option<Arc<dyn TradingRepository>>,
    config: UserBalanceCacheConfig,
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
        }
    }

    pub fn with_repository(
        config: UserBalanceCacheConfig,
        trading_repo: Arc<dyn TradingRepository>,
    ) -> Self {
        Self {
            cache: None,
            trading_repo: Some(trading_repo),
            config,
        }
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

        Ok(Self {
            cache,
            trading_repo,
            config,
        })
    }

    /// Get user balances from cache: user:{user_id}:asset:{asset_id}:balance hashes
    pub async fn get_user_balances(
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

                        // Convert to &str slice for batch fetch (solves N+1 query problem)
                        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();

                        // Batch fetch all balance hashes in single pipelined request
                        let mut balances = Vec::new();
                        if let Ok(hashes) = cache.hgetall_batch(&key_refs).await {
                            for hash in hashes.into_iter().flatten() {
                                if let Some(balance) = Self::parse_balance_hash(hash) {
                                    balances.push(balance);
                                }
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

    // Helper: Parse balance hash from Redis into UserBalanceModel
    // Kafka writes: user_id, asset_id, available, locked, total, timestamp
    // Parse all values as Decimal to handle full u64 range without overflow
    fn parse_balance_hash(hash: HashMap<String, String>) -> Option<UserBalanceModel> {
        let user_id = Decimal::from_str(hash.get("user_id")?).ok()?;
        let asset_id = hash.get("asset_id")?.parse::<i32>().ok()?;
        let available = Decimal::from_str(hash.get("available")?).ok()?;
        let locked = Decimal::from_str(hash.get("locked")?).ok()?;
        let total = Decimal::from_str(hash.get("total")?).ok()?;
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
}

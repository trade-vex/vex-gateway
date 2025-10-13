use crate::model::user_balances::Model as UserBalanceModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::BasicKvStore;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

#[derive(Clone)]
pub struct UserBalanceStore {
    cache: Option<RedisDb>,
    trading_repo: Option<Arc<dyn TradingRepository>>,
    config: UserBalanceCacheConfig,
}

#[derive(Clone)]
pub struct UserBalanceCacheConfig {
    pub ttl: Duration,
    pub enabled: bool,
}

impl Default for UserBalanceCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(1800),
            enabled: false,
        }
    }
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
        user_id: Uuid,
    ) -> Result<Vec<UserBalanceModel>, TradingError> {
        // Convert UUID to i64 to match Kafka consumer's key format
        // Kafka writes: user:{numeric_id}:asset:{asset_id}:balance
        let user_id_i64 = user_id.as_u128() as i64;
        
        // Cache: Scan for user:{user_id}:asset:*:balance keys, then fetch each hash
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                let pattern = format!("user:{}:asset:*:balance", user_id_i64);
                
                // Use SCAN to find all balance keys for this user
                match cache.scan_keys(&pattern).await {
                    Ok(keys) if !keys.is_empty() => {
                        debug!("User balances cache hit for user: {} (i64: {}, {} assets)", user_id, user_id_i64, keys.len());
                        
                        let mut balances = Vec::new();
                        // Fetch each balance hash
                        for key in keys {
                            match cache.hgetall_string(&key).await {
                                Ok(hash) if !hash.is_empty() => {
                                    if let Some(balance) = Self::parse_balance_hash(hash, user_id) {
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

    pub async fn invalidate_user_balances(&self, user_id: Uuid) {
        if !self.config.enabled {
            return;
        }

        if let Some(cache) = &self.cache {
            // Convert UUID to i64 to match Kafka consumer's key format
            let user_id_i64 = user_id.as_u128() as i64;
            let pattern = format!("user:{}:asset:*:balance", user_id_i64);
            
            // Scan for all balance keys
            match cache.scan_keys(&pattern).await {
                Ok(keys) => {
                    // Delete each balance hash
                    for key in &keys {
                        if let Err(e) = cache.del_key(key).await {
                            warn!("Failed to delete balance hash {}: {}", key, e);
                        }
                    }
                    debug!("Invalidated {} balance(s) for user: {}", keys.len(), user_id);
                }
                Err(e) => {
                    warn!("Failed to scan balance keys for user {}: {}", user_id, e);
                }
            }
        }
    }
    
    // Helper: Parse balance hash from Redis into UserBalanceModel
    // Kafka writes: user_id, asset_id, available, locked, total, timestamp
    fn parse_balance_hash(hash: HashMap<String, String>, _user_id: Uuid) -> Option<UserBalanceModel> {
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
}

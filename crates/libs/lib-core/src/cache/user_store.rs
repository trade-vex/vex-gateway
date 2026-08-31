use crate::cache::scaling_cache::ScalingCache;
use crate::model::user_balances::Model as UserBalanceModel;
use crate::repository::repository::{TradingError, TradingRepository};
use lib_db::backends::kv::redis::RedisDb;
use lib_db::traits::BasicKvStore;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

#[derive(Debug, Clone, PartialEq)]
pub struct UserBalance {
    pub id: i64,
    pub user_id: i64,
    pub asset_id: i32,
    pub available: u64,
    pub locked: u64,
    pub total: u64,
    pub timestamp: i64,
    pub created_at: Option<chrono::NaiveDateTime>,
}

impl From<UserBalanceModel> for UserBalance {
    fn from(model: UserBalanceModel) -> Self {
        Self {
            id: model.id,
            user_id: model.user_id,
            asset_id: model.asset_id,
            available: model.available as u64,
            locked: model.locked as u64,
            total: model.total as u64,
            timestamp: model.timestamp,
            created_at: model.created_at,
        }
    }
}

#[derive(Clone)]
pub struct UserBalanceStore {
    cache: Option<RedisDb>,
    trading_repo: Option<Arc<dyn TradingRepository>>,
    config: UserBalanceCacheConfig,
    scaling_cache: Option<Arc<ScalingCache>>,
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
            scaling_cache: None,
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
            scaling_cache: None,
        })
    }

    pub fn with_scaling_cache(mut self, scaling_cache: Arc<ScalingCache>) -> Self {
        self.scaling_cache = Some(scaling_cache);
        self
    }

    /// Get user balances from cache: user:{user_id}:asset:{asset_id}:balance hashes
    ///
    /// Read-only. The Events Handler owns the per-user asset index `user:{user_id}:balances` and
    /// the balance hashes; the gateway never writes either. The cache answers only when the index
    /// is non-empty and every indexed hash parses. Otherwise (nothing cached for the user, an
    /// indexed hash missing, or a Redis error) the repository answers alone, so an inconsistent
    /// cache never yields a silently incomplete balance set.
    pub async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalance>, TradingError> {
        if self.config.enabled {
            if let Some(cache) = &self.cache {
                if let Some(balances) = Self::cached_balances(cache, user_id).await {
                    debug!(
                        "User balances cache hit for user: {} ({} assets)",
                        user_id,
                        balances.len()
                    );
                    return Ok(balances);
                }
            }
        }

        // Database fallback
        if let Some(repo) = &self.trading_repo {
            let balances = repo.get_user_balances(user_id).await?;
            debug!("Database hit for user balances: {}", user_id);
            return Ok(balances.into_iter().map(Into::into).collect());
        }

        debug!("User balances not found: {}", user_id);
        Ok(vec![])
    }

    /// The user's complete cached balance set: SMEMBERS of the asset index, then every indexed
    /// hash in one pipelined round trip. `None` when the cache cannot answer on its own.
    async fn cached_balances(cache: &RedisDb, user_id: u64) -> Option<Vec<UserBalance>> {
        let asset_set_key = format!("user:{}:balances", user_id);
        let asset_ids = match cache.smembers_string(&asset_set_key).await {
            Ok(asset_ids) if !asset_ids.is_empty() => asset_ids,
            Ok(_) => {
                debug!("User balances cache miss for user: {}", user_id);
                return None;
            }
            Err(error) => {
                warn!("User balances cache error for user {}: {}", user_id, error);
                return None;
            }
        };

        let balance_keys: Vec<String> = asset_ids
            .iter()
            .map(|asset_id| format!("user:{}:asset:{}:balance", user_id, asset_id))
            .collect();
        let hashes = match cache.hgetall_strings(&balance_keys).await {
            Ok(hashes) => hashes,
            Err(error) => {
                warn!(
                    "Failed to get balance hashes for user {}: {}",
                    user_id, error
                );
                return None;
            }
        };

        let mut balances = Vec::with_capacity(hashes.len());
        for (balance_key, hash) in balance_keys.iter().zip(hashes) {
            match Self::parse_balance_hash(hash) {
                Some(balance) => balances.push(balance),
                None => {
                    warn!(
                        "Balance hash {} is missing or malformed although {} lists it; \
                         reading balances from the database",
                        balance_key, asset_set_key
                    );
                    return None;
                }
            }
        }
        Some(balances)
    }

    /// Convert UserBalance to UserBalanceResponse with scaling applied
    pub fn to_response(&self, model: UserBalance) -> crate::dto::trading::UserBalanceResponse {
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

    // Helper: Parse balance hash from Redis into UserBalance
    // Kafka writes: user_id, asset_id, available, locked, total, timestamp
    fn parse_balance_hash(hash: HashMap<String, String>) -> Option<UserBalance> {
        let user_id = hash.get("user_id")?.parse::<i64>().ok()?;
        let asset_id = hash.get("asset_id")?.parse::<i32>().ok()?;
        let available = hash.get("available")?.parse::<u64>().ok()?;
        let locked = hash.get("locked")?.parse::<u64>().ok()?;
        let total = hash.get("total")?.parse::<u64>().ok()?;
        let timestamp = hash.get("timestamp")?.parse::<i64>().ok()?;

        Some(UserBalance {
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

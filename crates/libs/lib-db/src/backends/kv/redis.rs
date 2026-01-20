use crate::{
    error::{DbError, Result},
    traits::{AdvancedKvStore, AtomicKvStore, BasicKvStore, ExpiringKvStore, PoolStats},
};
use async_trait::async_trait;
use bb8::{Pool, RunError};
use redis::{AsyncCommands, Client, ErrorKind, RedisError, aio::ConnectionManager};
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;
use std::time::Duration;

/// Redis backend
#[derive(Clone)]
pub struct RedisDb {
    pool: Pool<RedisConnectionManager>,
    config: Arc<RedisConfig>,
}

/// Redis configuration
#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub connection_timeout_ms: u64,
    pub max_lifetime_secs: u64,
    pub idle_timeout_secs: u64,
    pub retry_count: u32,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            max_connections: 50,
            min_connections: 10,
            connection_timeout_ms: 500,
            max_lifetime_secs: 300,
            idle_timeout_secs: 60,
            retry_count: 3,
        }
    }
}

/// Custom connection manager for bb8 pool
#[derive(Clone)]
struct RedisConnectionManager {
    client: Client,
}

#[async_trait]
impl bb8::ManageConnection for RedisConnectionManager {
    type Connection = ConnectionManager;
    type Error = RedisError;

    async fn connect(&self) -> std::result::Result<Self::Connection, Self::Error> {
        ConnectionManager::new(self.client.clone()).await
    }

    async fn is_valid(&self, conn: &mut Self::Connection) -> std::result::Result<(), Self::Error> {
        redis::cmd("PING").query_async(conn).await
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

impl RedisDb {
    /// Create with custom configuration
    pub fn with_config(config: RedisConfig) -> Self {
        // Create a dummy client - will be replaced in connect()
        let dummy_client = Client::open("redis://127.0.0.1:6379")
            .unwrap_or_else(|_| panic!("Failed to create dummy Redis client"));

        Self {
            pool: Pool::builder().build_unchecked(RedisConnectionManager {
                client: dummy_client,
            }),
            config: Arc::new(config),
        }
    }

    /// Get a connection from the pool
    async fn get_conn(&self) -> Result<bb8::PooledConnection<'_, RedisConnectionManager>> {
        self.pool
            .get()
            .await
            .map_err(|e: RunError<RedisError>| match e {
                RunError::TimedOut => DbError::Timeout {
                    duration_ms: self.config.connection_timeout_ms,
                },
                RunError::User(e) => e.into(),
            })
    }

    /// Execute with retry logic
    async fn execute_with_retry<F, T>(&self, f: F) -> Result<T>
    where
        F: Fn(
            &mut ConnectionManager,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::result::Result<T, RedisError>> + Send + '_>,
        >,
    {
        let mut last_error = None;
        let attempts = std::cmp::max(1, self.config.retry_count);

        for _ in 0..attempts {
            let mut conn = self.get_conn().await?;

            match f(&mut conn).await {
                Ok(result) => return Ok(result),
                Err(e) if Self::is_retryable(&e) => {
                    last_error = Some(e);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }

        match last_error {
            Some(e) => Err(e.into()),
            None => Err(DbError::Connection {
                message: "No attempts were made due to invalid retry configuration".to_string(),
                source: None,
            }),
        }
    }

    /// Check if error is retryable
    fn is_retryable(error: &RedisError) -> bool {
        matches!(
            error.kind(),
            ErrorKind::IoError | ErrorKind::BusyLoadingError | ErrorKind::TryAgain
        )
    }
}

// Basic Kv Store Implementation

#[async_trait]
impl BasicKvStore for RedisDb {
    async fn connect(url: &str) -> Result<Self> {
        let config = RedisConfig::default();

        let client = Client::open(url).map_err(|e| DbError::Connection {
            message: format!("Failed to connect to Redis: {}", e),
            source: Some(Box::new(e)),
        })?;

        // Test connection
        let test_conn = ConnectionManager::new(client.clone()).await?;
        drop(test_conn);

        // Create connection pool
        let manager = RedisConnectionManager {
            client: client.clone(),
        };

        let pool = Pool::builder()
            .max_size(config.max_connections)
            .min_idle(Some(config.min_connections))
            .connection_timeout(Duration::from_millis(config.connection_timeout_ms))
            .idle_timeout(Some(Duration::from_secs(config.idle_timeout_secs)))
            .max_lifetime(Some(Duration::from_secs(config.max_lifetime_secs)))
            .build(manager)
            .await
            .map_err(|e| DbError::Connection {
                message: format!("Failed to create connection pool: {}", e),
                source: Some(Box::new(e)),
            })?;

        Ok(Self {
            pool,
            config: Arc::new(config),
        })
    }

    async fn set<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + Send + Sync,
    {
        let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
            message: format!("Failed to serialize value: {}", e),
            source: Some(Box::new(e)),
        })?;

        self.execute_with_retry(|conn| {
            let key = key.to_string();
            let bytes = bytes.clone();
            Box::pin(async move { conn.set(key, bytes).await })
        })
        .await
    }

    async fn get<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let bytes: Option<Vec<u8>> = self
            .execute_with_retry(|conn| {
                let key = key.to_string();
                Box::pin(async move { conn.get(key).await })
            })
            .await?;

        match bytes {
            Some(data) => {
                let value = bincode::deserialize(&data).map_err(|e| DbError::Serialization {
                    message: format!("Failed to deserialize value: {}", e),
                    source: Some(Box::new(e)),
                })?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    async fn set_bytes(&self, key: &str, value: &[u8]) -> Result<()> {
        self.execute_with_retry(|conn| {
            let key = key.to_string();
            let value = value.to_vec();
            Box::pin(async move { conn.set(key, value).await })
        })
        .await
    }

    async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.execute_with_retry(|conn| {
            let key = key.to_string();
            Box::pin(async move { conn.get(key).await })
        })
        .await
    }

    async fn delete(&self, key: &str) -> Result<bool> {
        let deleted: i32 = self
            .execute_with_retry(|conn| {
                let key = key.to_string();
                Box::pin(async move { conn.del(key).await })
            })
            .await?;
        Ok(deleted > 0)
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        self.execute_with_retry(|conn| {
            let key = key.to_string();
            Box::pin(async move { conn.exists(key).await })
        })
        .await
    }
}

// ADVANCED KV STORE IMPLEMENTATION

#[async_trait]
impl AdvancedKvStore for RedisDb {
    async fn ping(&self) -> Result<()> {
        self.execute_with_retry(|conn| {
            Box::pin(async move { redis::cmd("PING").query_async(conn).await })
        })
        .await
    }

    fn pool_stats(&self) -> PoolStats {
        let state = self.pool.state();
        PoolStats {
            size: state.connections,
            idle: state.idle_connections,
            active: state.connections - state.idle_connections,
            max_size: self.config.max_connections,
        }
    }

    async fn mget<T>(&self, keys: &[&str]) -> Result<Vec<Option<T>>>
    where
        T: DeserializeOwned,
    {
        let values: Vec<Option<Vec<u8>>> = self
            .execute_with_retry(|conn| {
                let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
                Box::pin(async move { conn.get(keys).await })
            })
            .await?;

        let mut results = Vec::with_capacity(values.len());
        for value_opt in values {
            match value_opt {
                Some(bytes) => {
                    let value =
                        bincode::deserialize(&bytes).map_err(|e| DbError::Serialization {
                            message: format!("Failed to deserialize value: {}", e),
                            source: Some(Box::new(e)),
                        })?;
                    results.push(Some(value));
                }
                None => results.push(None),
            }
        }

        Ok(results)
    }

    async fn mset<T>(&self, pairs: &[(&str, &T)]) -> Result<()>
    where
        T: Serialize + Send + Sync,
    {
        let mut items = Vec::with_capacity(pairs.len() * 2);

        for (key, value) in pairs {
            let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
                message: format!("Failed to serialize value: {}", e),
                source: Some(Box::new(e)),
            })?;
            items.push((*key, bytes));
        }

        // Convert to owned data to avoid lifetime issues
        let items_owned: Vec<(String, Vec<u8>)> =
            items.into_iter().map(|(k, v)| (k.to_string(), v)).collect();

        let mut last_error = None;
        let attempts = std::cmp::max(1, self.config.retry_count);

        for _ in 0..attempts {
            let mut conn = self.get_conn().await?;

            let result = {
                let mut pipe = redis::pipe();
                pipe.atomic();
                for (key, value) in &items_owned {
                    pipe.set(key, value);
                }
                pipe.query_async(&mut *conn).await
            };

            match result {
                Ok(()) => return Ok(()),
                Err(e) if Self::is_retryable(&e) => {
                    last_error = Some(e);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }

        match last_error {
            Some(e) => Err(e.into()),
            None => Err(DbError::Connection {
                message: "No attempts were made due to invalid retry configuration".to_string(),
                source: None,
            }),
        }
    }

    async fn mdel(&self, keys: &[&str]) -> Result<u64> {
        let deleted: i32 = self
            .execute_with_retry(|conn| {
                let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
                Box::pin(async move { conn.del(keys).await })
            })
            .await?;
        Ok(deleted as u64)
    }
}

// Atomic Kv Store Implementation

#[async_trait]
impl AtomicKvStore for RedisDb {
    async fn incr(&self, key: &str, delta: i64) -> Result<i64> {
        self.execute_with_retry(|conn| {
            let key = key.to_string();
            Box::pin(async move { conn.incr(key, delta).await })
        })
        .await
    }

    async fn set_nx<T>(&self, key: &str, value: &T) -> Result<bool>
    where
        T: Serialize + Send + Sync,
    {
        let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
            message: format!("Failed to serialize value: {}", e),
            source: Some(Box::new(e)),
        })?;

        self.execute_with_retry(|conn| {
            let key = key.to_string();
            let bytes = bytes.clone();
            Box::pin(async move { conn.set_nx(key, bytes).await })
        })
        .await
    }

    async fn cas<T>(&self, key: &str, old: &T, new: &T) -> Result<bool>
    where
        T: Serialize + DeserializeOwned + Send + Sync + PartialEq,
    {
        // Redis doesn't have native CAS, so we implement with Lua script
        let script = r#"
            local current = redis.call('GET', KEYS[1])
            if current == ARGV[1] then
                redis.call('SET', KEYS[1], ARGV[2])
                return 1
            else
                return 0
            end
        "#;

        let old_bytes = bincode::serialize(old).map_err(|e| DbError::Serialization {
            message: format!("Failed to serialize old value: {}", e),
            source: Some(Box::new(e)),
        })?;

        let new_bytes = bincode::serialize(new).map_err(|e| DbError::Serialization {
            message: format!("Failed to serialize new value: {}", e),
            source: Some(Box::new(e)),
        })?;

        let result: i32 = self
            .execute_with_retry(|conn| {
                let key = key.to_string();
                let old_bytes = old_bytes.clone();
                let new_bytes = new_bytes.clone();
                Box::pin(async move {
                    redis::Script::new(script)
                        .key(key)
                        .arg(old_bytes)
                        .arg(new_bytes)
                        .invoke_async(conn)
                        .await
                })
            })
            .await?;

        Ok(result == 1)
    }
}

// Expiring Kv Store Implementation

#[async_trait]
impl ExpiringKvStore for RedisDb {
    async fn set_ex<T>(&self, key: &str, value: &T, ttl_secs: u64) -> Result<()>
    where
        T: Serialize + Send + Sync,
    {
        let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
            message: format!("Failed to serialize value: {}", e),
            source: Some(Box::new(e)),
        })?;

        let key = key.to_string();
        self.execute_with_retry(move |conn| {
            let key = key.clone();
            let bytes = bytes.clone();
            Box::pin(async move { conn.set_ex(key, bytes, ttl_secs).await })
        })
        .await
    }

    async fn expire(&self, key: &str, ttl_secs: u64) -> Result<bool> {
        self.execute_with_retry(|conn| {
            let key = key.to_string();
            let ttl = ttl_secs as i64;
            Box::pin(async move { conn.expire(key, ttl).await })
        })
        .await
    }

    async fn ttl(&self, key: &str) -> Result<Option<u64>> {
        let ttl: i64 = self
            .execute_with_retry(|conn| {
                let key = key.to_string();
                Box::pin(async move { conn.ttl(key).await })
            })
            .await?;

        match ttl {
            -2 => Ok(None),           // Key doesn't exist
            -1 => Ok(Some(u64::MAX)), // No TTL set
            n if n >= 0 => Ok(Some(n as u64)),
            _ => Ok(None),
        }
    }
}

// Raw String Operations (for Kafka-written data)
// These methods work with plain strings instead of bincode serialization
impl RedisDb {
    /// Get all fields and values from a hash (HGETALL)
    /// Returns HashMap<String, String> for plain string data
    pub async fn hgetall_string(
        &self,
        key: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        self.execute_with_retry(|conn| {
            let key = key.to_string();
            Box::pin(async move { conn.hgetall(key).await })
        })
        .await
    }

    /// Get all members of a set (SMEMBERS)
    /// Returns Vec<String> for plain string data
    pub async fn smembers_string(&self, key: &str) -> Result<Vec<String>> {
        self.execute_with_retry(|conn| {
            let key = key.to_string();
            Box::pin(async move { conn.smembers(key).await })
        })
        .await
    }

    /// Delete a key (DEL)
    pub async fn del_key(&self, key: &str) -> Result<()> {
        let _: () = self
            .execute_with_retry(|conn| {
                let key = key.to_string();
                Box::pin(async move { conn.del(key).await })
            })
            .await?;
        Ok(())
    }

    /// Remove member from set (SREM)
    pub async fn srem_string(&self, key: &str, member: &str) -> Result<()> {
        let _: () = self
            .execute_with_retry(|conn| {
                let key = key.to_string();
                let member = member.to_string();
                Box::pin(async move { conn.srem(key, member).await })
            })
            .await?;
        Ok(())
    }

    /// Scan for keys matching pattern (SCAN)
    pub async fn scan_keys(&self, pattern: &str) -> Result<Vec<String>> {
        let mut conn = self.get_conn().await?;
        let mut cursor = 0u64;
        let mut keys = Vec::new();

        loop {
            let (new_cursor, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut *conn)
                .await?;

            keys.extend(batch);
            cursor = new_cursor;

            if cursor == 0 {
                break;
            }
        }

        Ok(keys)
    }
}

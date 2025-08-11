use crate::error::Result;
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use std::future::Future;

/// SQL database interface
/// 
/// This trait provides a unified interface for SQL databases
#[async_trait]
pub trait SqlDb: Send + Sync + Clone + 'static {
    /// The row type returned by queries (e.g., sqlx::postgres::PgRow)
    type Row: Send + Sync;
    
    /// The argument type for parameterized queries
    type Arguments<'a>: Send + Sync;
    
    /// Connect to database with connection pooling
    async fn connect(url: &str) -> Result<Self>
    where
        Self: Sized;
    
    /// Execute a query that doesn't return rows (INSERT, UPDATE, DELETE)
    async fn execute(&self, sql: &str) -> Result<u64>;
    
    /// Execute a query with parameters
    async fn execute_with<'a>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<u64>;
    
    /// Query multiple rows
    async fn query<T>(&self, sql: &str) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;
    
    /// Query multiple rows with parameters
    async fn query_with<'a, T>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;
    
    /// Query single row (errors if not found)
    async fn query_one<T>(&self, sql: &str) -> Result<T>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;
    
    /// Query single row with parameters
    async fn query_one_with<'a, T>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<T>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;
    
    /// Query optional single row
    async fn query_optional<T>(&self, sql: &str) -> Result<Option<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;
    
    /// Query optional single row with parameters
    async fn query_optional_with<'a, T>(
        &self,
        sql: &str,
        args: Self::Arguments<'a>,
    ) -> Result<Option<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;
    
    /// Execute a transaction
    async fn transaction<F, R, Fut>(&self, f: F) -> Result<R>
    where
        F: FnOnce(Self) -> Fut + Send,
        Fut: Future<Output = Result<R>> + Send,
        R: Send;
    
    /// Check if we're in a transaction
    fn in_transaction(&self) -> bool;
    
    /// Get pool statistics (for monitoring)
    fn pool_stats(&self) -> PoolStats;
}

/// Connection pool statistics
#[derive(Debug, Clone, Copy)]
pub struct PoolStats {
    pub size: u32,
    pub idle: u32,
    pub active: u32,
    pub max_size: u32,
}

/// Key-value store interface
/// 
/// This trait provides a unified interface for KV stores
#[async_trait]
pub trait KvStore: Send + Sync + Clone + 'static {
    /// Connect to KV store with connection pooling
    async fn connect(url: &str) -> Result<Self>
    where
        Self: Sized;
    
    // ===== Basic Operations =====
    
    /// Set a value with automatic serialization
    async fn set<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + Send + Sync;
    
    /// Get a value with automatic deserialization
    async fn get<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned;
    
    /// Set raw bytes (fastest, no serialization)
    async fn set_bytes(&self, key: &str, value: &[u8]) -> Result<()>;
    
    /// Get raw bytes (fastest, no deserialization)
    async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>>;
    
    /// Delete a key
    async fn delete(&self, key: &str) -> Result<bool>;
    
    /// Check if key exists
    async fn exists(&self, key: &str) -> Result<bool>;
    
    // ===== Batch Operations =====
    
    /// Get multiple keys in one round-trip
    async fn mget<T>(&self, keys: &[&str]) -> Result<Vec<Option<T>>>
    where
        T: DeserializeOwned;
    
    /// Set multiple key-value pairs in one round-trip
    async fn mset<T>(&self, pairs: &[(&str, &T)]) -> Result<()>
    where
        T: Serialize + Send + Sync;
    
    /// Delete multiple keys
    async fn mdel(&self, keys: &[&str]) -> Result<u64>;
    
    // ===== Atomic Operations =====
    
    /// Atomic increment/decrement
    async fn incr(&self, key: &str, delta: i64) -> Result<i64>;
    
    /// Set if not exists (returns true if set)
    async fn set_nx<T>(&self, key: &str, value: &T) -> Result<bool>
    where
        T: Serialize + Send + Sync;
    
    /// Compare and swap (returns true if swapped)
    async fn cas<T>(&self, key: &str, old: &T, new: &T) -> Result<bool>
    where
        T: Serialize + Send + Sync + PartialEq;
    
    // ===== TTL Operations =====
    
    /// Set with expiration
    async fn set_ex<T>(&self, key: &str, value: &T, ttl_secs: u64) -> Result<()>
    where
        T: Serialize + Send + Sync;
    
    /// Set expiration on existing key (returns true if key exists)
    async fn expire(&self, key: &str, ttl_secs: u64) -> Result<bool>;
    
    /// Get remaining TTL in seconds (None = no TTL, Some(0) = expired)
    async fn ttl(&self, key: &str) -> Result<Option<u64>>;
    
    // ===== Pattern Operations =====
    
    /// Find keys matching pattern (use sparingly in production!)
    async fn scan(&self, pattern: &str, limit: usize) -> Result<Vec<String>>;
    
    // ===== Monitoring =====
    
    /// Get connection pool statistics
    fn pool_stats(&self) -> PoolStats;
    
    /// Ping to check connection
    async fn ping(&self) -> Result<()>;
}

// ============================================================================
// OPTIONAL: QUERYABLE TRAIT FOR COMPILE-TIME SQL CHECKING
// ============================================================================

/// Trait for types that can be queried from the database
/// This enables compile-time SQL checking with sqlx
pub trait Queryable: Sized {
    /// Table name for this entity
    const TABLE_NAME: &'static str;
    
    /// Column names for SELECT queries
    const COLUMNS: &'static [&'static str];
    
    /// Generate SELECT query
    fn select_query() -> String {
        format!(
            "SELECT {} FROM {}",
            Self::COLUMNS.join(", "),
            Self::TABLE_NAME
        )
    }
    
    /// Generate SELECT by ID query
    fn select_by_id_query() -> String {
        format!("{} WHERE id = $1", Self::select_query())
    }
}
use crate::{
    error::{DbError, Result},
    traits::{PoolStats, SqlDb},
};
use async_trait::async_trait;
use sqlx::{
    Executor,
    postgres::{PgArguments, PgPool, PgPoolOptions, PgRow},
};
use std::sync::Arc;

/// PostgreSQL backend
#[derive(Clone)]
pub struct PostgresDb {
    pool: PgPool,
    config: Arc<DbConfig>,
}

/// Database configuration
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_ms: u64,
    pub idle_timeout_secs: u64,
    pub max_lifetime_secs: u64,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            max_connections: 100,
            min_connections: 20,
            acquire_timeout_ms: 800,
            idle_timeout_secs: 60,
            max_lifetime_secs: 1800,
        }
    }
}

impl PostgresDb {
    /// Create a new PostgresDb with custom configuration and URL
    pub async fn with_config(url: &str, config: DbConfig) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(std::time::Duration::from_millis(config.acquire_timeout_ms))
            .idle_timeout(std::time::Duration::from_secs(config.idle_timeout_secs))
            .max_lifetime(std::time::Duration::from_secs(config.max_lifetime_secs))
            .test_before_acquire(false)
            .connect(url)
            .await
            .map_err(|e| DbError::Connection {
                message: format!("Failed to connect to PostgreSQL: {}", e),
                source: Some(Box::new(e)),
            })?;

        Ok(Self {
            pool,
            config: Arc::new(config),
        })
    }

    /// Get a reference to the underlying pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // Convenience methods for direct sqlx support (execute and query commands with raw sqlx)

    /// Execute a simple query without parameters
    pub async fn execute(&self, sql: &str) -> Result<u64> {
        // Calls the trait method with default arguments
        self.execute_with(sql, PgArguments::default()).await
    }

    /// Query multiple rows without parameters
    pub async fn query<T>(&self, sql: &str) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r PgRow>,
    {
        self.query_with(sql, PgArguments::default()).await
    }

    /// Query single row without parameters
    pub async fn query_one<T>(&self, sql: &str) -> Result<T>
    where
        T: Send + Unpin + for<'r> From<&'r PgRow>,
    {
        self.query_one_with(sql, PgArguments::default()).await
    }

    /// Query optional single row without parameters
    pub async fn query_optional<T>(&self, sql: &str) -> Result<Option<T>>
    where
        T: Send + Unpin + for<'r> From<&'r PgRow>,
    {
        self.query_optional_with(sql, PgArguments::default()).await
    }
}

#[async_trait]
impl SqlDb for PostgresDb {
    type Row = PgRow;
    type Arguments<'a> = PgArguments;
    type NativePool = PgPool;

    async fn connect(url: &str) -> Result<Self> {
        let config = DbConfig::default();
        Self::with_config(url, config).await
    }

    async fn execute_with<'a>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<u64> {
        let query = sqlx::query_with(sql, args);
        let result = self.pool.execute(query).await?;
        Ok(result.rows_affected())
    }

    async fn query_with<'a, T>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
    {
        let query = sqlx::query_with(sql, args);
        let rows: Vec<PgRow> = self.pool.fetch_all(query).await?;
        Ok(rows.iter().map(T::from).collect())
    }

    async fn query_one_with<'a, T>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<T>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
    {
        let query = sqlx::query_with(sql, args);
        let row: PgRow = self.pool.fetch_one(query).await?;
        Ok(T::from(&row))
    }

    async fn query_optional_with<'a, T>(
        &self,
        sql: &str,
        args: Self::Arguments<'a>,
    ) -> Result<Option<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
    {
        let query = sqlx::query_with(sql, args);
        let row: Option<PgRow> = self.pool.fetch_optional(query).await?;
        Ok(row.as_ref().map(T::from))
    }

    fn native_pool(&self) -> &Self::NativePool {
        &self.pool
    }

    fn pool_stats(&self) -> PoolStats {
        let size = self.pool.size();
        let idle = self.pool.num_idle() as u32;
        PoolStats {
            size,
            idle,
            active: size - idle,
            max_size: self.config.max_connections,
        }
    }
}

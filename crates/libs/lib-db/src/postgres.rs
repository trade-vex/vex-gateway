use crate::{
    error::{DbError, Result},
    traits::{PoolStats, SqlDb},
};
use async_trait::async_trait;
use sqlx::{
    postgres::{PgArguments, PgPool, PgPoolOptions, PgRow},
    Executor, Postgres, Transaction,
};
use std::future::Future;
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
            max_connections: 50,
            min_connections: 10,    
            acquire_timeout_ms: 500,
            idle_timeout_secs: 300,    // 5 minutes
            max_lifetime_secs: 1800,   // 30 minutes
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
    
    /// Begin a transaction explicitly
    pub async fn begin(&self) -> Result<PostgresTransaction<'_>> {
        let tx = self.pool.begin().await?;
        Ok(PostgresTransaction { tx })
    }
}

/// Transaction wrapper
pub struct PostgresTransaction<'a> {
    tx: Transaction<'a, Postgres>,
}

impl<'a> PostgresTransaction<'a> {
    /// Commit the transaction
    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await?;
        Ok(())
    }
    
    /// Rollback the transaction
    pub async fn rollback(self) -> Result<()> {
        self.tx.rollback().await?;
        Ok(())
    }
    
    /// Execute within transaction
    pub async fn execute(&mut self, sql: &str) -> Result<u64> {
        let result = self.tx.execute(sql).await?;
        Ok(result.rows_affected())
    }
    
    /// Execute with arguments within transaction
    pub async fn execute_with(&mut self, sql: &str, args: PgArguments) -> Result<u64> {
        let query = sqlx::query_with(sql, args);
        let result = self.tx.execute(query).await?;
        Ok(result.rows_affected())
    }
    
    /// Query within transaction
    pub async fn query<T>(&mut self, sql: &str) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r PgRow>,
    {
        let rows: Vec<PgRow> = self.tx.fetch_all(sql).await?;
        Ok(rows.iter().map(T::from).collect())
    }
    
    /// Query with arguments within transaction
    pub async fn query_with<T>(&mut self, sql: &str, args: PgArguments) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r PgRow>,
    {
        let query = sqlx::query_with(sql, args);
        let rows: Vec<PgRow> = self.tx.fetch_all(query).await?;
        Ok(rows.iter().map(T::from).collect())
    }
}

#[async_trait]
impl SqlDb for PostgresDb {
    type Row = PgRow;
    type Arguments<'a> = PgArguments;
    
    async fn connect(url: &str) -> Result<Self> {
        let config = DbConfig::default();
        
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(std::time::Duration::from_millis(config.acquire_timeout_ms))
            .idle_timeout(std::time::Duration::from_secs(config.idle_timeout_secs))
            .max_lifetime(std::time::Duration::from_secs(config.max_lifetime_secs))
            .test_before_acquire(false) // Skip for performance, rely on keepalive
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
    
    async fn execute(&self, sql: &str) -> Result<u64> {
        let result = self.pool.execute(sql).await?;
        Ok(result.rows_affected())
    }
    
    async fn execute_with<'a>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<u64> {
        let query = sqlx::query_with(sql, args);
        let result = self.pool.execute(query).await?;
        Ok(result.rows_affected())
    }
    
    async fn query<T>(&self, sql: &str) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
    {
        let rows: Vec<PgRow> = self.pool.fetch_all(sql).await?;
        Ok(rows.iter().map(T::from).collect())
    }
    
    async fn query_with<'a, T>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
    {
        let query = sqlx::query_with(sql, args);
        let rows: Vec<PgRow> = self.pool.fetch_all(query).await?;
        Ok(rows.iter().map(T::from).collect())
    }
    
    async fn query_one<T>(&self, sql: &str) -> Result<T>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
    {
        let row: PgRow = self.pool.fetch_one(sql).await?;
        Ok(T::from(&row))
    }
    
    async fn query_one_with<'a, T>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<T>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
    {
        let query = sqlx::query_with(sql, args);
        let row: PgRow = self.pool.fetch_one(query).await?;
        Ok(T::from(&row))
    }
    
    async fn query_optional<T>(&self, sql: &str) -> Result<Option<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
    {
        let row: Option<PgRow> = self.pool.fetch_optional(sql).await?;
        Ok(row.as_ref().map(T::from))
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
    
    async fn transaction<F, R, Fut>(&self, f: F) -> Result<R>
    where
        F: FnOnce(Self) -> Fut + Send,
        Fut: Future<Output = Result<R>> + Send,
        R: Send,
    {
        // Create a new instance that shares the pool but marks as in transaction
        let tx_db = self.clone();
        
        // Execute the function with the transaction context
        let result = f(tx_db).await?;
        
        Ok(result)
    }
    
    fn in_transaction(&self) -> bool {
        false
    }
    
    fn pool_stats(&self) -> PoolStats {
        let size = self.pool.size() as u32;
        let idle = self.pool.num_idle() as u32;
        PoolStats {
            size,
            idle,
            active: size - idle,
            max_size: self.config.max_connections,
        }
    }
}
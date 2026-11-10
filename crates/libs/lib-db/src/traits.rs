//! # Database Traits
//!
//! This module defines the core trait system for the database library, providing
//! unified interfaces for both SQL databases and key-value stores.
//!
//! ## Architecture Overview
//!
//! The trait system is designed around the **Escape Hatch pattern**, providing:
//! - **High-level abstractions** for common operations
//! - **Low-level access** when needed for performance or advanced features
//! - **Conditional compilation** based on feature flags
//!
//! ## Trait Hierarchy
//!
//! ### SQL Database Traits
//! - `SqlDb` - Core SQL database interface with connection pooling
//!
//! ### Key-Value Store Traits
//! - `BasicKvStore` - Core CRUD operations with serialization
//! - `AdvancedKvStore` - Batch operations and health monitoring
//! - `AtomicKvStore` - Atomic operations (increment, CAS, NX)
//! - `ExpiringKvStore` - TTL and expiration management
//! - `KvStore` - Complete interface combining all capabilities
//!
//! ## Usage Examples
//!
//! ### SQL Database
//! ```rust,ignore
//! use lib_db::traits::SqlDb;
//! use lib_db::postgres::PostgresDb;
//! use lib_db::error::Result;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Count {
//!     count: i64,
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
//!     
//!     // Execute INSERT/UPDATE/DELETE
//!     let affected = db.execute("INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com')").await?;
//!     
//!     // Query data with SELECT
//!     let results: Vec<Count> = db.query("SELECT COUNT(*) as count FROM users WHERE active = true").await?;
//!     
//!     Ok(())
//! }
//! ```
//!
//! ### Key-Value Store
//! ```rust
//! use lib_db::traits::{BasicKvStore, AdvancedKvStore, AtomicKvStore};
//! use lib_db::backends::kv::memory_kv::MemoryDb;
//! use lib_db::error::Result;
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct User {
//!     id: u32,
//!     name: String,
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let kv = MemoryDb::connect("memory://").await?;
//!     
//!     // Basic operations
//!     let user = User { id: 123, name: "Alice".to_string() };
//!     kv.set("user:123", &user).await?;
//!     let retrieved: Option<User> = kv.get("user:123").await?;
//!     
//!     // Advanced operations
//!     let stats = kv.pool_stats();
//!     
//!     // Atomic operations
//!     let new_count = kv.incr("counter", 1).await?;
//!     
//!     Ok(())
//! }
//! ```
//!
//! ## Error Handling
//!
//! All traits use the unified `Result<T>` type which wraps `DbError`. This provides
//! consistent error handling across different database backends:
//!
//! ```rust
//! use lib_db::error::{Result, DbError};
//!
//! async fn handle_errors() -> Result<()> {
//!     // Simulate a database operation that might fail
//!     let result: Result<()> = Ok(());
//!     match result {
//!         Ok(result) => Ok(result),
//!         Err(DbError::Connection { message: _, source: _ }) => {
//!             // Handle connection errors
//!             Ok(())
//!         }
//!         Err(DbError::Serialization { message: _, source: _ }) => {
//!             // Handle serialization errors
//!             Ok(())
//!         }
//!         Err(e) => Err(e),
//!     }
//! }
//! ```

use crate::error::Result;
use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

/// Connection pool statistics for monitoring and debugging
///
/// Provides real-time information about connection pool health and usage.
/// Useful for monitoring application performance and detecting connection issues.
///
/// ## Fields
/// - `size` - Current number of connections in the pool
/// - `idle` - Number of idle connections available for use
/// - `active` - Number of connections currently in use
/// - `max_size` - Maximum number of connections allowed in the pool
///
/// ## Example Usage
/// ```rust
/// use lib_db::traits::PoolStats;
///
/// fn monitor_pool(stats: PoolStats) {
///     let utilization = (stats.active as f64 / stats.max_size as f64) * 100.0;
///     println!("Pool utilization: {:.1}%", utilization);
///     
///     if stats.idle == 0 {
///         println!("Warning: No idle connections available");
///     }
/// }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct PoolStats {
    /// Current number of connections in the pool
    pub size: u32,
    /// Number of idle connections available for use
    pub idle: u32,
    /// Number of connections currently in use
    pub active: u32,
    /// Maximum number of connections allowed in the pool
    pub max_size: u32,
}

/// SQL database interface with connection pooling and parameterized queries
///
/// This trait provides a unified interface for SQL databases, abstracting away
/// the differences between different database engines while maintaining high
/// performance through connection pooling and efficient query execution.
///
/// ## Key Features
/// - **Connection pooling** for efficient resource management
/// - **Parameterized queries** to prevent SQL injection
/// - **Type-safe row mapping** with automatic deserialization
/// - **Escape hatch access** to native database clients
/// - **Pool monitoring** for health checks and debugging
///
/// ## Supported Operations
/// - Execute queries (INSERT, UPDATE, DELETE)
/// - Query single or multiple rows
/// - Optional row queries (returns None if not found)
/// - Direct access to native connection pools
///
/// ## Example Usage
/// ```rust,ignore
/// use lib_db::traits::SqlDb;
/// use lib_db::postgres::PostgresDb;
/// use lib_db::error::Result;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct User {
///     id: i64,
///     name: String,
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     // Connect to database (requires actual database setup)
///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
///     
///     // Insert a new user (execute for INSERT/UPDATE/DELETE)
///     let affected = db.execute("INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com')").await?;
///     
///     // Query users (query for SELECT)
///     let users: Vec<User> = db.query("SELECT id, name FROM users").await?;
///     
///     // Monitor pool health
///     let stats = db.pool_stats();
///     println!("Active connections: {}", stats.active);
///     
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait SqlDb: Send + Sync + Clone + 'static {
    /// The row type returned by queries (e.g., sqlx::postgres::PgRow)
    type Row: Send + Sync;

    /// The argument type for parameterized queries
    type Arguments<'a>: Send + Sync;

    /// The native pool type (e.g., sqlx::PgPool)
    type NativePool: Send + Sync;
    /// Connect to database with connection pooling
    ///
    /// Establishes a connection pool to the database using the provided URL.
    /// The connection string format varies by database type:
    /// - PostgreSQL: `postgres://user:pass@localhost/dbname`
    /// - MySQL: `mysql://user:pass@localhost/dbname`
    /// - SQLite: `sqlite://path/to/database.db`
    ///
    /// # Arguments
    /// * `url` - Database connection string
    ///
    /// # Returns
    /// * `Ok(Self)` - Connected database instance with connection pool
    /// * `Err(DbError::Connection)` - Connection failed
    /// * `Err(DbError::Configuration)` - Invalid connection string
    ///
    /// # Example
    /// ```rust,ignore
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     // Connect to database (requires actual database setup)
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/mydb").await?;
    ///     Ok(())
    /// }
    /// ```
    async fn connect(url: &str) -> Result<Self>
    where
        Self: Sized;

    /// Execute a query with parameters
    ///
    /// Executes a SQL statement that doesn't return rows (INSERT, UPDATE, DELETE).
    /// Returns the number of affected rows.
    ///
    /// # Arguments
    /// * `sql` - SQL statement with parameter placeholders
    /// * `args` - Parameter values for the query
    ///
    /// # Returns
    /// * `Ok(u64)` - Number of affected rows
    /// * `Err(DbError::Query)` - Query execution failed
    /// * `Err(DbError::Connection)` - Connection error
    ///
    /// # Example
    /// ```rust,ignore
    /// // For parameterless queries, use the convenience method execute() instead
    /// // This example shows parameterized usage (requires sqlx types)
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    /// use sqlx::Arguments;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     
    ///     // With parameters (bind values to prevent SQL injection)
    ///     let mut args = <PostgresDb as SqlDb>::Arguments::default();
    ///     args.add("Alice");
    ///     args.add("alice@example.com");
    ///     let affected = db.execute_with("INSERT INTO users (name, email) VALUES ($1, $2)", args).await?;
    ///     
    ///     Ok(())
    /// }
    /// ```
    async fn execute_with<'a>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<u64>;

    /// Query multiple rows with parameters
    ///
    /// Executes a SELECT query and returns all matching rows.
    ///
    /// # Arguments
    /// * `sql` - SQL SELECT statement with parameter placeholders
    /// * `args` - Parameter values for the query
    ///
    /// # Returns
    /// * `Ok(Vec<T>)` - All matching rows deserialized to type T
    /// * `Err(DbError::Query)` - Query execution failed
    /// * `Err(DbError::Serialization)` - Row deserialization failed
    /// * `Err(DbError::Connection)` - Connection error
    ///
    /// # Example
    /// ```rust,ignore
    /// // For parameterless queries, use the convenience method query() instead
    /// // This example shows parameterized usage (requires sqlx types)
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    /// use sqlx::Arguments;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     
    ///     // With parameters
    ///     let mut args = <PostgresDb as SqlDb>::Arguments::default();
    ///     args.add(true);
    ///     let users: Vec<User> = db.query_with("SELECT id, name FROM users WHERE active = $1", args).await?;
    ///     
    ///     Ok(())
    /// }
    /// ```
    async fn query_with<'a, T>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;
    /// Query single row with parameters
    ///
    /// Executes a SELECT query and returns exactly one row.
    ///
    /// # Arguments
    /// * `sql` - SQL SELECT statement with parameter placeholders
    /// * `args` - Parameter values for the query
    ///
    /// # Returns
    /// * `Ok(T)` - The single row deserialized to type T
    /// * `Err(DbError::Query)` - Query execution failed or no rows returned
    /// * `Err(DbError::Serialization)` - Row deserialization failed
    /// * `Err(DbError::Connection)` - Connection error
    ///
    /// # Panics
    /// This method will return an error if the query returns more than one row.
    ///
    /// # Example
    /// ```rust,ignore
    /// // For parameterless queries, use the convenience method query_one() instead  
    /// // This example shows parameterized usage (requires sqlx types)
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    /// use sqlx::Arguments;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     
    ///     // With parameters
    ///     let mut args = <PostgresDb as SqlDb>::Arguments::default();
    ///     args.add(123i64);
    ///     let user: User = db.query_one_with("SELECT id, name FROM users WHERE id = $1", args).await?;
    ///     
    ///     Ok(())
    /// }
    /// ```
    async fn query_one_with<'a, T>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<T>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;

    /// Query optional single row with parameters
    ///
    /// Executes a SELECT query and returns one row if found, None otherwise.
    ///
    /// # Arguments
    /// * `sql` - SQL SELECT statement with parameter placeholders
    /// * `args` - Parameter values for the query
    ///
    /// # Returns
    /// * `Ok(Some(T))` - Row found and deserialized to type T
    /// * `Ok(None)` - No rows found
    /// * `Err(DbError::Query)` - Query execution failed
    /// * `Err(DbError::Serialization)` - Row deserialization failed
    /// * `Err(DbError::Connection)` - Connection error
    ///
    /// # Panics
    /// This method will return an error if the query returns more than one row.
    ///
    /// # Example
    /// ```rust,ignore
    /// // For parameterless queries, use the convenience method query_optional() instead
    /// // This example shows parameterized usage (requires sqlx types)
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    /// use sqlx::Arguments;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     
    ///     // With parameters
    ///     let mut args = <PostgresDb as SqlDb>::Arguments::default();
    ///     args.add("alice@example.com");
    ///     let user: Option<User> = db.query_optional_with("SELECT id, name FROM users WHERE email = $1", args).await?;
    ///     
    ///     Ok(())
    /// }
    /// ```
    async fn query_optional_with<'a, T>(
        &self,
        sql: &str,
        args: Self::Arguments<'a>,
    ) -> Result<Option<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>;

    /// Escape Hatch Pattern: Get direct access to the native sqlx pool
    ///
    /// Provides direct access to the underlying database client for advanced
    /// operations that aren't covered by the trait interface.
    ///
    /// # Returns
    /// * `&Self::NativePool` - Reference to the native connection pool
    ///
    /// # Example
    /// ```rust,ignore
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     
    ///     // Access native pool for advanced operations
    ///     let native_pool = db.native_pool();
    ///     
    ///     // Use native pool directly for complex transactions
    ///     let mut tx = native_pool.begin().await?;
    ///     // ... perform complex transaction logic
    ///     tx.commit().await?;
    ///     
    ///     Ok(())
    /// }
    /// ```
    fn native_pool(&self) -> &Self::NativePool;

    /// Get pool statistics for monitoring
    ///
    /// Returns current connection pool statistics for health monitoring
    /// and debugging purposes.
    ///
    /// # Returns
    /// * `PoolStats` - Current pool statistics
    ///
    /// # Example
    /// ```rust,ignore
    /// use lib_db::traits::{SqlDb, PoolStats};
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     
    ///     let stats = db.pool_stats();
    ///     println!("Pool size: {}/{}", stats.size, stats.max_size);
    ///     println!("Active connections: {}", stats.active);
    ///     println!("Idle connections: {}", stats.idle);
    ///     
    ///     // Check for potential issues
    ///     if stats.active == stats.max_size {
    ///         println!("Warning: All connections are active!");
    ///     }
    ///     
    ///     Ok(())
    /// }
    /// ```
    fn pool_stats(&self) -> PoolStats;

    /// Convenience method: Execute a query without parameters
    ///
    /// This is a shorthand for `execute_with(sql, default_args)` for queries that don't
    /// need parameters. Use for INSERT, UPDATE, DELETE statements.
    ///
    /// # Arguments
    /// * `sql` - SQL statement without parameters
    ///
    /// # Returns
    /// * `Ok(u64)` - Number of affected rows
    /// * `Err(DbError)` - Query execution failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     let affected = db.execute("DELETE FROM sessions WHERE expires_at < NOW()").await?;
    ///     Ok(())
    /// }
    /// ```
    async fn execute(&self, sql: &str) -> Result<u64>
    where
        Self::Arguments<'static>: Default,
    {
        self.execute_with(sql, Default::default()).await
    }

    /// Convenience method: Query multiple rows without parameters
    ///
    /// This is a shorthand for `query_with(sql, default_args)` for queries that don't
    /// need parameters. Use for SELECT statements that return multiple rows.
    ///
    /// # Arguments
    /// * `sql` - SQL SELECT statement without parameters
    ///
    /// # Returns
    /// * `Ok(Vec<T>)` - All matching rows
    /// * `Err(DbError)` - Query execution failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     let users: Vec<User> = db.query("SELECT id, name FROM users").await?;
    ///     Ok(())
    /// }
    /// ```
    async fn query<T>(&self, sql: &str) -> Result<Vec<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
        Self::Arguments<'static>: Default,
    {
        self.query_with(sql, Default::default()).await
    }

    /// Convenience method: Query single row without parameters
    ///
    /// This is a shorthand for `query_one_with(sql, default_args)` for queries that don't
    /// need parameters. Use for SELECT statements that return exactly one row.
    ///
    /// # Arguments
    /// * `sql` - SQL SELECT statement without parameters
    ///
    /// # Returns
    /// * `Ok(T)` - The single row
    /// * `Err(DbError)` - Query execution failed or no rows returned
    ///
    /// # Example
    /// ```rust,ignore
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Count {
    ///     count: i64,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     let result: Count = db.query_one("SELECT COUNT(*) as count FROM users").await?;
    ///     Ok(())
    /// }
    /// ```
    async fn query_one<T>(&self, sql: &str) -> Result<T>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
        Self::Arguments<'static>: Default,
    {
        self.query_one_with(sql, Default::default()).await
    }

    /// Convenience method: Query optional single row without parameters
    ///
    /// This is a shorthand for `query_optional_with(sql, default_args)` for queries that don't
    /// need parameters. Use for SELECT statements that return zero or one row.
    ///
    /// # Arguments
    /// * `sql` - SQL SELECT statement without parameters
    ///
    /// # Returns
    /// * `Ok(Some(T))` - Row found
    /// * `Ok(None)` - No rows found
    /// * `Err(DbError)` - Query execution failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use lib_db::traits::SqlDb;
    /// use lib_db::postgres::PostgresDb;
    /// use lib_db::error::Result;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let db = PostgresDb::connect("postgres://user:pass@localhost/db").await?;
    ///     let user: Option<User> = db.query_optional("SELECT id, name FROM users WHERE id = 1").await?;
    ///     Ok(())
    /// }
    /// ```
    async fn query_optional<T>(&self, sql: &str) -> Result<Option<T>>
    where
        T: Send + Unpin + for<'r> From<&'r Self::Row>,
        Self::Arguments<'static>: Default,
    {
        self.query_optional_with(sql, Default::default()).await
    }
}

// ============================================================================
// KV STORE TRAIT SYSTEM
// ============================================================================

/// Basic key-value store operations
///
/// This trait defines the core operations that all KV stores must support.
/// It provides the foundation for storing and retrieving data with automatic
/// serialization/deserialization using JSON (serde_json).
///
/// ## Core Capabilities
/// - Basic CRUD operations (get, set, delete, exists)
/// - Raw byte operations for maximum performance
/// - Automatic serialization/deserialization
/// - Connection management
///
/// ## Example Usage
/// ```rust,no_run
/// use lib_db::traits::BasicKvStore;
/// use lib_db::redis::RedisDb;
/// use lib_db::error::Result;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct User {
///     id: u32,
///     name: String,
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let kv = RedisDb::connect("redis://localhost:6379").await?;
///     
///     // Basic operations
///     kv.set("user:123", &User { id: 123, name: "Alice".to_string() }).await?;
///     let user: Option<User> = kv.get("user:123").await?;
///     let exists = kv.exists("user:123").await?;
///     let deleted = kv.delete("user:123").await?;
///     
///     // Raw byte operations (faster)
///     kv.set_bytes("raw:key", &[1, 2, 3, 4]).await?;
///     let bytes: Option<Vec<u8>> = kv.get_bytes("raw:key").await?;
///     
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait BasicKvStore: Send + Sync + Clone + 'static {
    /// Connect to KV store
    ///
    /// Establishes a connection to the KV store using the provided URL.
    /// The connection string format varies by store type:
    /// - Redis: `redis://localhost:6379`
    /// - Memcached: `memcached://localhost:11211`
    /// - DynamoDB: `dynamodb://us-east-1`
    ///
    /// # Arguments
    /// * `url` - Connection string specific to the KV store
    ///
    /// # Returns
    /// * `Ok(Self)` - Connected KV store instance
    /// * `Err(DbError::Connection)` - Connection failed
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::redis::RedisDb;
    /// use lib_db::traits::BasicKvStore;
    /// use lib_db::error::Result;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     Ok(())
    /// }
    /// ```
    async fn connect(url: &str) -> Result<Self>
    where
        Self: Sized;

    /// Set a value with automatic serialization
    ///
    /// Serializes the value as JSON and stores it under the given key.
    /// If the key already exists, the value will be overwritten.
    ///
    /// # Arguments
    /// * `key` - The key to store the value under
    /// * `value` - The value to store (must implement `Serialize`)
    ///
    /// # Returns
    /// * `Ok(())` - Value stored successfully
    /// * `Err(DbError::Serialization)` - Serialization failed
    /// * `Err(DbError::Connection)` - Connection error
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::traits::BasicKvStore;
    /// use lib_db::redis::RedisDb;
    /// use lib_db::error::Result;
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct User {
    ///     id: u32,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     kv.set("user:123", &User { id: 123, name: "Alice".to_string() }).await?;
    ///     Ok(())
    /// }
    /// ```
    async fn set<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + Send + Sync;

    /// Get a value with automatic deserialization
    ///
    /// Retrieves and deserializes a value from the given key.
    ///
    /// # Arguments
    /// * `key` - The key to retrieve
    ///
    /// # Returns
    /// * `Ok(Some(T))` - Value found and deserialized successfully
    /// * `Ok(None)` - Key not found
    /// * `Err(DbError::Serialization)` - Deserialization failed
    /// * `Err(DbError::Connection)` - Connection error
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::traits::BasicKvStore;
    /// use lib_db::redis::RedisDb;
    /// use lib_db::error::Result;
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct User {
    ///     id: u32,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     let user: Option<User> = kv.get("user:123").await?;
    ///     Ok(())
    /// }
    /// ```
    async fn get<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned;

    /// Delete a key
    ///
    /// Removes the specified key from the store.
    ///
    /// # Arguments
    /// * `key` - The key to delete
    ///
    /// # Returns
    /// * `Ok(true)` - Key was deleted
    /// * `Ok(false)` - Key didn't exist
    /// * `Err(DbError::Connection)` - Connection error
    async fn delete(&self, key: &str) -> Result<bool>;

    /// Check if key exists
    ///
    /// Returns whether the specified key exists in the store.
    ///
    /// # Arguments
    /// * `key` - The key to check
    ///
    /// # Returns
    /// * `Ok(true)` - Key exists
    /// * `Ok(false)` - Key doesn't exist
    /// * `Err(DbError::Connection)` - Connection error
    async fn exists(&self, key: &str) -> Result<bool>;

    /// Set raw bytes (fastest, no serialization)
    ///
    /// Stores raw bytes without any serialization overhead.
    /// Use this for maximum performance when you have pre-serialized data.
    ///
    /// # Arguments
    /// * `key` - The key to store under
    /// * `value` - Raw bytes to store
    ///
    /// # Returns
    /// * `Ok(())` - Bytes stored successfully
    /// * `Err(DbError::Connection)` - Connection error
    async fn set_bytes(&self, key: &str, value: &[u8]) -> Result<()>;

    /// Get raw bytes (fastest, no deserialization)
    ///
    /// Retrieves raw bytes without any deserialization overhead.
    /// Use this for maximum performance when you need raw data.
    ///
    /// # Arguments
    /// * `key` - The key to retrieve
    ///
    /// # Returns
    /// * `Ok(Some(Vec<u8>))` - Raw bytes found
    /// * `Ok(None)` - Key not found
    /// * `Err(DbError::Connection)` - Connection error
    async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>>;
}

/// Advanced KV store operations
///
/// This trait provides advanced features that not all KV stores support.
/// It includes connection health monitoring, connection pooling statistics,
/// and batch operations for improved performance.
///
/// ## Advanced Capabilities
/// - Connection health monitoring (ping)
/// - Connection pool statistics
/// - Batch operations (mget, mset, mdel)
///
/// ## Store Support
/// - **Redis** - Full support
/// - **Memcached** - Full support
/// - **DynamoDB** - Partial support (no ping, different pooling)
/// - **Etcd** - No ping concept
/// - **Consul** - Different health check mechanism
///
/// ## Example Usage
/// ```rust,no_run
/// use lib_db::traits::{BasicKvStore, AdvancedKvStore};
/// use lib_db::redis::RedisDb;
/// use lib_db::error::Result;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct User {
///     id: u32,
///     name: String,
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let kv = RedisDb::connect("redis://localhost:6379").await?;
///     
///     // Health check
///     kv.ping().await?;
///     
///     // Pool statistics
///     let stats = kv.pool_stats();
///     println!("Active connections: {}", stats.active);
///     
///     // Batch operations
///     let user1 = User { id: 1, name: "Alice".to_string() };
///     let user2 = User { id: 2, name: "Bob".to_string() };
///     kv.mset(&[("user:1", &user1), ("user:2", &user2)]).await?;
///     let values: Vec<Option<User>> = kv.mget(&["user:1", "user:2"]).await?;
///     let deleted = kv.mdel(&["user:1", "user:2"]).await?;
///     
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait AdvancedKvStore: BasicKvStore {
    /// Ping to check connection health
    ///
    /// Sends a ping command to verify the connection is alive.
    /// Useful for health checks and connection validation.
    ///
    /// # Returns
    /// * `Ok(())` - Connection is healthy
    /// * `Err(DbError::Connection)` - Connection failed
    /// * `Err(DbError::Unsupported)` - Store doesn't support ping
    ///
    /// # Note
    /// Not all KV stores support ping operations. For stores that don't support
    /// ping, this method should return `Err(DbError::Unsupported)`.
    async fn ping(&self) -> Result<()>;

    /// Get connection pool statistics
    ///
    /// Returns current pool state for monitoring and debugging.
    ///
    /// # Returns
    /// * `PoolStats` - Current pool statistics
    ///
    /// # Note
    /// For stores that don't use traditional connection pooling (e.g., DynamoDB),
    /// this should return meaningful statistics about the underlying client state.
    fn pool_stats(&self) -> PoolStats;

    /// Get multiple keys in one round-trip
    ///
    /// Retrieves multiple values in a single network request for better performance.
    /// The returned vector maintains the same order as the input keys.
    ///
    /// # Arguments
    /// * `keys` - Array of keys to retrieve
    ///
    /// # Returns
    /// * `Ok(Vec<Option<T>>)` - Values in same order as keys (None if key not found)
    /// * `Err(DbError::Serialization)` - Deserialization failed for any value
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support batch operations
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::traits::{BasicKvStore, AdvancedKvStore};
    /// use lib_db::redis::RedisDb;
    /// use lib_db::error::Result;
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct User {
    ///     id: u32,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     let users: Vec<Option<User>> = kv.mget(&["user:1", "user:2", "user:3"]).await?;
    ///     Ok(())
    /// }
    /// ```
    async fn mget<T>(&self, keys: &[&str]) -> Result<Vec<Option<T>>>
    where
        T: DeserializeOwned;

    /// Set multiple key-value pairs in one round-trip
    ///
    /// Stores multiple key-value pairs in a single network request.
    /// Atomicity guarantees vary by store implementation.
    ///
    /// # Arguments
    /// * `pairs` - Array of (key, value) pairs to store
    ///
    /// # Returns
    /// * `Ok(())` - All values stored successfully
    /// * `Err(DbError::Serialization)` - Serialization failed for any value
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support batch operations
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::traits::{BasicKvStore, AdvancedKvStore};
    /// use lib_db::redis::RedisDb;
    /// use lib_db::error::Result;
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct User {
    ///     id: u32,
    ///     name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     let user1 = User { id: 1, name: "Alice".to_string() };
    ///     let user2 = User { id: 2, name: "Bob".to_string() };
    ///     let user3 = User { id: 3, name: "Charlie".to_string() };
    ///     kv.mset(&[
    ///         ("user:1", &user1),
    ///         ("user:2", &user2),
    ///         ("user:3", &user3)
    ///     ]).await?;
    ///     Ok(())
    /// }
    /// ```
    async fn mset<T>(&self, pairs: &[(&str, &T)]) -> Result<()>
    where
        T: Serialize + Send + Sync;

    /// Delete multiple keys
    ///
    /// Removes multiple keys in a single operation.
    ///
    /// # Arguments
    /// * `keys` - Array of keys to delete
    ///
    /// # Returns
    /// * `Ok(u64)` - Number of keys actually deleted
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support batch operations
    async fn mdel(&self, keys: &[&str]) -> Result<u64>;
}

/// Atomic operations for KV stores
///
/// This trait provides atomic operations that not all KV stores support.
/// These operations are useful for implementing locks, counters, and
/// concurrent data structures.
///
/// ## Atomic Capabilities
/// - Atomic increment/decrement
/// - Set if not exists (NX)
/// - Compare and swap (CAS)
///
/// ## Store Support
/// - **Redis** - Full support
/// - **Memcached** - Full support
/// - **DynamoDB** - Partial support (no native increment, different CAS)
/// - **Etcd** - No increment, different CAS (revision-based)
/// - **Consul** - No increment, different CAS (modify index-based)
///
/// ## Example Usage
/// ```rust,no_run
/// use lib_db::traits::{BasicKvStore, AtomicKvStore};
/// use lib_db::redis::RedisDb;
/// use lib_db::error::Result;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct LockData {
///     process_id: u32,
///     timestamp: u64,
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let kv = RedisDb::connect("redis://localhost:6379").await?;
///     
///     // Atomic increment
///     let new_count = kv.incr("page_views", 1).await?;
///     
///     // Set if not exists (for locks)
///     let lock_data = LockData { process_id: 123, timestamp: 1234567890 };
///     let was_set = kv.set_nx("lock:resource", &lock_data).await?;
///     if was_set {
///         // We acquired the lock
///     }
///     
///     // Compare and swap
///     let old_value = 42;
///     let new_value = 43;
///     let swapped = kv.cas("counter", &old_value, &new_value).await?;
///     if swapped {
///         // Update was successful
///     } else {
///         // Value changed by another process, retry
///     }
///     
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait AtomicKvStore: BasicKvStore {
    /// Atomic increment/decrement
    ///
    /// Atomically increments or decrements a numeric value.
    /// If the key doesn't exist, it's treated as 0 before the operation.
    ///
    /// # Arguments
    /// * `key` - The key to increment/decrement
    /// * `delta` - Amount to add (positive) or subtract (negative)
    ///
    /// # Returns
    /// * `Ok(i64)` - New value after operation
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support increment
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::traits::{BasicKvStore, AtomicKvStore};
    /// use lib_db::redis::RedisDb;
    /// use lib_db::error::Result;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     let new_count = kv.incr("page_views", 1).await?; // Increment
    ///     let new_count = kv.incr("credits", -5).await?;   // Decrement
    ///     Ok(())
    /// }
    /// ```
    async fn incr(&self, key: &str, delta: i64) -> Result<i64>;

    /// Set if not exists (returns true if set)
    ///
    /// Atomically sets a value only if the key doesn't already exist.
    /// Useful for implementing locks or ensuring uniqueness.
    ///
    /// # Arguments
    /// * `key` - The key to set
    /// * `value` - The value to store
    ///
    /// # Returns
    /// * `Ok(true)` - Value was set (key didn't exist)
    /// * `Ok(false)` - Key already existed, no change made
    /// * `Err(DbError::Serialization)` - Serialization failed
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support NX operations
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::traits::{BasicKvStore, AtomicKvStore};
    /// use lib_db::redis::RedisDb;
    /// use lib_db::error::Result;
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct LockData {
    ///     process_id: u32,
    ///     timestamp: u64,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     let lock_data = LockData { process_id: 123, timestamp: 1234567890 };
    ///     let was_set = kv.set_nx("lock:resource", &lock_data).await?;
    ///     if was_set {
    ///         // We acquired the lock
    ///     }
    ///     Ok(())
    /// }
    /// ```
    async fn set_nx<T>(&self, key: &str, value: &T) -> Result<bool>
    where
        T: Serialize + Send + Sync;

    /// Compare and swap (returns true if swapped)
    ///
    /// Atomically updates a value only if it matches the expected old value.
    /// Useful for optimistic locking and concurrent updates.
    ///
    /// # Arguments
    /// * `key` - The key to update
    /// * `old` - Expected current value
    /// * `new` - New value to set
    ///
    /// # Returns
    /// * `Ok(true)` - Value was updated (old value matched)
    /// * `Ok(false)` - Old value didn't match, no change made
    /// * `Err(DbError::Serialization)` - Serialization failed
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support CAS operations
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::traits::{BasicKvStore, AtomicKvStore};
    /// use lib_db::redis::RedisDb;
    /// use lib_db::error::Result;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     let old_value = 42;
    ///     let new_value = 43;
    ///     let swapped = kv.cas("counter", &old_value, &new_value).await?;
    ///     if swapped {
    ///         // Update was successful
    ///     } else {
    ///         // Value changed by another process, retry
    ///     }
    ///     Ok(())
    /// }
    /// ```
    async fn cas<T>(&self, key: &str, old: &T, new: &T) -> Result<bool>
    where
        T: Serialize + DeserializeOwned + Send + Sync + PartialEq;
}

/// Expiration management for KV stores
///
/// This trait provides expiration functionality that not all KV stores support.
/// It allows setting time-to-live (TTL) values on keys for automatic cleanup.
///
/// ## Expiration Capabilities
/// - Set values with expiration
/// - Set expiration on existing keys
/// - Query remaining TTL
///
/// ## Store Support
/// - **Redis** - Full support
/// - **Memcached** - Full support
/// - **DynamoDB** - Partial support (set expiration, no TTL query)
/// - **Etcd** - No TTL concept
/// - **Consul** - No TTL concept
///
/// ## Example Usage
/// ```rust,no_run
/// use lib_db::traits::{BasicKvStore, ExpiringKvStore};
/// use lib_db::redis::RedisDb;
/// use lib_db::error::Result;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct SessionData {
///     user_id: u32,
///     token: String,
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let kv = RedisDb::connect("redis://localhost:6379").await?;
///     
///     // Set with expiration
///     let session_data = SessionData { user_id: 123, token: "abc123".to_string() };
///     kv.set_ex("session:123", &session_data, 3600).await?; // 1 hour TTL
///     
///     // Set expiration on existing key
///     kv.expire("existing_key", 1800).await?; // 30 minutes TTL
///     
///     // Check remaining TTL
///     let ttl = kv.ttl("session:123").await?;
///     match ttl {
///         Some(seconds) => println!("Expires in {} seconds", seconds),
///         None => println!("No expiration set"),
///     }
///     
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait ExpiringKvStore: BasicKvStore {
    /// Set with expiration
    ///
    /// Stores a value with an automatic expiration time.
    /// The key will be automatically deleted after the specified TTL.
    ///
    /// # Arguments
    /// * `key` - The key to store
    /// * `value` - The value to store
    /// * `ttl_secs` - Time to live in seconds
    ///
    /// # Returns
    /// * `Ok(())` - Value stored with expiration
    /// * `Err(DbError::Serialization)` - Serialization failed
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support expiration
    ///
    /// # Example
    /// ```rust,no_run
    /// use lib_db::traits::{BasicKvStore, ExpiringKvStore};
    /// use lib_db::redis::RedisDb;
    /// use lib_db::error::Result;
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct SessionData {
    ///     user_id: u32,
    ///     token: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let kv = RedisDb::connect("redis://localhost:6379").await?;
    ///     let session_data = SessionData { user_id: 123, token: "abc123".to_string() };
    ///     kv.set_ex("session:123", &session_data, 3600).await?; // 1 hour TTL
    ///     Ok(())
    /// }
    /// ```
    async fn set_ex<T>(&self, key: &str, value: &T, ttl_secs: u64) -> Result<()>
    where
        T: Serialize + Send + Sync;

    /// Set expiration on existing key (returns true if key exists)
    ///
    /// Sets an expiration time on an existing key without changing its value.
    ///
    /// # Arguments
    /// * `key` - The key to set expiration on
    /// * `ttl_secs` - Time to live in seconds
    ///
    /// # Returns
    /// * `Ok(true)` - Expiration set (key existed)
    /// * `Ok(false)` - Key doesn't exist
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support expiration
    async fn expire(&self, key: &str, ttl_secs: u64) -> Result<bool>;

    /// Get remaining TTL in seconds
    ///
    /// Returns the remaining time to live for a key.
    ///
    /// # Arguments
    /// * `key` - The key to check
    ///
    /// # Returns
    /// * `Ok(Some(ttl))` - Remaining TTL in seconds
    /// * `Ok(None)` - Key doesn't exist or has no TTL
    /// * `Err(DbError::Connection)` - Connection error
    /// * `Err(DbError::Unsupported)` - Store doesn't support TTL queries
    ///
    /// # Note
    /// * `Some(0)` indicates the key has expired but not yet been cleaned up
    /// * `Some(u64::MAX)` indicates no TTL is set
    async fn ttl(&self, key: &str) -> Result<Option<u64>>;
}

/// Full-featured key-value store interface
///
/// This trait combines all KV store capabilities into a single interface.
/// It's implemented by stores that support all features (like Redis).
///
/// ## Complete Capabilities
/// - Basic CRUD operations
/// - Advanced features (ping, pooling, batch operations)
/// - Atomic operations (increment, NX, CAS)
/// - Expiration management
///
/// ## Store Support
/// - **Redis** - Full implementation
/// - **Memcached** - Full implementation
/// - **DynamoDB** - Partial (no ping, different CAS)
/// - **Etcd** - Partial (no increment, different CAS, no TTL)
/// - **Consul** - Partial (no increment, different CAS, no TTL)
///
/// ## Example Usage
/// ```rust,no_run
/// use lib_db::traits::{KvStore, BasicKvStore, AdvancedKvStore, AtomicKvStore, ExpiringKvStore};
/// use lib_db::redis::RedisDb;
/// use lib_db::error::Result;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct UserData {
///     id: u32,
///     name: String,
/// }
///
/// #[derive(Serialize, Deserialize)]
/// struct SessionData {
///     user_id: u32,
///     token: String,
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let kv = RedisDb::connect("redis://localhost:6379").await?;
///     
///     // All operations available
///     let user_data = UserData { id: 123, name: "Alice".to_string() };
///     let session_data = SessionData { user_id: 123, token: "abc123".to_string() };
///     kv.set("user:123", &user_data).await?;
///     kv.set_ex("session:123", &session_data, 3600).await?;
///     kv.incr("counter", 1).await?;
///     kv.ping().await?;
///     
///     Ok(())
/// }
/// ```
pub trait KvStore: BasicKvStore + AdvancedKvStore + AtomicKvStore + ExpiringKvStore {}

// Blanket implementation for any type that implements all subtraits
impl<T> KvStore for T where T: BasicKvStore + AdvancedKvStore + AtomicKvStore + ExpiringKvStore {}

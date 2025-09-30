# lib-db: Modular Database Interface

A flexible, trait-based abstraction for both key-value stores and SQL databases with support for multiple backends and feature sets.

## Features

- **Modular Trait System**: Implement only the features your database supports
- **Multiple Backends**: Redis, PostgreSQL, high-performance in-memory store, and extensible for more
- **Performance Optimized**: Raw byte operations, batch support, connection pooling, and fine-grained locking
- **Dual Interface**: Support for both KV stores and SQL databases
- **Memory Store Registry**: Multiple isolated in-memory stores with different configurations

## Architecture

The library uses a modular trait system that provides two main interfaces:

### SQL Database Interface

- **`SqlDb`**: Unified interface for SQL databases with connection pooling, parameterized queries, and escape hatch access

### Key-Value Store Interface

The KV store functionality is split into logical groups:

- **`BasicKvStore`**: Essential CRUD operations (all stores must implement)
- **`AdvancedKvStore`**: Connection health, pooling, and batch operations
- **`AtomicKvStore`**: Atomic operations (increment, NX, CAS)
- **`ExpiringKvStore`**: TTL and expiration management
- **`KvStore`**: Full-featured interface (combines all traits)

### Database Support 

#### SQL Databases

| Feature | PostgreSQL | MySQL* | SQLite* |
|---------|------------|--------|---------|
| Connection Pooling | ✅ | ✅ | ✅ |
| Parameterized Queries | ✅ | ✅ | ✅ |
| Transaction Support | ✅ | ✅ | ✅ |
| Escape Hatch Access | ✅ | ✅ | ✅ |

#### Key-Value Stores

| Feature | Redis | Memory (DashMap) | DynamoDB* | Etcd* | Consul* |
|---------|-------|------------------|-----------|-------|---------|
| Basic CRUD | ✅ | ✅ | ✅ | ✅ | ✅ |
| Raw Bytes | ✅ | ✅ | ✅ | ✅ | ✅ |
| Connection Pooling | ✅ | ✅ | ⚠️ | ❌ | ❌ |
| Ping/Health | ✅ | ✅ | ❌ | ⚠️ | ⚠️ |
| Batch Operations | ✅ | ✅ | ⚠️ | ❌ | ❌ |
| Atomic Increment | ✅ | ✅ | ⚠️ | ❌ | ❌ |
| Set NX | ✅ | ✅ | ⚠️ | ❌ | ❌ |
| Compare & Swap | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ |
| Expiration | ✅ | ✅ | ⚠️ | ❌ | ❌ |
| **Fine-grained Locking** | ❌ | ✅ | ❌ | ❌ | ❌ |
| **Multiple Named Stores** | ❌ | ✅ | ❌ | ❌ | ❌ |
| **Eviction Policies** | ❌ | ✅ | ❌ | ❌ | ❌ |

`* -> Not yet implemented`

## Quick Start

### SQL Database Usage

```rust
use lib_db::{
    error::Result,
    postgres::PostgresDb,
    traits::SqlDb,
};
use sqlx::postgres::PgArguments;

#[tokio::main]
async fn main() -> Result<()> {
    // Connect to PostgreSQL
    let db = PostgresDb::connect("postgresql://user:pass@localhost/dbname").await?;
    
    // Execute a query
    let mut args = PgArguments::default();
    args.add("Alice");
    args.add("alice@example.com");
    let affected = db.execute_with(
        "INSERT INTO users (name, email) VALUES ($1, $2)",
        args
    ).await?;
    
    // Query multiple rows
    let mut args = PgArguments::default();
    args.add(true);
    let users: Vec<User> = db.query_with(
        "SELECT id, name, email FROM users WHERE active = $1",
        args
    ).await?;
    
    // Get native pool for advanced operations
    let native_pool = db.native_pool();
    
    Ok(())
}
```

### Key-Value Store Usage

#### Redis Store

```rust
use lib_db::{
    error::Result,
    redis::RedisDb,
    traits::BasicKvStore,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct User {
    id: u32,
    name: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Connect to Redis
    let kv = RedisDb::connect("redis://localhost:6379").await?;
    
    // Store a user
    let user = User { id: 123, name: "Alice".to_string() };
    kv.set("user:123", &user).await?;
    
    // Retrieve the user
    let retrieved: Option<User> = kv.get("user:123").await?;
    println!("User: {:?}", retrieved);
    
    Ok(())
}
```

#### High-Performance Memory Store

```rust
use lib_db::{
    error::Result,
    memory_kv::{MemoryDb, REGISTRY},
    traits::{BasicKvStore, AdvancedKvStore, AtomicKvStore, ExpiringKvStore},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct User {
    id: u32,
    name: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Method 1: Direct connection to named store
    let kv = MemoryDb::connect("memory://user_cache").await?;
    
    // Method 2: Using the global registry (recommended)
    let user_cache = REGISTRY.get_or_create("user_cache").await;
    let session_store = REGISTRY.get_or_create("sessions").await;
    let temp_data = REGISTRY.get_or_create("temp_data").await;
    
    // Each store is completely isolated
    user_cache.set("user:123", &User { id: 123, name: "Alice".to_string() }).await?;
    session_store.set("session:456", &"token").await?;
    temp_data.set("temp:789", &"data").await?;
    
    // All operations available with high concurrency
    kv.set("user:123", &"Alice").await?;
    kv.set_ex("session:123", &"token", 3600).await?; // 1 hour TTL
    kv.incr("counter", 1).await?;
    kv.ping().await?;
    
    Ok(())
}
```

### Advanced Memory Store Configuration

```rust
use lib_db::{
    memory_kv::{MemoryConfig, EvictionPolicy, REGISTRY},
    traits::BasicKvStore,
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // Create custom configuration
    let config = MemoryConfig {
        max_entries: 10_000,
        default_ttl: Some(Duration::from_secs(300)), // 5 minutes
        eviction_policy: EvictionPolicy::LRU,
        enable_cleanup: true,
        cleanup_interval: Duration::from_secs(60),
        compression_enabled: false,
        metrics_enabled: true,
    };
    
    // Get store with custom config
    let fast_cache = REGISTRY
        .get_or_create_with_config("fast_cache", config)
        .await;
    
    // URL-based configuration
    let kv = MemoryDb::connect("memory://my_store?max_entries=1000&ttl=3600&eviction=lru").await?;
    
    Ok(())
}
```

### Using Different Feature Sets

#### SQL Database Features

```rust
use lib_db::{
    postgres::PostgresDb,
    traits::SqlDb,
};

// Function that works with any SQL database
async fn query_users<T: SqlDb>(db: &T) -> Result<Vec<User>> 
where
    for<'r> T::Row: sqlx::FromRow<'r, T::Row>,
{
    // Note: The actual implementation would depend on how the trait
    // handles parameterized queries generically. For PostgreSQL:
    // let mut args = PgArguments::default();
    // args.add(true);
    // let users = db.query_with("SELECT id, name, email FROM users WHERE active = $1", args).await?;
    todo!("Implementation depends on the actual SqlDb trait design")
}

// Function that uses escape hatch for advanced operations
async fn complex_transaction<T: SqlDb>(db: &T) -> Result<()> {
    let native_pool = db.native_pool();
    // Use native pool for complex transactions, migrations, etc.
    Ok(())
}
```

#### Key-Value Store Features

```rust
use lib_db::{
    memory::MemoryDb,
    redis::RedisDb,
    traits::{BasicKvStore, AdvancedKvStore, AtomicKvStore, ExpiringKvStore},
};

// Function that works with ANY KV store
async fn cache_data<T: BasicKvStore>(kv: &T) -> Result<()> {
    kv.set("cache:key", &"cached_value").await?;
    let value: Option<String> = kv.get("cache:key").await?;
    Ok(())
}

// Function that requires advanced features
async fn advanced_cache<T: BasicKvStore + AdvancedKvStore>(kv: &T) -> Result<()> {
    // Health check
    kv.ping().await?;
    
    // Batch operations
    kv.mset(&[("key1", &"value1"), ("key2", &"value2")]).await?;
    let values: Vec<Option<String>> = kv.mget(&["key1", "key2"]).await?;
    
    Ok(())
}

// Function that requires atomic operations
async fn atomic_counter<T: BasicKvStore + AtomicKvStore>(kv: &T) -> Result<()> {
    let new_value = kv.incr("counter", 1).await?;
    println!("Counter: {}", new_value);
    
    // Set if not exists
    let was_set = kv.set_nx("lock:resource", &"locked").await?;
    if was_set {
        println!("Acquired lock");
    }
    
    Ok(())
}

// Function that requires expiration
async fn session_management<T: BasicKvStore + ExpiringKvStore>(kv: &T) -> Result<()> {
    let session = Session { user_id: 123, token: "abc123" };
    
    // Set with 1 hour expiration
    kv.set_ex("session:123", &session, 3600).await?;
    
    // Check remaining TTL
    let ttl = kv.ttl("session:123").await?;
    println!("Session expires in {:?} seconds", ttl);
    
    Ok(())
}
```

### Database-Specific Examples

```rust
// PostgreSQL
let postgres_db = PostgresDb::connect("postgresql://user:pass@localhost/dbname").await?;
query_users(&postgres_db).await?;
complex_transaction(&postgres_db).await?;

// Memory KV store (high-performance)
let memory_kv = MemoryDb::connect("memory://my_cache").await?;
cache_data(&memory_kv).await?;
advanced_cache(&memory_kv).await?;
atomic_counter(&memory_kv).await?;
session_management(&memory_kv).await?;

// Redis KV store
let redis_kv = RedisDb::connect("redis://localhost:6379").await?;
cache_data(&redis_kv).await?;
advanced_cache(&redis_kv).await?;
atomic_counter(&redis_kv).await?;
session_management(&redis_kv).await?;
```

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
# Minimal installation (no backends)
lib-db = { path = "crates/libs/lib-db", default-features = false }

# Enable specific backends
lib-db = { path = "crates/libs/lib-db", features = ["redis"] }
lib-db = { path = "crates/libs/lib-db", features = ["postgres"] }
lib-db = { path = "crates/libs/lib-db", features = ["redis", "postgres"] }

# Default usage (both Redis and PostgreSQL)
lib-db = { path = "crates/libs/lib-db" }
```

### Feature Flags

- `redis` - Enable Redis backend and high-performance in-memory store
- `postgres` - Enable PostgreSQL backend
- `default` - Enable both Redis and PostgreSQL

**Note:** The `redis` feature also includes the `MemoryDb` implementation with DashMap, which provides excellent performance for testing and development.

## Configuration

### Redis Configuration

```rust
use lib_db::redis::{RedisDb, RedisConfig};

let config = RedisConfig {
    max_connections: 50,
    min_connections: 10,
    connection_timeout_ms: 500,
    max_lifetime_secs: 300,
    idle_timeout_secs: 60,
    retry_count: 3,
};

let kv = RedisDb::with_config(config);
let kv = kv.connect("redis://localhost:6379").await?;
```

### Memory Store Configuration

```rust
use lib_db::memory_kv::{MemoryConfig, EvictionPolicy};

let config = MemoryConfig {
    max_entries: 100_000,
    default_ttl: Some(Duration::from_secs(3600)), // 1 hour
    eviction_policy: EvictionPolicy::LRU,
    enable_cleanup: true,
    cleanup_interval: Duration::from_secs(60),
    compression_enabled: false,
    metrics_enabled: true,
};
```

### Connection Strings

#### SQL Databases
- **PostgreSQL**: `postgresql://user:pass@localhost:5432/dbname`
- **MySQL**: `mysql://user:pass@localhost:3306/dbname` (future)
- **SQLite**: `sqlite://path/to/database.db` (future)

#### Key-Value Stores
- **Redis**: `redis://localhost:6379`
- **Memory**: `memory://store_name?param=value`
  - `memory://user_cache` - Basic named store
  - `memory://sessions?max_entries=1000&ttl=3600&eviction=lru` - With configuration
- **DynamoDB**: `dynamodb://us-east-1` (future)
- **Etcd**: `etcd://localhost:2379` (future)

## Performance Documentation

### Memory Store Performance Benefits

The new DashMap-based memory store provides significant performance improvements:

- **Fine-grained Locking**: Each key has its own lock, allowing concurrent access
- **No Read-Write Contention**: Reads don't block other reads
- **High Concurrency**: Multiple operations can happen simultaneously
- **Background Cleanup**: Automatic expiration without blocking operations
- **Multiple Named Stores**: Isolated stores for different data types

### Use Raw Byte Operations

```rust
// Faster - no serialization overhead
let bytes = bincode::serialize(&data).unwrap();
kv.set_bytes("key", &bytes).await?;

// Slower - automatic serialization
kv.set("key", &data).await?;
```

### Use Batch Operations

```rust
// Faster - single network round-trip
kv.mset(&[("key1", &value1), ("key2", &value2)]).await?;
let values = kv.mget(&["key1", "key2"]).await?;

// Slower - multiple round-trips
kv.set("key1", &value1).await?;
kv.set("key2", &value2).await?;
let value1 = kv.get("key1").await?;
let value2 = kv.get("key2").await?;
```

### Memory Store Registry Benefits

```rust
// Separate stores for different purposes
let user_cache = REGISTRY.get_or_create("user_cache").await;
let session_store = REGISTRY.get_or_create("sessions").await;
let product_cache = REGISTRY.get_or_create("products").await;

// Each store can have different configurations
let fast_cache = REGISTRY
    .get_or_create_with_config("fast_cache", fast_config)
    .await;

// Monitor all stores
let registry_stats = REGISTRY.stats().await;
println!("Total stores: {}", registry_stats.store_count);
println!("Total entries: {}", registry_stats.total_entries);
```

## Testing

### Running Examples

```bash
# Run the trait system example
cargo run --example kv_trait

# Run with Redis (requires redis-server)
redis-server &
cargo run --example kv_trait
```

### Running Tests

```bash
cargo test
cargo test --features redis
```

## Adding New Backends

### Adding a New SQL Database

To add support for a new SQL database:

1. **Implement SqlDb** (required):

```rust
use lib_db::traits::SqlDb;

pub struct MySqlDb {
    pool: sqlx::MySqlPool,
}

#[async_trait]
impl SqlDb for MySqlDb {
    type Row = sqlx::mysql::MySqlRow;
    type Arguments<'a> = sqlx::mysql::MySqlArguments;
    type NativePool = sqlx::MySqlPool;
    
    async fn connect(url: &str) -> Result<Self> {
        let pool = sqlx::MySqlPool::connect(url).await?;
        Ok(Self { pool })
    }
    
    async fn execute_with<'a>(&self, sql: &str, args: Self::Arguments<'a>) -> Result<u64> {
        // Implementation
    }
    
    // ... implement other required methods
}
```

2. **Add to lib.rs**:

```rust
pub mod my_sql_db;
```

### Adding a New Key-Value Store

To add support for a new KV store:

1. **Implement BasicKvStore** (required):

```rust
use lib_db::traits::BasicKvStore;

pub struct MyKvStore {
    // Your implementation
}

#[async_trait]
impl BasicKvStore for MyKvStore {
    async fn connect(url: &str) -> Result<Self> {
        // Implementation
    }
    
    async fn set<T>(&self, key: &str, value: &T) -> Result<()>
    where T: Serialize + Send + Sync {
        // Implementation
    }
    
    // ... implement other required methods
}
```

2. **Implement Additional Traits** (optional):

```rust
#[async_trait]
impl AdvancedKvStore for MyKvStore {
    // Implement ping, pool_stats, mget, mset, mdel
}

#[async_trait]
impl AtomicKvStore for MyKvStore {
    // Implement incr, set_nx, cas
}

#[async_trait]
impl ExpiringKvStore for MyKvStore {
    // Implement set_ex, expire, ttl
}
```

3. **Add to lib.rs**:

```rust
pub mod my_kv_store;
```
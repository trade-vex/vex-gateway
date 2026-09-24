use lib_db::{
    error::Result,
    memory_kv::MemoryDb,
    redis::RedisDb,
    traits::{AdvancedKvStore, AtomicKvStore, BasicKvStore, ExpiringKvStore, KvStore},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct User {
    id: u32,
    name: String,
    email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Session {
    user_id: u32,
    token: String,
    created_at: u64,
}

/// Example: Using only BasicKvStore
async fn basic_kv_operations<T: BasicKvStore>(kv: &T) -> Result<()> {
    println!("=== Basic Operations ===");

    // Store a user
    let user = User {
        id: 123,
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
    };

    kv.set("user:123", &user).await?;
    println!("Stored user");

    // Retrieve the user
    let retrieved: Option<User> = kv.get("user:123").await?;
    println!("Retrieved user: {:?}", retrieved);

    // Check if key exists
    let exists = kv.exists("user:123").await?;
    println!("Key exists: {}", exists);

    // Raw byte operations
    let session = Session {
        user_id: 123,
        token: "abc123".to_string(),
        created_at: 1234567890,
    };

    let bytes = serde_json::to_vec(&session).unwrap();
    kv.set_bytes("session:123", &bytes).await?;
    println!("Stored session as raw bytes");

    let retrieved_bytes = kv.get_bytes("session:123").await?;
    if let Some(bytes) = retrieved_bytes {
        let session: Session = serde_json::from_slice(&bytes).unwrap();
        println!("Retrieved session: {:?}", session);
    }

    // Delete the key
    let deleted = kv.delete("user:123").await?;
    println!("Deleted user: {}", deleted);

    Ok(())
}

/// Example: Using AdvancedKvStore features
async fn advanced_operations<T: AdvancedKvStore>(kv: &T) -> Result<()> {
    println!("=== Advanced Operations ===");

    // Health check
    kv.ping().await?;
    println!("Ping successful");

    // Pool statistics
    let stats = kv.pool_stats();
    println!("Pool stats: {:?}", stats);

    // Batch operations
    let user1 = User {
        id: 1,
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
    };
    let user2 = User {
        id: 2,
        name: "Bob".to_string(),
        email: "bob@example.com".to_string(),
    };
    let user3 = User {
        id: 3,
        name: "Charlie".to_string(),
        email: "charlie@example.com".to_string(),
    };

    let users = vec![("user:1", &user1), ("user:2", &user2), ("user:3", &user3)];

    kv.mset(&users).await?;
    println!("Batch set successful");

    let keys = vec!["user:1", "user:2", "user:3"];
    let retrieved: Vec<Option<User>> = kv.mget(&keys).await?;
    println!("Batch get successful: {:?}", retrieved);

    let deleted = kv.mdel(&keys).await?;
    println!("Batch delete successful: {} keys deleted", deleted);

    Ok(())
}

/// Example: Using AtomicKvStore features
async fn atomic_operations<T: AtomicKvStore>(kv: &T) -> Result<()> {
    println!("=== Atomic Operations ===");

    // Atomic increment
    let new_value = kv.incr("counter", 1).await?;
    println!("Incremented counter to: {}", new_value);

    let new_value = kv.incr("counter", 5).await?;
    println!("Incremented counter to: {}", new_value);

    // Set if not exists
    let lock_data = "locked".to_string();
    let was_set = kv.set_nx("lock:resource", &lock_data).await?;
    println!("Set NX result: {}", was_set);

    // Try to set again (should fail)
    let was_set = kv
        .set_nx("lock:resource", &"another_lock".to_string())
        .await?;
    println!("Second Set NX result: {}", was_set);

    // Compare and swap
    let old_value = "old".to_string();
    let new_value = "new".to_string();

    // First, set the initial value
    kv.set("cas_key", &old_value).await?;

    // Try CAS with correct old value
    let swapped = kv.cas("cas_key", &old_value, &new_value).await?;
    println!("CAS with correct old value: {}", swapped);

    // Try CAS with wrong old value
    let swapped = kv
        .cas(
            "cas_key",
            &"wrong_old".to_string(),
            &"another_new".to_string(),
        )
        .await?;
    println!("CAS with wrong old value: {}", swapped);

    Ok(())
}

/// Example: Using ExpiringKvStore features (not all stores support this)
async fn expiring_operations<T: ExpiringKvStore>(kv: &T) -> Result<()> {
    println!("=== Expiring Operations ===");

    let session = Session {
        user_id: 123,
        token: "abc123".to_string(),
        created_at: 1234567890,
    };

    // Set with expiration (1 hour)
    kv.set_ex("session:123", &session, 3600).await?;
    println!("Set session with 1 hour expiration");

    // Check TTL
    let ttl = kv.ttl("session:123").await?;
    println!("TTL: {:?}", ttl);

    // Set expiration on existing key
    kv.set("temp_key", &"temporary".to_string()).await?;
    let expired = kv.expire("temp_key", 60).await?; // 1 minute
    println!("Set expiration on existing key: {}", expired);

    Ok(())
}

/// Example: Using full KvStore
async fn full_featured_operations<T: KvStore>(kv: &T) -> Result<()> {
    println!("=== Full Featured Operations ===");

    // All operations are available
    kv.set("full_key", &"full_value".to_string()).await?;
    kv.set_ex("expiring_key", &"expiring_value".to_string(), 300)
        .await?;
    kv.incr("counter", 1).await?;
    kv.ping().await?;

    println!("All operations successful");

    Ok(())
}

/// Example: Generic function that works with any BasicKvStore
async fn generic_cache_operations<T: BasicKvStore>(kv: &T) -> Result<()> {
    println!("=== Generic Cache Operations ===");

    // This function works with ANY KV store that implements BasicKvStore
    let cache_data = vec![1, 2, 3, 4, 5];
    kv.set("cache:data", &cache_data).await?;

    let retrieved: Option<Vec<i32>> = kv.get("cache:data").await?;
    println!("Retrieved cached data: {:?}", retrieved);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Key-Value Store Trait System Examples");
    println!("=====================================\n");

    // Test with memory store
    let memory_kv = MemoryDb::connect("memory://").await?;
    basic_kv_operations(&memory_kv).await?;
    generic_cache_operations(&memory_kv).await?;

    // Test with Redis if available
    if let Ok(redis_kv) = RedisDb::connect("redis://localhost:6379").await {
        println!("\n{}", "=".repeat(50));
        basic_kv_operations(&redis_kv).await?;
        advanced_operations(&redis_kv).await?;
        atomic_operations(&redis_kv).await?;
        expiring_operations(&redis_kv).await?;
        full_featured_operations(&redis_kv).await?;
        generic_cache_operations(&redis_kv).await?;
    } else {
        println!("\n Redis not available - run with: redis-server");
    }

    println!("\n All examples completed successfully!");
    Ok(())
}

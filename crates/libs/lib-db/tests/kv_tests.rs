use lib_db::{
    error::Result,
    memory_kv::MemoryDb,
    redis::RedisDb,
    traits::{BasicKvStore, AdvancedKvStore, AtomicKvStore, ExpiringKvStore, KvStore},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct TestUser {
    id: u32,
    name: String,
    email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct TestSession {
    user_id: u32,
    token: String,
    created_at: u64,
}

/// Test helper function for basic KV operations
async fn test_basic_operations<T: BasicKvStore>(kv: &T) -> Result<()> {
    // Test set and get
    let user = TestUser {
        id: 123,
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
    };
    
    kv.set("user:123", &user).await?;
    
    let retrieved: Option<TestUser> = kv.get("user:123").await?;
    assert_eq!(retrieved, Some(user));
    
    // Test exists
    let exists = kv.exists("user:123").await?;
    assert!(exists);
    
    let not_exists = kv.exists("user:999").await?;
    assert!(!not_exists);
    
    // Test raw byte operations
    let session = TestSession {
        user_id: 123,
        token: "abc123".to_string(),
        created_at: 1234567890,
    };
    
    let bytes = bincode::serialize(&session).unwrap();
    kv.set_bytes("session:123", &bytes).await?;
    
    let retrieved_bytes = kv.get_bytes("session:123").await?;
    assert!(retrieved_bytes.is_some());
    
    if let Some(bytes) = retrieved_bytes {
        let session: TestSession = bincode::deserialize(&bytes).unwrap();
        assert_eq!(session.user_id, 123);
        assert_eq!(session.token, "abc123");
    }
    
    // Test delete
    let deleted = kv.delete("user:123").await?;
    assert!(deleted);
    
    let retrieved_after_delete: Option<TestUser> = kv.get("user:123").await?;
    assert_eq!(retrieved_after_delete, None);
    
    Ok(())
}

/// Test helper function for advanced KV operations
async fn test_advanced_operations<T: AdvancedKvStore>(kv: &T) -> Result<()> {
    // Test ping
    kv.ping().await?;
    
    // Test pool stats
    let stats = kv.pool_stats();
    assert!(stats.size >= 0);
    assert!(stats.idle >= 0);
    assert!(stats.active >= 0);
    assert!(stats.max_size > 0);
    
    // Test batch operations
    let user1 = TestUser { id: 1, name: "Alice".to_string(), email: "alice@example.com".to_string() };
    let user2 = TestUser { id: 2, name: "Bob".to_string(), email: "bob@example.com".to_string() };
    let user3 = TestUser { id: 3, name: "Charlie".to_string(), email: "charlie@example.com".to_string() };
    
    let users = vec![
        ("user:1", &user1),
        ("user:2", &user2),
        ("user:3", &user3),
    ];
    
    kv.mset(&users).await?;
    
    let keys = vec!["user:1", "user:2", "user:3"];
    let retrieved: Vec<Option<TestUser>> = kv.mget(&keys).await?;
    
    assert_eq!(retrieved.len(), 3);
    assert_eq!(retrieved[0], Some(user1));
    assert_eq!(retrieved[1], Some(user2));
    assert_eq!(retrieved[2], Some(user3));
    
    let deleted = kv.mdel(&keys).await?;
    assert_eq!(deleted, 3);
    
    Ok(())
}

/// Test helper function for atomic KV operations
async fn test_atomic_operations<T: AtomicKvStore>(kv: &T) -> Result<()> {
    // Test atomic increment with unique key
    let unique_key = format!("counter:{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let new_value = kv.incr(&unique_key, 1).await?;
    assert_eq!(new_value, 1);
    
    let new_value = kv.incr(&unique_key, 5).await?;
    assert_eq!(new_value, 6);
    
    // Test set if not exists with unique key
    let unique_lock_key = format!("lock:resource:{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let lock_data = "locked".to_string();
    let was_set = kv.set_nx(&unique_lock_key, &lock_data).await?;
    assert!(was_set);
    
    // Try to set again (should fail)
    let was_set = kv.set_nx(&unique_lock_key, &"another_lock".to_string()).await?;
    assert!(!was_set);
    
    // Test compare and swap with unique key
    let unique_cas_key = format!("cas_key:{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let old_value = "old".to_string();
    let new_value = "new".to_string();
    
    // First, set the initial value
    kv.set(&unique_cas_key, &old_value).await?;
    
    // Try CAS with correct old value
    let swapped = kv.cas(&unique_cas_key, &old_value, &new_value).await?;
    assert!(swapped);
    
    // Try CAS with wrong old value
    let swapped = kv.cas(&unique_cas_key, &"wrong_old".to_string(), &"another_new".to_string()).await?;
    assert!(!swapped);
    
    Ok(())
}

/// Test helper function for expiring KV operations
async fn test_expiring_operations<T: ExpiringKvStore>(kv: &T) -> Result<()> {
    let session = TestSession {
        user_id: 123,
        token: "abc123".to_string(),
        created_at: 1234567890,
    };
    
    // Test set with expiration
    kv.set_ex("session:123", &session, 3600).await?;
    
    // Test TTL
    let ttl = kv.ttl("session:123").await?;
    assert!(ttl.is_some());
    if let Some(ttl_value) = ttl {
        assert!(ttl_value > 0);
        assert!(ttl_value <= 3600);
    }
    
    // Test set expiration on existing key
    kv.set("temp_key", &"temporary".to_string()).await?;
    let expired = kv.expire("temp_key", 60).await?;
    assert!(expired);
    
    Ok(())
}

/// Test helper function for full KV operations
async fn test_full_operations<T: KvStore>(kv: &T) -> Result<()> {
    // Test all operations are available
    kv.set("full_key", &"full_value".to_string()).await?;
    kv.set_ex("expiring_key", &"expiring_value".to_string(), 300).await?;
    kv.incr("counter", 1).await?;
    kv.ping().await?;
    
    Ok(())
}

#[tokio::test]
async fn test_memory_kv_basic_operations() -> Result<()> {
    let kv = MemoryDb::connect("memory://").await?;
    test_basic_operations(&kv).await
}

#[tokio::test]
async fn test_memory_kv_advanced_operations() -> Result<()> {
    let kv = MemoryDb::connect("memory://").await?;
    test_advanced_operations(&kv).await
}

#[tokio::test]
async fn test_memory_kv_atomic_operations() -> Result<()> {
    let kv = MemoryDb::connect("memory://").await?;
    test_atomic_operations(&kv).await
}

#[tokio::test]
async fn test_memory_kv_expiring_operations() -> Result<()> {
    let kv = MemoryDb::connect("memory://").await?;
    test_expiring_operations(&kv).await
}

#[tokio::test]
async fn test_memory_kv_full_operations() -> Result<()> {
    let kv = MemoryDb::connect("memory://").await?;
    test_full_operations(&kv).await
}

#[tokio::test]
async fn test_memory_kv_generic_operations() -> Result<()> {
    let kv = MemoryDb::connect("memory://").await?;
    
    // Test that generic functions work with any BasicKvStore
    let cache_data = vec![1, 2, 3, 4, 5];
    kv.set("cache:data", &cache_data).await?;
    
    let retrieved: Option<Vec<i32>> = kv.get("cache:data").await?;
    assert_eq!(retrieved, Some(cache_data));
    
    Ok(())
}

#[tokio::test]
async fn test_memory_kv_error_handling() -> Result<()> {
    let kv = MemoryDb::connect("memory://").await?;
    
    // Test getting non-existent key
    let result: Option<String> = kv.get("non_existent").await?;
    assert_eq!(result, None);
    
    // Test deleting non-existent key
    let deleted = kv.delete("non_existent").await?;
    assert!(!deleted);
    
    Ok(())
}

// Redis tests (only run if Redis is available)
#[cfg(feature = "redis")]
mod redis_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_redis_kv_basic_operations() -> Result<()> {
        if let Ok(kv) = RedisDb::connect("redis://localhost:6379").await {
            test_basic_operations(&kv).await
        } else {
            println!("Redis not available - skipping test");
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_redis_kv_advanced_operations() -> Result<()> {
        if let Ok(kv) = RedisDb::connect("redis://localhost:6379").await {
            test_advanced_operations(&kv).await
        } else {
            println!("Redis not available - skipping test");
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_redis_kv_atomic_operations() -> Result<()> {
        if let Ok(kv) = RedisDb::connect("redis://localhost:6379").await {
            test_atomic_operations(&kv).await
        } else {
            println!("Redis not available - skipping test");
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_redis_kv_expiring_operations() -> Result<()> {
        if let Ok(kv) = RedisDb::connect("redis://localhost:6379").await {
            test_expiring_operations(&kv).await
        } else {
            println!("Redis not available - skipping test");
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_redis_kv_full_operations() -> Result<()> {
        if let Ok(kv) = RedisDb::connect("redis://localhost:6379").await {
            test_full_operations(&kv).await
        } else {
            println!("Redis not available - skipping test");
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_redis_kv_generic_operations() -> Result<()> {
        if let Ok(kv) = RedisDb::connect("redis://localhost:6379").await {
            // Test that generic functions work with any BasicKvStore
            let cache_data = vec![1, 2, 3, 4, 5];
            kv.set("cache:data", &cache_data).await?;
            
            let retrieved: Option<Vec<i32>> = kv.get("cache:data").await?;
            assert_eq!(retrieved, Some(cache_data));
            
            Ok(())
        } else {
            println!("Redis not available - skipping test");
            Ok(())
        }
    }
}

#[tokio::test]
async fn test_kv_trait_compatibility() -> Result<()> {
    // Test that different KV stores can be used interchangeably
    let memory_kv = MemoryDb::connect("memory://").await?;
    
    // Test with BasicKvStore trait
    test_basic_operations(&memory_kv).await?;
    
    // Test with KvStore trait (should include all capabilities)
    test_full_operations(&memory_kv).await?;
    
    Ok(())
}

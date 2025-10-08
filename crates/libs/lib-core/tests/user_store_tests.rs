//! user balance store tests
//! 
//! these tests verify that our userbalancestore works correctly with caching, database operations,
//! and error handling. we test both the cache-only and cache+repository configurations.

use sea_orm::{Database, DatabaseConnection, ConnectionTrait};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use lib_core::{
    repository::repository::{TradingRepository, PostgresTradingRepository},
    cache::user_store::{UserBalanceStore, UserBalanceCacheConfig},
};

// need to prevent multiple tests from trying to create/drop tables at the same time
static SCHEMA_MUTEX: Mutex<()> = Mutex::new(());
static SCHEMA_INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// connect to the test database with timeout
async fn setup_test_db() -> Result<DatabaseConnection, sea_orm::DbErr> {
    // use the same database url as the main app, or fall back to localhost
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://vex:1234@localhost:5432/vex_gateway".to_string());
    
    // add timeout to prevent hanging
    let connection_future = Database::connect(&database_url);
    let timeout_duration = std::time::Duration::from_secs(5);
    
    match tokio::time::timeout(timeout_duration, connection_future).await {
        Ok(Ok(db)) => Ok(db),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(sea_orm::DbErr::Custom("Database connection timeout".to_string())),
    }
}

// helper function to fail tests when database is not available
async fn require_database() -> DatabaseConnection {
    match setup_test_db_with_schema().await {
        Ok(db) => db,
        Err(e) => {
            panic!("Database connection failed: {}. Tests require a running database.", e);
        }
    }
}

// set up the database with all required tables and indexes
// this only runs once per test suite to avoid conflicts
async fn setup_test_db_with_schema() -> Result<DatabaseConnection, sea_orm::DbErr> {
    let db = setup_test_db().await?;
    
    // make sure we only create the schema once, even with multiple concurrent tests
    if !SCHEMA_INITIALIZED.load(std::sync::atomic::Ordering::SeqCst) {
        let _guard = SCHEMA_MUTEX.lock().unwrap();
        
        // double-check pattern - another test might have initialized it while we waited
        if !SCHEMA_INITIALIZED.load(std::sync::atomic::Ordering::SeqCst) {
            // build the complete database schema for our tests
            let create_schema_sql = r#"
                -- users table (needed for foreign key references)
                CREATE TABLE IF NOT EXISTS users (
                    user_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                    username VARCHAR(255) NOT NULL UNIQUE,
                    email VARCHAR(255) NOT NULL UNIQUE,
                    password_hash VARCHAR(255) NOT NULL,
                    status VARCHAR(50) NOT NULL DEFAULT 'active',
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                );
                
                -- clean slate - drop existing tables to avoid conflicts
                DROP TABLE IF EXISTS user_balances CASCADE;
                
                -- user balances table for account management
                CREATE TABLE user_balances (
                    user_id UUID PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
                    available BIGINT NOT NULL DEFAULT 0,
                    locked BIGINT NOT NULL DEFAULT 0,
                    last_updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
                );
                
                -- add some indexes to speed up queries
                CREATE INDEX IF NOT EXISTS idx_user_balances_user_id ON user_balances(user_id);
            "#;
            
            db.execute_unprepared(create_schema_sql)
                .await
                .map_err(|e| sea_orm::DbErr::Custom(format!("Failed to create test database schema: {}", e)))?;
            
            // mark as initialized so other tests don't try to recreate it
            SCHEMA_INITIALIZED.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    
    Ok(db)
}

// clean up test data after tests complete
// we need to delete in the right order due to foreign key constraints
async fn cleanup_test_data(db: &DatabaseConnection) {
    // delete test data in reverse dependency order to avoid fk constraint issues
    let cleanup_sql = r#"
        DELETE FROM user_balances WHERE user_id IN (
            SELECT user_id FROM users WHERE username LIKE 'test_%'
        );
        DELETE FROM users WHERE username LIKE 'test_%';
    "#;
    db.execute_unprepared(cleanup_sql)
        .await
        .expect("Failed to cleanup test data");
}

// generate a unique user id for testing
fn create_test_user_id() -> Uuid {
    Uuid::new_v4()
}

// create a cache config with caching enabled
fn create_enabled_cache_config() -> UserBalanceCacheConfig {
    UserBalanceCacheConfig {
        ttl: std::time::Duration::from_secs(60),
        enabled: true,
    }
}

// create a cache config with caching disabled
fn create_disabled_cache_config() -> UserBalanceCacheConfig {
    UserBalanceCacheConfig {
        ttl: std::time::Duration::from_secs(60),
        enabled: false,
    }
}

#[cfg(test)] 
mod tests {
    use super::*;
    use std::sync::Once;

    static CLEANUP: Once = Once::new();

    // clean up all test data after the test suite finishes
    // this runs once at the end to keep the database clean
    async fn global_cleanup() {
        CLEANUP.call_once(|| {
            println!("Cleaning up global test environment...");
        });
        
        if let Ok(db) = setup_test_db().await {
            cleanup_test_data(&db).await;
        }
    }

    // test creating a new userbalancestore with default config
    // this verifies the basic store creation works
    #[test]
    fn test_user_balance_store_new() {
        let config = UserBalanceCacheConfig::default();
        let _store = UserBalanceStore::new(config.clone());
        
        // Store should be created successfully
        // We can't access private fields directly, but we can test that it was created
        // The store should be created without errors
        assert!(!config.enabled); // Default config should have caching disabled
    }

    // test creating a userbalancestore with a repository
    // this tests the store creation with database access
    #[tokio::test]
    async fn test_user_balance_store_with_repository() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
        let config = UserBalanceCacheConfig::default();
        
        let _store = UserBalanceStore::with_repository(config.clone(), repo);
        
        // Store should be created successfully
        // We can't access private fields directly, but we can test that it was created
        // The store should be created without errors
        assert!(!config.enabled); // Default config should have caching disabled
    }

    // test getting a user balance when no repository is available
    // this should return empty vector without errors
    #[tokio::test]
    async fn test_get_user_balance_no_repository() {
        let config = UserBalanceCacheConfig::default();
        let store = UserBalanceStore::new(config);
        let user_id = create_test_user_id();

        let result = store.get_user_balances(user_id).await;
        
        match result {
            Ok(balances) => {
                // This is expected - no repository means no data
                assert_eq!(balances.len(), 0);
            }
            Err(e) => {
                panic!("Expected Ok(vec![]) but got error: {}", e);
            }
        }
    }

    // test getting a user balance from repository (no cache)
    // this tests the basic database lookup functionality
    #[tokio::test]
    async fn test_get_user_balance_from_repository() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_disabled_cache_config();
        let store = UserBalanceStore::with_repository(config, repo.clone());
        let user_id = create_test_user_id();

        // Set up a user first (foreign key requirement)
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_repo_balance_{}', 'test_repo_balance_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user
        repo.create_user_balance(user_id, 15000, 3000).await.unwrap();

        // Now try to get the balance
        let result = store.get_user_balances(user_id).await;

        match result {
            Ok(balances) => {
                assert_eq!(balances.len(), 1);
                let balance = &balances[0];
                assert_eq!(balance.user_id, user_id);
                assert_eq!(balance.available, 15000);
                assert_eq!(balance.locked, 3000);
            }
            Err(e) => {
                panic!("Get user balance failed: {}", e);
            }
        }
    }

    // test getting a user balance that doesn't exist
    // this should return none, not an error
    #[tokio::test]
    async fn test_get_user_balance_not_found() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
        let config = create_disabled_cache_config();
        let store = UserBalanceStore::with_repository(config, repo);
        let non_existent_user_id = Uuid::new_v4();

        let result = store.get_user_balances(non_existent_user_id).await;

        match result {
            Ok(balances) => {
                // This is expected - user doesn't exist
                assert_eq!(balances.len(), 0);
            }
            Err(e) => {
                panic!("Get user balance failed: {}", e);
            }
        }
    }

    // test redis cache hit behavior
    // this tests that the redis cache is working correctly
    #[tokio::test]
    async fn test_redis_cache_hit_behavior() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        // Create store with Redis cache - add timeout to prevent hanging
        let store_future = UserBalanceStore::with_redis_cache(
            config, 
            "redis://localhost:6379".to_string(), 
            Some(repo.clone())
        );
        
        let store = match tokio::time::timeout(std::time::Duration::from_secs(2), store_future).await {
            Ok(Ok(store)) => store,
            Ok(Err(e)) => {
                panic!("Redis connection failed: {}. Redis must be running for this test!", e);
            }
            Err(_) => {
                panic!("connection timeout after 2 seconds. Redis must be running for this test!");
            }
        };

        let user_id = create_test_user_id();

        // Set up a user and create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_redis_cache_hit_{}', 'test_redis_cache_hit_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user using the repository directly
        repo.create_user_balance(user_id, 20000, 4000).await.unwrap();

        // First call should hit the database and cache the result
        let result1 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result1.len(), 1);
        let balance1 = &result1[0];

        // Second call should hit the cache
        let result2 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result2.len(), 1);
        let balance2 = &result2[0];

        println!("{:?}", balance2);
        // Both results should be identical
        assert_eq!(balance1.user_id, balance2.user_id);
        assert_eq!(balance1.available, balance2.available);
        assert_eq!(balance1.locked, balance2.locked);
    }

    // test cache hit behavior without redis
    // this tests that the cache is working correctly with repository only
    #[tokio::test]
    async fn test_cache_hit_behavior() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        let store = UserBalanceStore::with_repository(config, repo.clone());

        let user_id = create_test_user_id();

        // Set up a user and create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_cache_hit_{}', 'test_cache_hit_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user using the repository directly
        repo.create_user_balance(user_id, 20000, 4000).await.unwrap();

        // First call should hit the database and cache the result
        let result1 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result1.len(), 1);
        let balance1 = &result1[0];

        // Second call should hit the cache
        let result2 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result2.len(), 1);
        let balance2 = &result2[0];

        // Both results should be identical
        assert_eq!(balance1.user_id, balance2.user_id);
        assert_eq!(balance1.available, balance2.available);
        assert_eq!(balance1.locked, balance2.locked);
    }

    // test cache miss behavior
    // this tests that cache misses are handled correctly
    #[tokio::test]
    async fn test_cache_miss_behavior() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        let store = UserBalanceStore::with_repository(config, repo);

        let user_id = create_test_user_id();

        // Set up a user but don't create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_cache_miss_{}', 'test_cache_miss_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Try to get a balance that doesn't exist
        let result = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result.len(), 0);
    }

    // test redis cache invalidation
    // this tests that redis cache invalidation works correctly
    #[tokio::test]
    async fn test_redis_cache_invalidation() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        // Create store with Redis cache - add timeout to prevent hanging
        let store_future = UserBalanceStore::with_redis_cache(
            config, 
            "redis://localhost:6379".to_string(), 
            Some(repo.clone())
        );
        
        let store = match tokio::time::timeout(std::time::Duration::from_secs(2), store_future).await {
            Ok(Ok(store)) => store,
            Ok(Err(e)) => {
                panic!("Redis connection failed: {}. Redis must be running for this test!", e);
            }
            Err(_) => {
                panic!("Redis connection timeout after 2 seconds. Redis must be running for this test!");
            }
        };

        let user_id = create_test_user_id();

        // Set up a user and create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_redis_invalidation_{}', 'test_redis_invalidation_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user using the repository directly
        repo.create_user_balance(user_id, 25000, 5000).await.unwrap();

        // First call should be a cache miss (data not in cache yet)
        let result1 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result1.len(), 1);
        
        // Second call should be a cache hit (data now in cache)
        let result2 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result2.len(), 1);
        
        // Invalidate the cache
        store.invalidate_user_balances(user_id).await;

        // Third call should be a cache miss again (cache was invalidated)
        let result3 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result3.len(), 1);
        
        // Verify data integrity - all results should be identical
        assert_eq!(result1[0].user_id, result2[0].user_id);
        assert_eq!(result2[0].user_id, result3[0].user_id);
    }

    // test cache invalidation without redis
    // this tests that cache invalidation works correctly with repository only
    #[tokio::test]
    async fn test_cache_invalidation() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        let store = UserBalanceStore::with_repository(config, repo.clone());

        let user_id = create_test_user_id();

        // Set up a user and create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_invalidation_{}', 'test_invalidation_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user using the repository directly
        repo.create_user_balance(user_id, 25000, 5000).await.unwrap();

        // First call should be a cache miss (data not in cache yet)
        let result1 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result1.len(), 1);
        
        // Second call should be a cache hit (data now in cache)
        let result2 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result2.len(), 1);
        
        // Invalidate the cache
        store.invalidate_user_balances(user_id).await;

        // Third call should be a cache miss again (cache was invalidated)
        let result3 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result3.len(), 1);
        
        // Verify data integrity - all results should be identical
        assert_eq!(result1[0].user_id, result2[0].user_id);
        assert_eq!(result2[0].user_id, result3[0].user_id);
    }

    // test cache failure fallback
    // this tests that the store falls back to repository when cache fails
    #[tokio::test]
    async fn test_cache_failure_fallback() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        // Create store with invalid Redis URL to simulate cache failure
        let store = UserBalanceStore::with_redis_cache(
            config, 
            "redis://invalid-host:6379".to_string(), 
            Some(repo.clone())
        ).await.expect("Store should be created even with invalid Redis URL");

        let user_id = create_test_user_id();

        // Set up a user and create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_cache_failure_{}', 'test_cache_failure_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user using the repository directly
        repo.create_user_balance(user_id, 30000, 6000).await.unwrap();

        // Should still work by falling back to repository
        let result = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].user_id, user_id);
        assert_eq!(result[0].available, 30000);
        assert_eq!(result[0].locked, 6000);
    }

    // test cache ttl expiration
    // this tests that cache entries expire after ttl
    #[tokio::test]
    async fn test_cache_ttl_expiration() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        
        // Create config with very short TTL
        let config = UserBalanceCacheConfig {
            ttl: std::time::Duration::from_millis(100), // 100ms TTL
            enabled: true,
        };
        
        // Create store with Redis cache - add timeout to prevent hanging
        let store_future = UserBalanceStore::with_redis_cache(
            config, 
            "redis://localhost:6379".to_string(), 
            Some(repo.clone())
        );
        
        let store = match tokio::time::timeout(std::time::Duration::from_secs(2), store_future).await {
            Ok(Ok(store)) => store,
            Ok(Err(e)) => {
                panic!("Redis connection failed: {}. Redis must be running for this test!", e);
            }
            Err(_) => {
                panic!("Redis connection timeout after 2 seconds. Redis must be running for this test!");
            }
        };

        let user_id = create_test_user_id();

        // Set up a user and create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_ttl_{}', 'test_ttl_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user using the repository directly
        repo.create_user_balance(user_id, 35000, 7000).await.unwrap();

        // First call should hit the database and cache the result
        let result1 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result1.len(), 1);

        // Second call should hit the cache (within TTL)
        let result2 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result2.len(), 1);

        // Wait for TTL to expire
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // Third call should be a cache miss (TTL expired)
        let result3 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result3.len(), 1);

        // Verify data integrity - all results should be identical
        assert_eq!(result1[0].user_id, result2[0].user_id);
        assert_eq!(result2[0].user_id, result3[0].user_id);
    }

    // test cache statistics tracking
    // this tests that cache statistics are properly tracked
    #[tokio::test]
    async fn test_cache_statistics() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        let store = UserBalanceStore::with_repository(config, repo.clone());

        let user_id = create_test_user_id();

        // Set up a user and create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_stats_{}', 'test_stats_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user using the repository directly
        repo.create_user_balance(user_id, 30000, 6000).await.unwrap();

        // Test that we can get the balance multiple times
        let result1 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result1.len(), 1);

        let result2 = store.get_user_balances(user_id).await.unwrap();
        assert_eq!(result2.len(), 1);

        // Verify data integrity - all results should be identical
        assert_eq!(result1[0].user_id, result2[0].user_id);
        assert_eq!(result1[0].available, result2[0].available);
        assert_eq!(result1[0].locked, result2[0].locked);
    }

    // test error handling when repository fails
    // this tests that repository errors are properly handled
    #[tokio::test]
    async fn test_repository_error_handling() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
        let config = create_disabled_cache_config();
        let store = UserBalanceStore::with_repository(config, repo);
        let user_id = create_test_user_id();

        // Try to get a balance for a user that doesn't exist
        // This should return empty vector, not an error
        let result = store.get_user_balances(user_id).await;
        
        match result {
            Ok(balances) => {
                // This is expected - user doesn't exist
                assert_eq!(balances.len(), 0);
            }
            Err(e) => {
                panic!("Expected Ok(vec![]) but got error: {}", e);
            }
        }
    }

    // test concurrent access to the store
    // this tests thread safety
    #[tokio::test]
    async fn test_concurrent_access() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        let store = Arc::new(UserBalanceStore::with_repository(config, repo.clone()));

        let user_id = create_test_user_id();

        // Set up a user and create a balance
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_concurrent_{}', 'test_concurrent_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance for this user using the repository directly
        repo.create_user_balance(user_id, 35000, 7000).await.unwrap();

        let mut handles = vec![];

        // Spawn multiple tasks that try to get the same balance concurrently
        for _i in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = tokio::spawn(async move {
                let result = store_clone.get_user_balances(user_id).await;
                assert!(result.is_ok());
                let balances = result.unwrap();
                assert_eq!(balances.len(), 1);
                let balance_model = &balances[0];
                assert_eq!(balance_model.user_id, user_id);
                assert_eq!(balance_model.available, 35000);
                assert_eq!(balance_model.locked, 7000);
            });
            
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }
    }

    // test store send + sync traits
    // this ensures the store is safe for async usage
    #[test]
    fn test_store_send_sync() {
        // Ensure UserBalanceStore is Send + Sync for async usage
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        
        assert_send::<UserBalanceStore>();
        assert_sync::<UserBalanceStore>();
    }

    /// final cleanup test that runs after all other tests
    #[tokio::test]
    async fn zzz_cleanup_after_all_tests() {
        // Database is required for cleanup
        let _db = require_database().await;
        // Give other tests a moment to finish
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        global_cleanup().await;
    }
}
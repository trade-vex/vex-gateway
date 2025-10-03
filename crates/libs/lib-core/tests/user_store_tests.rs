// //! User Balance Store Tests
// //! 
// //! These tests verify that our UserBalanceStore works correctly with caching, database operations,
// //! and error handling. We test both the cache-only and cache+repository configurations.

// use sea_orm::{Database, DatabaseConnection, ConnectionTrait};
// use std::sync::{Arc, Mutex};
// use uuid::Uuid;

// use lib_trading::{
//     repository::{TradingRepository, PostgresTradingRepository},
//     user_store::{UserBalanceStore, UserBalanceCacheConfig},
//     redis_cache::RedisCacheConfig,
// };

// // Need to prevent multiple tests from trying to create/drop tables at the same time
// static SCHEMA_MUTEX: Mutex<()> = Mutex::new(());
// static SCHEMA_INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// /// Connect to the test database
// async fn setup_test_db() -> DatabaseConnection {
//     // Use the same database URL as the main app, or fall back to localhost
//     let database_url = std::env::var("DATABASE_URL")
//         .unwrap_or_else(|_| "postgresql://vex:1234@localhost:5432/vex_gateway".to_string());
    
//     Database::connect(&database_url)
//         .await
//         .expect("Failed to connect to test database")
// }

// /// Set up the database with all required tables and indexes
// /// This only runs once per test suite to avoid conflicts
// async fn setup_test_db_with_schema() -> DatabaseConnection {
//     let db = setup_test_db().await;
    
//     // Make sure we only create the schema once, even with multiple concurrent tests
//     if !SCHEMA_INITIALIZED.load(std::sync::atomic::Ordering::SeqCst) {
//         let _guard = SCHEMA_MUTEX.lock().unwrap();
        
//         // Double-check pattern - another test might have initialized it while we waited
//         if !SCHEMA_INITIALIZED.load(std::sync::atomic::Ordering::SeqCst) {
//             // Build the complete database schema for our tests
//             let create_schema_sql = r#"
//                 -- Users table (needed for foreign key references)
//                 CREATE TABLE IF NOT EXISTS users (
//                     user_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
//                     username VARCHAR(255) NOT NULL UNIQUE,
//                     email VARCHAR(255) NOT NULL UNIQUE,
//                     password_hash VARCHAR(255) NOT NULL,
//                     status VARCHAR(50) NOT NULL DEFAULT 'active',
//                     created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
//                     updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
//                 );
                
//                 -- Clean slate - drop existing tables to avoid conflicts
//                 DROP TABLE IF EXISTS user_balances CASCADE;
                
//                 -- User balances table for account management
//                 CREATE TABLE user_balances (
//                     user_id UUID PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
//                     available BIGINT NOT NULL DEFAULT 0,
//                     locked BIGINT NOT NULL DEFAULT 0,
//                     last_updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
//                 );
                
//                 -- Add some indexes to speed up queries
//                 CREATE INDEX IF NOT EXISTS idx_user_balances_user_id ON user_balances(user_id);
//             "#;
            
//             db.execute_unprepared(create_schema_sql)
//                 .await
//                 .expect("Failed to create test database schema");
            
//             // Mark as initialized so other tests don't try to recreate it
//             SCHEMA_INITIALIZED.store(true, std::sync::atomic::Ordering::SeqCst);
//         }
//     }
    
//     db
// }

// /// Clean up test data after tests complete
// /// We need to delete in the right order due to foreign key constraints
// async fn cleanup_test_data(db: &DatabaseConnection) {
//     // Delete test data in reverse dependency order to avoid FK constraint issues
//     let cleanup_sql = r#"
//         DELETE FROM user_balances WHERE user_id IN (
//             SELECT user_id FROM users WHERE username LIKE 'test_%'
//         );
//         DELETE FROM users WHERE username LIKE 'test_%';
//     "#;
//     db.execute_unprepared(cleanup_sql)
//         .await
//         .expect("Failed to cleanup test data");
// }

// /// Generate a unique user ID for testing
// fn create_test_user_id() -> Uuid {
//     Uuid::new_v4()
// }

// /// Create a Redis config for testing
// fn create_test_redis_config() -> RedisCacheConfig {
//     RedisCacheConfig {
//         url: "redis://localhost:6379".to_string(),
//         key_prefix: "test_user_balance:".to_string(),
//         max_connections: 5,
//         min_connections: 1,
//         connection_timeout_ms: 1000,
//         max_lifetime_secs: 300,
//         idle_timeout_secs: 60,
//         retry_count: 3,
//     }
// }

// /// Create a cache config with caching enabled
// fn create_enabled_cache_config() -> UserBalanceCacheConfig {
//     UserBalanceCacheConfig {
//         ttl: std::time::Duration::from_secs(60),
//         enabled: true,
//     }
// }

// /// Create a cache config with caching disabled
// fn create_disabled_cache_config() -> UserBalanceCacheConfig {
//     UserBalanceCacheConfig {
//         ttl: std::time::Duration::from_secs(60),
//         enabled: false,
//     }
// }

// #[cfg(test)] 
// mod tests {
//     use super::*;
//     use std::sync::Once;

//     static CLEANUP: Once = Once::new();

//     /// Clean up all test data after the test suite finishes
//     /// This runs once at the end to keep the database clean
//     async fn global_cleanup() {
//         CLEANUP.call_once(|| {
//             println!("Cleaning up global test environment...");
//         });
        
//         let db = setup_test_db().await;
//         cleanup_test_data(&db).await;
//     }

//     /// Test creating a new UserBalanceStore with default config
//     /// This verifies the basic store creation works
//     #[test]
//     fn test_user_balance_store_new() {
//         let config = UserBalanceCacheConfig::default();
//         let store = UserBalanceStore::new(config);
        
//         // Store should be created successfully
//         // We can't access private fields directly, but we can test that it was created
//         assert_eq!(store.get_stats().hits, 0);
//         assert_eq!(store.get_stats().misses, 0);
//     }

//     /// Test creating a UserBalanceStore with a repository
//     /// This tests the store creation with database access
//     #[tokio::test]
//     async fn test_user_balance_store_with_repository() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
//         let config = UserBalanceCacheConfig::default();
        
//         let store = UserBalanceStore::with_repository(config, repo);
        
//         // Store should be created successfully
//         // We can't access private fields directly, but we can test that it was created
//         assert_eq!(store.get_stats().hits, 0);
//         assert_eq!(store.get_stats().misses, 0);
//     }

//     /// Test creating a UserBalanceStore with Redis cache
//     /// This tests the store creation with Redis caching enabled
//     #[tokio::test]
//     async fn test_user_balance_store_with_redis_cache() {
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = UserBalanceStore::with_redis_cache(cache_config, redis_config, None).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         // Store should be created successfully
//         // We can't access private fields directly, but we can test that it was created
//         assert_eq!(store.get_stats().hits, 0);
//         assert_eq!(store.get_stats().misses, 0);
//     }

//     /// Test creating a UserBalanceStore with both Redis cache and repository
//     /// This tests the full configuration with both caching and database access
//     #[tokio::test]
//     async fn test_user_balance_store_with_cache_and_repository() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = UserBalanceStore::with_redis_cache(cache_config, redis_config, Some(repo)).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         // Store should be created successfully
//         // We can't access private fields directly, but we can test that it was created
//         assert_eq!(store.get_stats().hits, 0);
//         assert_eq!(store.get_stats().misses, 0);
//     }

//     /// Test getting a user balance when no repository is available
//     /// This should return None without errors
//     #[tokio::test]
//     async fn test_get_user_balance_no_repository() {
//         let config = UserBalanceCacheConfig::default();
//         let store = UserBalanceStore::new(config);
//         let user_id = create_test_user_id();

//         let result = store.get_user_balance_by_id(user_id).await;
        
//         match result {
//             Ok(None) => {
//                 // This is expected - no repository means no data
//             }
//             Ok(Some(_)) => {
//                 panic!("Expected None but got Some balance");
//             }
//             Err(e) => {
//                 panic!("Expected Ok(None) but got error: {}", e);
//             }
//         }
//     }

//     /// Test getting a user balance from repository (no cache)
//     /// This tests the basic database lookup functionality
//     #[tokio::test]
//     async fn test_get_user_balance_from_repository() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let config = create_disabled_cache_config();
//         let store = UserBalanceStore::with_repository(config, repo.clone());
//         let user_id = create_test_user_id();

//         // Set up a user first (foreign key requirement)
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_repo_balance_{}', 'test_repo_balance_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create a balance for this user
//         repo.create_user_balance(user_id, 15000, 3000).await.unwrap();

//         // Now try to get the balance
//         let result = store.get_user_balance_by_id(user_id).await;

//         match result {
//             Ok(Some(balance)) => {
//                 assert_eq!(balance.user_id, user_id);
//                 assert_eq!(balance.available, 15000);
//                 assert_eq!(balance.locked, 3000);
//             }
//             Ok(None) => {
//                 panic!("Expected balance but got None");
//             }
//             Err(e) => {
//                 panic!("Get user balance failed: {}", e);
//             }
//         }
//     }

//     /// Test getting a user balance that doesn't exist
//     /// This should return None, not an error
//     #[tokio::test]
//     async fn test_get_user_balance_not_found() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
//         let config = create_disabled_cache_config();
//         let store = UserBalanceStore::with_repository(config, repo);
//         let non_existent_user_id = Uuid::new_v4();

//         let result = store.get_user_balance_by_id(non_existent_user_id).await;

//         match result {
//             Ok(None) => {
//                 // This is expected - user doesn't exist
//             }
//             Ok(Some(_)) => {
//                 panic!("Expected None but got Some balance");
//             }
//             Err(e) => {
//                 panic!("Get user balance failed: {}", e);
//             }
//         }
//     }

//     /// Test cache hit behavior
//     /// This tests that the cache is working correctly
//     #[tokio::test]
//     async fn test_cache_hit_behavior() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = UserBalanceStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create a balance
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_cache_hit_{}', 'test_cache_hit_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create a balance for this user using the repository directly
//         repo.create_user_balance(user_id, 20000, 4000).await.unwrap();

//         // First call should hit the database and cache the result
//         let result1 = store.get_user_balance_by_id(user_id).await.unwrap();
//         assert!(result1.is_some());
//         let balance1 = result1.unwrap();

//         // Second call should hit the cache
//         let result2 = store.get_user_balance_by_id(user_id).await.unwrap();
//         assert!(result2.is_some());
//         let balance2 = result2.unwrap();

//         // Both results should be identical
//         assert_eq!(balance1.user_id, balance2.user_id);
//         assert_eq!(balance1.available, balance2.available);
//         assert_eq!(balance1.locked, balance2.locked);

//         // Check that we have cache hits
//         let stats = store.get_stats();
//         assert!(stats.hits > 0);
//     }

//     /// Test cache miss behavior
//     /// This tests that cache misses are handled correctly
//     #[tokio::test]
//     async fn test_cache_miss_behavior() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = UserBalanceStore::with_redis_cache(cache_config, redis_config, Some(repo)).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user but don't create a balance
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_cache_miss_{}', 'test_cache_miss_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Try to get a balance that doesn't exist
//         let result = store.get_user_balance_by_id(user_id).await.unwrap();
//         assert!(result.is_none());

//         // Check that we have cache misses
//         let stats = store.get_stats();
//         assert!(stats.misses > 0);
//     }

//     /// Test cache invalidation
//     /// This tests that cache invalidation works correctly by verifying hit/miss statistics
//     #[tokio::test]
//     async fn test_cache_invalidation() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = UserBalanceStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create a balance
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_invalidation_{}', 'test_invalidation_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create a balance for this user using the repository directly
//         repo.create_user_balance(user_id, 25000, 5000).await.unwrap();

//         // Initial stats should be zero
//         let initial_stats = store.get_stats();
//         assert_eq!(initial_stats.hits, 0);
//         assert_eq!(initial_stats.misses, 0);

//         // First call should be a cache miss (data not in cache yet)
//         let result1 = store.get_user_balance_by_id(user_id).await.unwrap();
//         assert!(result1.is_some());
        
//         let after_first_call_stats = store.get_stats();
//         assert_eq!(after_first_call_stats.misses, 1, "First call should be a cache miss");
//         assert_eq!(after_first_call_stats.hits, 0, "No hits yet");

//         // Second call should be a cache hit (data now in cache)
//         let result2 = store.get_user_balance_by_id(user_id).await.unwrap();
//         assert!(result2.is_some());
        
//         let after_second_call_stats = store.get_stats();
//         assert_eq!(after_second_call_stats.misses, 1, "Still only 1 miss");
//         assert_eq!(after_second_call_stats.hits, 1, "Second call should be a cache hit");

//         // Invalidate the cache
//         store.invalidate_user_balance(user_id).await;

//         // Third call should be a cache miss again (cache was invalidated)
//         let result3 = store.get_user_balance_by_id(user_id).await.unwrap();
//         assert!(result3.is_some());
        
//         let after_invalidation_stats = store.get_stats();
//         assert_eq!(after_invalidation_stats.misses, 2, "Third call should be a cache miss after invalidation");
//         assert_eq!(after_invalidation_stats.hits, 1, "Still only 1 hit");

//         // Verify data integrity - all results should be identical
//         assert_eq!(result1.as_ref().unwrap().user_id, result2.as_ref().unwrap().user_id);
//         assert_eq!(result2.as_ref().unwrap().user_id, result3.as_ref().unwrap().user_id);
//     }

//     /// Test cache statistics tracking
//     /// This tests that cache statistics are properly tracked
//     #[tokio::test]
//     async fn test_cache_statistics() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = UserBalanceStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create a balance
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_stats_{}', 'test_stats_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create a balance for this user using the repository directly
//         repo.create_user_balance(user_id, 30000, 6000).await.unwrap();

//         // Initial stats should be zero
//         let initial_stats = store.get_stats();
//         assert_eq!(initial_stats.hits, 0);
//         assert_eq!(initial_stats.misses, 0);
//         assert_eq!(initial_stats.errors, 0);

//         // First call should be a cache miss
//         let _ = store.get_user_balance_by_id(user_id).await.unwrap();
//         let after_miss_stats = store.get_stats();
//         assert!(after_miss_stats.misses > 0);

//         // Second call should be a cache hit
//         let _ = store.get_user_balance_by_id(user_id).await.unwrap();
//         let after_hit_stats = store.get_stats();
//         assert!(after_hit_stats.hits > 0);
//         assert!(after_hit_stats.hits > after_miss_stats.hits);
//     }

//     /// Test error handling when repository fails
//     /// This tests that repository errors are properly handled
//     #[tokio::test]
//     async fn test_repository_error_handling() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
//         let config = create_disabled_cache_config();
//         let store = UserBalanceStore::with_repository(config, repo);
//         let user_id = create_test_user_id();

//         // Try to get a balance for a user that doesn't exist
//         // This should return None, not an error
//         let result = store.get_user_balance_by_id(user_id).await;
        
//         match result {
//             Ok(None) => {
//                 // This is expected - user doesn't exist
//             }
//             Ok(Some(_)) => {
//                 panic!("Expected None but got Some balance");
//             }
//             Err(e) => {
//                 panic!("Expected Ok(None) but got error: {}", e);
//             }
//         }
//     }

//     /// Test concurrent access to the store
//     /// This tests thread safety
//     #[tokio::test]
//     async fn test_concurrent_access() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = Arc::new(UserBalanceStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!"));

//         let user_id = create_test_user_id();

//         // Set up a user and create a balance
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_concurrent_{}', 'test_concurrent_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create a balance for this user using the repository directly
//         repo.create_user_balance(user_id, 35000, 7000).await.unwrap();

//         let mut handles = vec![];

//         // Spawn multiple tasks that try to get the same balance concurrently
//         for _i in 0..10 {
//             let store_clone = Arc::clone(&store);
//             let handle = tokio::spawn(async move {
//                 let result = store_clone.get_user_balance_by_id(user_id).await;
//                 assert!(result.is_ok());
//                 let balance = result.unwrap();
//                 assert!(balance.is_some());
//                 let balance_model = balance.unwrap();
//                 assert_eq!(balance_model.user_id, user_id);
//                 assert_eq!(balance_model.available, 35000);
//                 assert_eq!(balance_model.locked, 7000);
//             });
            
//             handles.push(handle);
//         }

//         // Wait for all tasks to complete
//         for handle in handles {
//             handle.await.unwrap();
//         }
//     }

//     /// Test store Send + Sync traits
//     /// This ensures the store is safe for async usage
//     #[test]
//     fn test_store_send_sync() {
//         // Ensure UserBalanceStore is Send + Sync for async usage
//         fn assert_send<T: Send>() {}
//         fn assert_sync<T: Sync>() {}
        
//         assert_send::<UserBalanceStore>();
//         assert_sync::<UserBalanceStore>();
//     }

//     /// Final cleanup test that runs after all other tests
//     /// The 'zzz' prefix ensures this runs last
//     #[tokio::test]
//     async fn zzz_cleanup_after_all_tests() {
//         // Give other tests a moment to finish
//         tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
//         global_cleanup().await;
//     }
// }

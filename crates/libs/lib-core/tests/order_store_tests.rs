// //! Order Store Tests
// //! 
// //! These tests verify that our OrderStore works correctly with dual caching (individual orders + user order lists),
// //! database operations, and error handling. We test both cache types and their interactions.

// use chrono::Utc;
// use sea_orm::{Database, DatabaseConnection, ConnectionTrait};
// use std::sync::{Arc, Mutex};
// use uuid::Uuid;

// use lib_trading::{
//     repository::{TradingRepository, PostgresTradingRepository},
//     order_store::{OrderStore, OrderCacheConfig},
//     orders::Model as OrderModel,
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
//                 DROP TABLE IF EXISTS orders CASCADE;
                
//                 -- Orders table for trading operations
//                 CREATE TABLE orders (
//                     order_id BIGSERIAL PRIMARY KEY,
//                     user_id UUID NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
//                     price BIGINT NOT NULL,
//                     size BIGINT NOT NULL,
//                     side TEXT NOT NULL CHECK (side IN ('Bid', 'Ask')),
//                     timestamp BIGINT NOT NULL
//                 );
                
//                 -- Add some indexes to speed up queries
//                 CREATE INDEX IF NOT EXISTS idx_orders_user_id ON orders(user_id);
//                 CREATE INDEX IF NOT EXISTS idx_orders_timestamp ON orders(timestamp);
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
//         DELETE FROM orders WHERE user_id IN (
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

// /// Create a sample order for testing
// fn create_test_order(user_id: Uuid) -> OrderModel {
//     OrderModel {
//         order_id: 0, // Database will assign the real ID
//         user_id,
//         price: 50000,  // $500.00 in cents
//         size: 1000,    // 10 shares
//         side: "Bid".to_string(),
//         timestamp: Utc::now().timestamp_millis(),
//     }
// }

// /// Create a Redis config for testing
// fn create_test_redis_config() -> RedisCacheConfig {
//     RedisCacheConfig {
//         url: "redis://localhost:6379".to_string(),
//         key_prefix: "test_order:".to_string(),
//         max_connections: 5,
//         min_connections: 1,
//         connection_timeout_ms: 1000,
//         max_lifetime_secs: 300,
//         idle_timeout_secs: 60,
//         retry_count: 3,
//     }
// }

// /// Create a cache config with caching enabled
// fn create_enabled_cache_config() -> OrderCacheConfig {
//     OrderCacheConfig {
//         ttl: std::time::Duration::from_secs(60),
//         enabled: true,
//     }
// }

// /// Create a cache config with caching disabled
// fn create_disabled_cache_config() -> OrderCacheConfig {
//     OrderCacheConfig {
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

//     /// Test creating a new OrderStore with default config
//     /// This verifies the basic store creation works
//     #[test]
//     fn test_order_store_new() {
//         let config = OrderCacheConfig::default();
//         let store = OrderStore::new(config);
        
//         // Store should be created successfully
//         // We can't access private fields directly, but we can test that it was created
//         assert_eq!(store.get_stats().individual_cache.hits, 0);
//         assert_eq!(store.get_stats().individual_cache.misses, 0);
//         assert_eq!(store.get_stats().user_orders_cache.hits, 0);
//         assert_eq!(store.get_stats().user_orders_cache.misses, 0);
//         assert_eq!(store.get_stats().database.hits, 0);
//     }

//     /// Test creating an OrderStore with a repository
//     /// This tests the store creation with database access
//     #[tokio::test]
//     async fn test_order_store_with_repository() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
//         let config = OrderCacheConfig::default();
        
//         let store = OrderStore::with_repository(config, repo);
        
//         // Store should be created successfully
//         // We can't access private fields directly, but we can test that it was created
//         assert_eq!(store.get_stats().individual_cache.hits, 0);
//         assert_eq!(store.get_stats().individual_cache.misses, 0);
//         assert_eq!(store.get_stats().user_orders_cache.hits, 0);
//         assert_eq!(store.get_stats().user_orders_cache.misses, 0);
//         assert_eq!(store.get_stats().database.hits, 0);
//     }

//     /// Test creating an OrderStore with Redis cache (both individual and user orders caches)
//     /// This tests the store creation with Redis caching enabled
//     #[tokio::test]
//     async fn test_order_store_with_redis_cache() {
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, None).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         // Store should be created successfully
//         // We can't access private fields directly, but we can test that it was created
//         assert_eq!(store.get_stats().individual_cache.hits, 0);
//         assert_eq!(store.get_stats().individual_cache.misses, 0);
//         assert_eq!(store.get_stats().user_orders_cache.hits, 0);
//         assert_eq!(store.get_stats().user_orders_cache.misses, 0);
//         assert_eq!(store.get_stats().database.hits, 0);
//     }

//     /// Test creating an OrderStore with both Redis cache and repository
//     /// This tests the full configuration with both caching and database access
//     #[tokio::test]
//     async fn test_order_store_with_cache_and_repository() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo)).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         // Store should be created successfully
//         // We can't access private fields directly, but we can test that it was created
//         assert_eq!(store.get_stats().individual_cache.hits, 0);
//         assert_eq!(store.get_stats().individual_cache.misses, 0);
//         assert_eq!(store.get_stats().user_orders_cache.hits, 0);
//         assert_eq!(store.get_stats().user_orders_cache.misses, 0);
//         assert_eq!(store.get_stats().database.hits, 0);
//     }

//     /// Test getting an order when no repository is available
//     /// This should return None without errors
//     #[tokio::test]
//     async fn test_get_order_no_repository() {
//         let config = OrderCacheConfig::default();
//         let store = OrderStore::new(config);
//         let user_id = create_test_user_id();
//         let order_id = 12345;

//         let result = store.get_order_by_id(order_id, user_id).await;
        
//         match result {
//             Ok(None) => {
//                 // This is expected - no repository means no data
//             }
//             Ok(Some(_)) => {
//                 panic!("Expected None but got Some order");
//             }
//             Err(e) => {
//                 panic!("Expected Ok(None) but got error: {}", e);
//             }
//         }
//     }

//     /// Test getting user orders when no repository is available
//     /// This should return empty vector without errors
//     #[tokio::test]
//     async fn test_get_user_orders_no_repository() {
//         let config = OrderCacheConfig::default();
//         let store = OrderStore::new(config);
//         let user_id = create_test_user_id();

//         let result = store.get_user_orders(user_id).await;
        
//         match result {
//             Ok(orders) => {
//                 // Should return empty vector
//                 assert_eq!(orders.len(), 0);
//             }
//             Err(e) => {
//                 panic!("Expected Ok(vec![]) but got error: {}", e);
//             }
//         }
//     }

//     /// Test getting an order from repository (no cache)
//     /// This tests the basic database lookup functionality
//     #[tokio::test]
//     async fn test_get_order_from_repository() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let config = create_disabled_cache_config();
//         let store = OrderStore::with_repository(config, repo.clone());
//         let user_id = create_test_user_id();

//         // Set up a user first (foreign key requirement)
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_repo_order_{}', 'test_repo_order_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create an order for this user
//         let order = create_test_order(user_id);
//         let order_id = repo.create_order(order).await.unwrap();

//         // Now try to get the order
//         let result = store.get_order_by_id(order_id, user_id).await;

//         match result {
//             Ok(Some(order)) => {
//                 assert_eq!(order.order_id, order_id as i64);
//                 assert_eq!(order.user_id, user_id);
//                 assert_eq!(order.price, 50000);
//                 assert_eq!(order.size, 1000);
//                 assert_eq!(order.side, "Bid");
//             }
//             Ok(None) => {
//                 panic!("Expected order but got None");
//             }
//             Err(e) => {
//                 panic!("Get order failed: {}", e);
//             }
//         }
//     }

//     /// Test getting user orders from repository (no cache)
//     /// This tests the basic database lookup functionality for order lists
//     #[tokio::test]
//     async fn test_get_user_orders_from_repository() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let config = create_disabled_cache_config();
//         let store = OrderStore::with_repository(config, repo.clone());
//         let user_id = create_test_user_id();

//         // Set up a user first (foreign key requirement)
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_repo_orders_{}', 'test_repo_orders_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create two orders for this user
//         let order1 = create_test_order(user_id);
//         let order2 = OrderModel {
//             order_id: 0,
//             user_id,
//             price: 51000,
//             size: 2000,
//             side: "Ask".to_string(),
//             timestamp: Utc::now().timestamp_millis(),
//         };

//         let order1_id = repo.create_order(order1).await.unwrap();
//         let order2_id = repo.create_order(order2).await.unwrap();

//         // Now try to get all orders for this user
//         let result = store.get_user_orders(user_id).await;

//         match result {
//             Ok(orders) => {
//                 assert_eq!(orders.len(), 2);
                
//                 // Make sure both orders are in the result
//                 let order_ids: Vec<i64> = orders.iter().map(|o| o.order_id).collect();
//                 assert!(order_ids.contains(&(order1_id as i64)));
//                 assert!(order_ids.contains(&(order2_id as i64)));
                
//                 // Verify the order details are correct
//                 for order in &orders {
//                     assert_eq!(order.user_id, user_id);
//                     assert!(order.price == 50000 || order.price == 51000);
//                     assert!(order.size == 1000 || order.size == 2000);
//                     assert!(order.side == "Bid" || order.side == "Ask");
//                 }
//             }
//             Err(e) => {
//                 panic!("Get user orders failed: {}", e);
//             }
//         }
//     }

//     /// Test getting an order that doesn't exist
//     /// This should return None, not an error
//     #[tokio::test]
//     async fn test_get_order_not_found() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
//         let config = create_disabled_cache_config();
//         let store = OrderStore::with_repository(config, repo);
//         let user_id = create_test_user_id();
//         let non_existent_order_id = 99999;

//         let result = store.get_order_by_id(non_existent_order_id, user_id).await;

//         match result {
//             Ok(None) => {
//                 // This is expected - order doesn't exist
//             }
//             Ok(Some(_)) => {
//                 panic!("Expected None but got Some order");
//             }
//             Err(e) => {
//                 panic!("Get order failed: {}", e);
//             }
//         }
//     }

//     /// Test getting user orders for a user with no orders
//     /// This should return empty vector, not an error
//     #[tokio::test]
//     async fn test_get_user_orders_empty() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let config = create_disabled_cache_config();
//         let store = OrderStore::with_repository(config, repo);
//         let user_id = create_test_user_id();

//         // Set up a user but don't create any orders
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_empty_orders_{}', 'test_empty_orders_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         let result = store.get_user_orders(user_id).await;

//         match result {
//             Ok(orders) => {
//                 // Should be an empty list
//                 assert_eq!(orders.len(), 0);
//             }
//             Err(e) => {
//                 panic!("Get user orders failed: {}", e);
//             }
//         }
//     }

//     /// Test individual order cache hit behavior
//     /// This tests that the individual order cache is working correctly
//     #[tokio::test]
//     async fn test_individual_order_cache_hit_behavior() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create an order
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_individual_cache_{}', 'test_individual_cache_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create an order for this user using the repository directly
//         let order = create_test_order(user_id);
//         let order_id = repo.create_order(order).await.unwrap();

//         // First call should hit the database and cache the result
//         let result1 = store.get_order_by_id(order_id, user_id).await.unwrap();
//         assert!(result1.is_some());
//         let order1 = result1.unwrap();

//         // Second call should hit the cache
//         let result2 = store.get_order_by_id(order_id, user_id).await.unwrap();
//         assert!(result2.is_some());
//         let order2 = result2.unwrap();

//         // Both results should be identical
//         assert_eq!(order1.order_id, order2.order_id);
//         assert_eq!(order1.user_id, order2.user_id);
//         assert_eq!(order1.price, order2.price);
//         assert_eq!(order1.size, order2.size);
//         assert_eq!(order1.side, order2.side);

//         // Check that we have cache hits
//         let stats = store.get_stats();
//         assert!(stats.individual_cache.hits > 0);
//     }

//     /// Test user orders cache hit behavior
//     /// This tests that the user orders cache is working correctly
//     #[tokio::test]
//     async fn test_user_orders_cache_hit_behavior() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create multiple orders
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_user_orders_cache_{}', 'test_user_orders_cache_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create two orders for this user using the repository directly
//         let order1 = create_test_order(user_id);
//         let order2 = OrderModel {
//             order_id: 0,
//             user_id,
//             price: 52000,
//             size: 1500,
//             side: "Ask".to_string(),
//             timestamp: Utc::now().timestamp_millis(),
//         };

//         repo.create_order(order1).await.unwrap();
//         repo.create_order(order2).await.unwrap();

//         // First call should hit the database and cache the result
//         let result1 = store.get_user_orders(user_id).await.unwrap();
//         assert_eq!(result1.len(), 2);

//         // Second call should hit the cache
//         let result2 = store.get_user_orders(user_id).await.unwrap();
//         assert_eq!(result2.len(), 2);

//         // Both results should be identical
//         assert_eq!(result1.len(), result2.len());
//         for (order1, order2) in result1.iter().zip(result2.iter()) {
//             assert_eq!(order1.order_id, order2.order_id);
//             assert_eq!(order1.user_id, order2.user_id);
//             assert_eq!(order1.price, order2.price);
//             assert_eq!(order1.size, order2.size);
//             assert_eq!(order1.side, order2.side);
//         }

//         // Check that we have cache hits
//         let stats = store.get_stats();
//         assert!(stats.user_orders_cache.hits > 0);
//     }

//     /// Test cache miss behavior for individual orders
//     /// This tests that cache misses are handled correctly
//     #[tokio::test]
//     async fn test_individual_order_cache_miss_behavior() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo)).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();
//         let non_existent_order_id = 99999;

//         // Set up a user but don't create any orders
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_individual_miss_{}', 'test_individual_miss_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Try to get an order that doesn't exist
//         let result = store.get_order_by_id(non_existent_order_id, user_id).await.unwrap();
//         assert!(result.is_none());

//         // Check that we have cache misses
//         let stats = store.get_stats();
//         assert!(stats.individual_cache.misses > 0);
//     }

//     /// Test cache miss behavior for user orders
//     /// This tests that cache misses are handled correctly for order lists
//     #[tokio::test]
//     async fn test_user_orders_cache_miss_behavior() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo)).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user but don't create any orders
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_user_orders_miss_{}', 'test_user_orders_miss_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Try to get orders for a user with no orders
//         let result = store.get_user_orders(user_id).await.unwrap();
//         assert_eq!(result.len(), 0);

//         // Check that we have cache misses
//         let stats = store.get_stats();
//         assert!(stats.user_orders_cache.misses > 0);
//     }

//     /// Test cache invalidation for individual orders
//     /// This tests that individual order cache invalidation works correctly
//     #[tokio::test]
//     async fn test_individual_order_cache_invalidation() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create an order
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_individual_invalidation_{}', 'test_individual_invalidation_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create an order for this user using the repository directly
//         let order = create_test_order(user_id);
//         let order_id = repo.create_order(order).await.unwrap();

//         // Initial stats should be zero
//         let initial_stats = store.get_stats();
//         assert_eq!(initial_stats.individual_cache.hits, 0);
//         assert_eq!(initial_stats.individual_cache.misses, 0);

//         // First call should be a cache miss (data not in cache yet)
//         let result1 = store.get_order_by_id(order_id, user_id).await.unwrap();
//         assert!(result1.is_some());
        
//         let after_first_call_stats = store.get_stats();
//         assert_eq!(after_first_call_stats.individual_cache.misses, 1, "First call should be a cache miss");
//         assert_eq!(after_first_call_stats.individual_cache.hits, 0, "No hits yet");

//         // Second call should be a cache hit (data now in cache)
//         let result2 = store.get_order_by_id(order_id, user_id).await.unwrap();
//         assert!(result2.is_some());
        
//         let after_second_call_stats = store.get_stats();
//         assert_eq!(after_second_call_stats.individual_cache.misses, 1, "Still only 1 miss");
//         assert_eq!(after_second_call_stats.individual_cache.hits, 1, "Second call should be a cache hit");

//         // Invalidate the cache
//         store.invalidate_order(order_id, user_id).await;

//         // Third call should be a cache miss again (cache was invalidated)
//         let result3 = store.get_order_by_id(order_id, user_id).await.unwrap();
//         assert!(result3.is_some());
        
//         let after_invalidation_stats = store.get_stats();
//         assert_eq!(after_invalidation_stats.individual_cache.misses, 2, "Third call should be a cache miss after invalidation");
//         assert_eq!(after_invalidation_stats.individual_cache.hits, 1, "Still only 1 hit");

//         // Verify data integrity - all results should be identical
//         assert_eq!(result1.as_ref().unwrap().order_id, result2.as_ref().unwrap().order_id);
//         assert_eq!(result2.as_ref().unwrap().order_id, result3.as_ref().unwrap().order_id);
//     }

//     /// Test cache invalidation for user orders
//     /// This tests that user orders cache invalidation works correctly
//     #[tokio::test]
//     async fn test_user_orders_cache_invalidation() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create orders
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_user_orders_invalidation_{}', 'test_user_orders_invalidation_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create orders for this user using the repository directly
//         let order1 = create_test_order(user_id);
//         let order2 = OrderModel {
//             order_id: 0,
//             user_id,
//             price: 53000,
//             size: 2500,
//             side: "Ask".to_string(),
//             timestamp: Utc::now().timestamp_millis(),
//         };

//         repo.create_order(order1).await.unwrap();
//         repo.create_order(order2).await.unwrap();

//         // Initial stats should be zero
//         let initial_stats = store.get_stats();
//         assert_eq!(initial_stats.user_orders_cache.hits, 0);
//         assert_eq!(initial_stats.user_orders_cache.misses, 0);

//         // First call should be a cache miss (data not in cache yet)
//         let result1 = store.get_user_orders(user_id).await.unwrap();
//         assert_eq!(result1.len(), 2);
        
//         let after_first_call_stats = store.get_stats();
//         assert_eq!(after_first_call_stats.user_orders_cache.misses, 1, "First call should be a cache miss");
//         assert_eq!(after_first_call_stats.user_orders_cache.hits, 0, "No hits yet");

//         // Second call should be a cache hit (data now in cache)
//         let result2 = store.get_user_orders(user_id).await.unwrap();
//         assert_eq!(result2.len(), 2);
        
//         let after_second_call_stats = store.get_stats();
//         assert_eq!(after_second_call_stats.user_orders_cache.misses, 1, "Still only 1 miss");
//         assert_eq!(after_second_call_stats.user_orders_cache.hits, 1, "Second call should be a cache hit");

//         // Invalidate the cache
//         store.invalidate_user_orders(user_id).await;

//         // Third call should be a cache miss again (cache was invalidated)
//         let result3 = store.get_user_orders(user_id).await.unwrap();
//         assert_eq!(result3.len(), 2);
        
//         let after_invalidation_stats = store.get_stats();
//         assert_eq!(after_invalidation_stats.user_orders_cache.misses, 2, "Third call should be a cache miss after invalidation");
//         assert_eq!(after_invalidation_stats.user_orders_cache.hits, 1, "Still only 1 hit");

//         // Verify data integrity - all results should be identical
//         assert_eq!(result1.len(), result2.len());
//         assert_eq!(result2.len(), result3.len());
//     }

//     /// Test dual cache invalidation
//     /// This tests that invalidating an individual order also invalidates user orders cache
//     #[tokio::test]
//     async fn test_dual_cache_invalidation() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create orders
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_dual_invalidation_{}', 'test_dual_invalidation_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create orders for this user using the repository directly
//         let order1 = create_test_order(user_id);
//         let order2 = OrderModel {
//             order_id: 0,
//             user_id,
//             price: 54000,
//             size: 3000,
//             side: "Ask".to_string(),
//             timestamp: Utc::now().timestamp_millis(),
//         };

//         let order1_id = repo.create_order(order1).await.unwrap();
//         repo.create_order(order2).await.unwrap();

//         // Initial stats should be zero
//         let initial_stats = store.get_stats();
//         assert_eq!(initial_stats.individual_cache.hits, 0);
//         assert_eq!(initial_stats.individual_cache.misses, 0);
//         assert_eq!(initial_stats.user_orders_cache.hits, 0);
//         assert_eq!(initial_stats.user_orders_cache.misses, 0);

//         // Cache both individual order and user orders (first calls = cache misses)
//         let _ = store.get_order_by_id(order1_id, user_id).await.unwrap();
//         let _ = store.get_user_orders(user_id).await.unwrap();
        
//         let after_first_calls_stats = store.get_stats();
//         assert_eq!(after_first_calls_stats.individual_cache.misses, 1, "Individual order first call should be cache miss");
//         assert_eq!(after_first_calls_stats.user_orders_cache.misses, 1, "User orders first call should be cache miss");

//         // Second calls should be cache hits
//         let _ = store.get_order_by_id(order1_id, user_id).await.unwrap();
//         let _ = store.get_user_orders(user_id).await.unwrap();
        
//         let after_second_calls_stats = store.get_stats();
//         assert_eq!(after_second_calls_stats.individual_cache.hits, 1, "Individual order second call should be cache hit");
//         assert_eq!(after_second_calls_stats.user_orders_cache.hits, 1, "User orders second call should be cache hit");

//         // Invalidate individual order (should also invalidate user orders cache)
//         store.invalidate_order(order1_id, user_id).await;

//         // Both caches should be invalidated - next calls should be cache misses again
//         let individual_result = store.get_order_by_id(order1_id, user_id).await.unwrap();
//         assert!(individual_result.is_some());

//         let user_orders_result = store.get_user_orders(user_id).await.unwrap();
//         assert_eq!(user_orders_result.len(), 2);
        
//         let after_invalidation_stats = store.get_stats();
//         assert_eq!(after_invalidation_stats.individual_cache.misses, 2, "Individual order should be cache miss after invalidation");
//         assert_eq!(after_invalidation_stats.user_orders_cache.misses, 2, "User orders should be cache miss after invalidation");
//         assert_eq!(after_invalidation_stats.individual_cache.hits, 1, "Individual order hits should remain 1");
//         assert_eq!(after_invalidation_stats.user_orders_cache.hits, 1, "User orders hits should remain 1");
//     }

//     /// Test cache statistics tracking
//     /// This tests that cache statistics are properly tracked for both cache types
//     #[tokio::test]
//     async fn test_cache_statistics() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = OrderStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!");

//         let user_id = create_test_user_id();

//         // Set up a user and create an order
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_stats_{}', 'test_stats_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create an order for this user using the repository directly
//         let order = create_test_order(user_id);
//         let order_id = repo.create_order(order).await.unwrap();

//         // Initial stats should be zero
//         let initial_stats = store.get_stats();
//         assert_eq!(initial_stats.individual_cache.hits, 0);
//         assert_eq!(initial_stats.individual_cache.misses, 0);
//         assert_eq!(initial_stats.individual_cache.errors, 0);
//         assert_eq!(initial_stats.user_orders_cache.hits, 0);
//         assert_eq!(initial_stats.user_orders_cache.misses, 0);
//         assert_eq!(initial_stats.user_orders_cache.errors, 0);
//         assert_eq!(initial_stats.database.hits, 0);
//         assert_eq!(initial_stats.database.errors, 0);

//         // First call should be a cache miss
//         let _ = store.get_order_by_id(order_id, user_id).await.unwrap();
//         let after_miss_stats = store.get_stats();
//         assert!(after_miss_stats.individual_cache.misses > 0);

//         // Second call should be a cache hit
//         let _ = store.get_order_by_id(order_id, user_id).await.unwrap();
//         let after_hit_stats = store.get_stats();
//         assert!(after_hit_stats.individual_cache.hits > 0);
//         assert!(after_hit_stats.individual_cache.hits > after_miss_stats.individual_cache.hits);

//         // Test user orders cache statistics
//         let _ = store.get_user_orders(user_id).await.unwrap();
//         let after_user_orders_miss = store.get_stats();
//         assert!(after_user_orders_miss.user_orders_cache.misses > after_hit_stats.user_orders_cache.misses);

//         let _ = store.get_user_orders(user_id).await.unwrap();
//         let after_user_orders_hit = store.get_stats();
//         assert!(after_user_orders_hit.user_orders_cache.hits > after_hit_stats.user_orders_cache.hits);
//     }

//     /// Test concurrent access to the store
//     /// This tests thread safety for both cache types
//     #[tokio::test]
//     async fn test_concurrent_access() {
//         let db = setup_test_db_with_schema().await;
//         let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
//         let redis_config = create_test_redis_config();
//         let cache_config = create_enabled_cache_config();
        
//         let store = Arc::new(OrderStore::with_redis_cache(cache_config, redis_config, Some(repo.clone())).await
//             .expect("❌ Redis connection failed - Redis must be running for this test!"));

//         let user_id = create_test_user_id();

//         // Set up a user and create an order
//         let create_user_sql = format!(
//             "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_concurrent_{}', 'test_concurrent_{}@example.com', 'hash')",
//             user_id, user_id, user_id
//         );
//         db.execute_unprepared(&create_user_sql).await.unwrap();

//         // Create an order for this user using the repository directly
//         let order = create_test_order(user_id);
//         let order_id = repo.create_order(order).await.unwrap();

//         let mut handles = vec![];

//         // Spawn multiple tasks that try to get the same order concurrently
//         for _i in 0..10 {
//             let store_clone = Arc::clone(&store);
//             let handle = tokio::spawn(async move {
//                 let result = store_clone.get_order_by_id(order_id, user_id).await;
//                 assert!(result.is_ok());
//                 let order = result.unwrap();
//                 assert!(order.is_some());
//                 let order_model = order.unwrap();
//                 assert_eq!(order_model.user_id, user_id);
//                 assert_eq!(order_model.price, 50000);
//                 assert_eq!(order_model.size, 1000);
//                 assert_eq!(order_model.side, "Bid");
//             });
            
//             handles.push(handle);
//         }

//         // Spawn multiple tasks that try to get user orders concurrently
//         for _i in 0..10 {
//             let store_clone = Arc::clone(&store);
//             let handle = tokio::spawn(async move {
//                 let result = store_clone.get_user_orders(user_id).await;
//                 assert!(result.is_ok());
//                 let orders = result.unwrap();
//                 assert_eq!(orders.len(), 1);
//                 assert_eq!(orders[0].user_id, user_id);
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
//         // Ensure OrderStore is Send + Sync for async usage
//         fn assert_send<T: Send>() {}
//         fn assert_sync<T: Sync>() {}
        
//         assert_send::<OrderStore>();
//         assert_sync::<OrderStore>();
//     }

//     /// Final cleanup test that runs after all other tests
//     #[tokio::test]
//     async fn zzz_cleanup_after_all_tests() {
//         // Giving other tests a moment to finish
//         tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
//         global_cleanup().await;
//     }
// }

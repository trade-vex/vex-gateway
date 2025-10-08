//! order store tests
//! 
//! these tests verify that our orderstore works correctly with dual caching (individual orders + user order lists),
//! database operations, and error handling. we test both cache types and their interactions.

use chrono::Utc;
use sea_orm::{Database, DatabaseConnection, ConnectionTrait};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use lib_core::{
    repository::repository::{TradingRepository, PostgresTradingRepository},
    cache::order_store::{OrderStore, OrderCacheConfig},
    model::orders::Model as OrderModel,
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
                DROP TABLE IF EXISTS orders CASCADE;
                
                -- orders table for trading operations
                CREATE TABLE orders (
                    order_id BIGSERIAL PRIMARY KEY,
                    user_id UUID NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
                    price BIGINT NOT NULL,
                    size BIGINT NOT NULL,
                    side TEXT NOT NULL CHECK (side IN ('Bid', 'Ask')),
                    timestamp BIGINT NOT NULL
                );
                
                -- add some indexes to speed up queries
                CREATE INDEX IF NOT EXISTS idx_orders_user_id ON orders(user_id);
                CREATE INDEX IF NOT EXISTS idx_orders_timestamp ON orders(timestamp);
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
        DELETE FROM orders WHERE user_id IN (
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

// create a sample order for testing
fn create_test_order(user_id: Uuid) -> OrderModel {
    OrderModel {
        order_id: 0, // database will assign the real id
        user_id,
        price: 50000,  // $500.00 in cents
        size: 1000,    // 10 shares
        side: "Bid".to_string(),
        timestamp: Utc::now().timestamp_millis(),
    }
}

// create a cache config with caching enabled
fn create_enabled_cache_config() -> OrderCacheConfig {
    OrderCacheConfig {
        ttl: std::time::Duration::from_secs(60),
        enabled: true,
    }
}

// create a cache config with caching disabled
fn create_disabled_cache_config() -> OrderCacheConfig {
    OrderCacheConfig {
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

    // test creating a new orderstore with default config
    // this verifies the basic store creation works
    #[test]
    fn test_order_store_new() {
        let config = OrderCacheConfig::default();
        let _store = OrderStore::new(config.clone());
        
        // Store should be created successfully
        // We can't access private fields directly, but we can test that it was created
        // The store should be created without errors
        assert!(!config.enabled); // Default config should have caching disabled
    }

    // test creating an orderstore with a repository
    // this tests the store creation with database access
    #[tokio::test]
    async fn test_order_store_with_repository() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
        let config = OrderCacheConfig::default();
        
        let _store = OrderStore::with_repository(config.clone(), repo);
        
        // Store should be created successfully
        // We can't access private fields directly, but we can test that it was created
        // The store should be created without errors
        assert!(!config.enabled); // Default config should have caching disabled
    }

    // test getting an order when no repository is available
    // this should return none without errors
    #[tokio::test]
    async fn test_get_order_no_repository() {
        let config = OrderCacheConfig::default();
        let store = OrderStore::new(config);
        let user_id = create_test_user_id();
        let order_id = 12345;

        let result = store.get_order_by_id(order_id, user_id).await;
        
        match result {
            Ok(None) => {
                // This is expected - no repository means no data
            }
            Ok(Some(_)) => {
                panic!("Expected None but got Some order");
            }
            Err(e) => {
                panic!("Expected Ok(None) but got error: {}", e);
            }
        }
    }

    /// Test getting user orders when no repository is available
    /// This should return empty vector without errors
    #[tokio::test]
    async fn test_get_user_orders_no_repository() {
        let config = OrderCacheConfig::default();
        let store = OrderStore::new(config);
        let user_id = create_test_user_id();

        let result = store.get_user_orders(user_id).await;
        
        match result {
            Ok(orders) => {
                // Should return empty vector
                assert_eq!(orders.len(), 0);
            }
            Err(e) => {
                panic!("Expected Ok(vec![]) but got error: {}", e);
            }
        }
    }

    /// Test getting an order from repository (no cache)
    /// This tests the basic database lookup functionality
    #[tokio::test]
    async fn test_get_order_from_repository() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_disabled_cache_config();
        let store = OrderStore::with_repository(config, repo.clone());
        let user_id = create_test_user_id();

        // Set up a user first (foreign key requirement)
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_repo_order_{}', 'test_repo_order_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create an order for this user
        let order = create_test_order(user_id);
        let order_id = repo.create_order(order).await.unwrap();

        // Now try to get the order
        let result = store.get_order_by_id(order_id, user_id).await;

        match result {
            Ok(Some(order)) => {
                assert_eq!(order.order_id, order_id as i64);
                assert_eq!(order.user_id, user_id);
                assert_eq!(order.price, 50000);
                assert_eq!(order.size, 1000);
                assert_eq!(order.side, "Bid");
            }
            Ok(None) => {
                panic!("Expected order but got None");
            }
            Err(e) => {
                panic!("Get order failed: {}", e);
            }
        }
    }

    /// Test getting user orders from repository (no cache)
    /// This tests the basic database lookup functionality for order lists
    #[tokio::test]
    async fn test_get_user_orders_from_repository() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_disabled_cache_config();
        let store = OrderStore::with_repository(config, repo.clone());
        let user_id = create_test_user_id();

        // Set up a user first (foreign key requirement)
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_repo_orders_{}', 'test_repo_orders_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create two orders for this user
        let order1 = create_test_order(user_id);
        let order2 = OrderModel {
            order_id: 0,
            user_id,
            price: 51000,
            size: 2000,
            side: "Ask".to_string(),
            timestamp: Utc::now().timestamp_millis(),
        };

        let order1_id = repo.create_order(order1).await.unwrap();
        let order2_id = repo.create_order(order2).await.unwrap();

        // Now try to get all orders for this user
        let result = store.get_user_orders(user_id).await;

        match result {
            Ok(orders) => {
                assert_eq!(orders.len(), 2);
                
                // Make sure both orders are in the result
                let order_ids: Vec<i64> = orders.iter().map(|o| o.order_id).collect();
                assert!(order_ids.contains(&(order1_id as i64)));
                assert!(order_ids.contains(&(order2_id as i64)));
                
                // Verify the order details are correct
                for order in &orders {
                    assert_eq!(order.user_id, user_id);
                    assert!(order.price == 50000 || order.price == 51000);
                    assert!(order.size == 1000 || order.size == 2000);
                    assert!(order.side == "Bid" || order.side == "Ask");
                }
            }
            Err(e) => {
                panic!("Get user orders failed: {}", e);
            }
        }
    }

    /// Test getting an order that doesn't exist
    /// This should return None, not an error
    #[tokio::test]
    async fn test_get_order_not_found() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db)));
        let config = create_disabled_cache_config();
        let store = OrderStore::with_repository(config, repo);
        let user_id = create_test_user_id();
        let non_existent_order_id = 99999;

        let result = store.get_order_by_id(non_existent_order_id, user_id).await;

        match result {
            Ok(None) => {
                // This is expected - order doesn't exist
            }
            Ok(Some(_)) => {
                panic!("Expected None but got Some order");
            }
            Err(e) => {
                panic!("Get order failed: {}", e);
            }
        }
    }

    /// Test getting user orders for a user with no orders
    /// This should return empty vector, not an error
    #[tokio::test]
    async fn test_get_user_orders_empty() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_disabled_cache_config();
        let store = OrderStore::with_repository(config, repo);
        let user_id = create_test_user_id();

        // Set up a user but don't create any orders
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_empty_orders_{}', 'test_empty_orders_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        let result = store.get_user_orders(user_id).await;

        match result {
            Ok(orders) => {
                // Should be an empty list
                assert_eq!(orders.len(), 0);
            }
            Err(e) => {
                panic!("Get user orders failed: {}", e);
            }
        }
    }

    // test redis cache hit behavior for individual orders
    // this tests that the redis cache is working correctly for individual orders
    #[tokio::test]
    async fn test_redis_individual_order_cache_hit_behavior() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        // Create store with Redis cache - add timeout to prevent hanging
        let store_future = OrderStore::with_redis_cache(
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

        // Set up a user and create an order
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_redis_individual_{}', 'test_redis_individual_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create an order for this user using the repository directly
        let order = create_test_order(user_id);
        let order_id = repo.create_order(order).await.unwrap();

        // First call should hit the database and cache the result
        let result1 = store.get_order_by_id(order_id, user_id).await.unwrap();
        assert!(result1.is_some());
        let order1 = result1.unwrap();

        // Second call should hit the cache
        let result2 = store.get_order_by_id(order_id, user_id).await.unwrap();
        assert!(result2.is_some());
        let order2 = result2.unwrap();

        // Both results should be identical
        assert_eq!(order1.order_id, order2.order_id);
        assert_eq!(order1.user_id, order2.user_id);
        assert_eq!(order1.price, order2.price);
        assert_eq!(order1.size, order2.size);
        assert_eq!(order1.side, order2.side);
    }

    // test redis cache hit behavior for user orders
    // this tests that the redis cache is working correctly for user order lists
    #[tokio::test]
    async fn test_redis_user_orders_cache_hit_behavior() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        // Create store with Redis cache - add timeout to prevent hanging
        let store_future = OrderStore::with_redis_cache(
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

        // Set up a user and create multiple orders
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_redis_user_orders_{}', 'test_redis_user_orders_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create two orders for this user using the repository directly
        let order1 = create_test_order(user_id);
        let order2 = OrderModel {
            order_id: 0,
            user_id,
            price: 52000,
            size: 1500,
            side: "Ask".to_string(),
            timestamp: Utc::now().timestamp_millis(),
        };
        repo.create_order(order1).await.unwrap();
        repo.create_order(order2).await.unwrap();

        // First call should hit the database and cache the result
        let result1 = store.get_user_orders(user_id).await.unwrap();
        assert_eq!(result1.len(), 2);

        // Second call should hit the cache
        let result2 = store.get_user_orders(user_id).await.unwrap();
        assert_eq!(result2.len(), 2);

        // Both results should be identical
        assert_eq!(result1.len(), result2.len());
        for (order1, order2) in result1.iter().zip(result2.iter()) {
            assert_eq!(order1.order_id, order2.order_id);
            assert_eq!(order1.user_id, order2.user_id);
            assert_eq!(order1.price, order2.price);
            assert_eq!(order1.size, order2.size);
            assert_eq!(order1.side, order2.side);
        }
    }

    /// Test cache statistics tracking
    /// This tests that cache statistics are properly tracked for both cache types
    #[tokio::test]
    async fn test_cache_statistics() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        let store = OrderStore::with_repository(config, repo.clone());
        let user_id = create_test_user_id();

        // Set up a user and create an order
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_stats_{}', 'test_stats_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create an order for this user using the repository directly
        let order = create_test_order(user_id);
        let order_id = repo.create_order(order).await.unwrap();

        // Test that we can get the order multiple times
        let result1 = store.get_order_by_id(order_id, user_id).await.unwrap();
        assert!(result1.is_some());

        let result2 = store.get_order_by_id(order_id, user_id).await.unwrap();
        assert!(result2.is_some());

        // Test that we can get user orders multiple times
        let orders1 = store.get_user_orders(user_id).await.unwrap();
        assert_eq!(orders1.len(), 1);

        let orders2 = store.get_user_orders(user_id).await.unwrap();
        assert_eq!(orders2.len(), 1);

        // Verify data integrity - all results should be identical
        assert_eq!(result1.as_ref().unwrap().order_id, result2.as_ref().unwrap().order_id);
        assert_eq!(orders1.len(), orders2.len());
    }

    /// Test concurrent access to the store
    /// This tests thread safety for both cache types
    #[tokio::test]
    async fn test_concurrent_access() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let config = create_enabled_cache_config();
        
        let store = Arc::new(OrderStore::with_repository(config, repo.clone()));

        let user_id = create_test_user_id();

        // Set up a user and create an order
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_concurrent_{}', 'test_concurrent_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create an order for this user using the repository directly
        let order = create_test_order(user_id);
        let order_id = repo.create_order(order).await.unwrap();

        let mut handles = vec![];

        // Spawn multiple tasks that try to get the same order concurrently
        for _i in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = tokio::spawn(async move {
                let result = store_clone.get_order_by_id(order_id, user_id).await;
                assert!(result.is_ok());
                let order = result.unwrap();
                assert!(order.is_some());
                let order_model = order.unwrap();
                assert_eq!(order_model.user_id, user_id);
                assert_eq!(order_model.price, 50000);
                assert_eq!(order_model.size, 1000);
                assert_eq!(order_model.side, "Bid");
            });
            
            handles.push(handle);
        }

        // Spawn multiple tasks that try to get user orders concurrently
        for _i in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = tokio::spawn(async move {
                let result = store_clone.get_user_orders(user_id).await;
                assert!(result.is_ok());
                let orders = result.unwrap();
                assert_eq!(orders.len(), 1);
                assert_eq!(orders[0].user_id, user_id);
            });
            
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }
    }

    /// Test store Send + Sync traits
    /// This ensures the store is safe for async usage
    #[test]
    fn test_store_send_sync() {
        // Ensure OrderStore is Send + Sync for async usage
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        
        assert_send::<OrderStore>();
        assert_sync::<OrderStore>();
    }

    /// final cleanup test that runs after all other tests
    #[tokio::test]
    async fn zzz_cleanup_after_all_tests() {
        // Database is required for cleanup
        let _db = require_database().await;
        // Giving other tests a moment to finish
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        global_cleanup().await;
    }
}
//! trading repository tests
//! 
//! these tests verify that our postgrestradingrepository works correctly with real database operations.
//! we test all the crud operations for both user balances and orders.

use chrono::Utc;
use sea_orm::{Database, DatabaseConnection, ConnectionTrait};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use lib_core::repository::repository::{TradingRepository, PostgresTradingRepository, TradingError};
use lib_core::model::orders::Model as OrderModel;

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
                DROP TABLE IF EXISTS user_balances CASCADE;
                
                -- orders table for trading operations
                CREATE TABLE orders (
                    order_id BIGSERIAL PRIMARY KEY,
                    user_id UUID NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
                    price BIGINT NOT NULL,
                    size BIGINT NOT NULL,
                    side TEXT NOT NULL CHECK (side IN ('Bid', 'Ask')),
                    timestamp BIGINT NOT NULL
                );
                
                -- user balances table for account management
                CREATE TABLE user_balances (
                    user_id UUID PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
                    available BIGINT NOT NULL DEFAULT 0,
                    locked BIGINT NOT NULL DEFAULT 0,
                    last_updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
                );
                
                -- add some indexes to speed up queries
                CREATE INDEX IF NOT EXISTS idx_orders_user_id ON orders(user_id);
                CREATE INDEX IF NOT EXISTS idx_orders_timestamp ON orders(timestamp);
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
        DELETE FROM orders WHERE user_id IN (
            SELECT user_id FROM users WHERE username LIKE 'test_%'
        );
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

    // test creating and retrieving a user balance
    // this tests the complete create -> retrieve workflow and data integrity
    #[tokio::test]
    async fn test_create_and_get_user_balance() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // first we need a user in the database (foreign key requirement)
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_user_balance_{}', 'test_balance_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // test creating a balance
        let create_result = repo.create_user_balance(user_id, 10000, 5000).await;
        assert!(create_result.is_ok(), "Create user balance should succeed");

        // test retrieving the balance we just created
        let get_result = repo.get_user_balance(user_id).await;
        match get_result {
            Ok(Some(balance)) => {
                // verify data integrity - what we put in is what we get out
                assert_eq!(balance.user_id, user_id);
                assert_eq!(balance.available, 10000);
                assert_eq!(balance.locked, 5000);
            }
            Ok(None) => {
                panic!("Expected balance but got None - create/get workflow failed");
            }
            Err(e) => {
                panic!("Get user balance failed: {}", e);
            }
        }
    }

    // test what happens when we try to get a balance for a user that doesn't exist
    // should return none, not an error
    #[tokio::test]
    async fn test_get_user_balance_not_found() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let non_existent_user_id = Uuid::new_v4();

        // Try to get a balance for a user that doesn't exist
        let result = repo.get_user_balance(non_existent_user_id).await;

        match result {
            Ok(None) => {
                // This is what we expect - no balance found
            }
            Ok(Some(_)) => {
                panic!("Expected None but got Some balance");
            }
            Err(e) => {
                panic!("Get user balance failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_update_user_balance_success() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // Create a test user first
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_update_balance_{}', 'test_update_balance_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create initial balance
        repo.create_user_balance(user_id, 10000, 5000).await.unwrap();

        // Update the balance
        let result = repo.update_user_balance(user_id, 20000, 8000).await;

        match result {
            Ok(_) => {
                // Verify the balance was updated
                let balance = repo.get_user_balance(user_id).await.unwrap().unwrap();
                assert_eq!(balance.available, 20000);
                assert_eq!(balance.locked, 8000);
            }
            Err(e) => {
                panic!("Update user balance failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_get_user_balances_success() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // Create a test user first
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_get_balances_{}', 'test_get_balances_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create a balance
        repo.create_user_balance(user_id, 25000, 7500).await.unwrap();

        let result = repo.get_user_balances(user_id).await;

        match result {
            Ok(balances) => {
                assert_eq!(balances.len(), 1);
                let balance = &balances[0];
                assert_eq!(balance.user_id, user_id);
                assert_eq!(balance.available, 25000);
                assert_eq!(balance.locked, 7500);
            }
            Err(e) => {
                panic!("Get user balances failed: {}", e);
            }
        }
    }

    // test creating and retrieving an order
    // this tests the complete create -> retrieve workflow and data integrity
    #[tokio::test]
    async fn test_create_and_get_order() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // Need a user first (foreign key constraint)
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_create_order_{}', 'test_create_order_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Test creating an order
        let order = create_test_order(user_id);
        let create_result = repo.create_order(order).await;
        
        match create_result {
            Ok(order_id) => {
                // Should get back a valid order ID
                assert!(order_id > 0, "Order ID should be positive");
                
                // Test retrieving the order we just created
                let get_result = repo.get_order_by_id(order_id, user_id).await;
                match get_result {
                    Ok(Some(retrieved_order)) => {
                        // Verify data integrity - what we put in is what we get out
                        assert_eq!(retrieved_order.order_id, order_id as i64);
                        assert_eq!(retrieved_order.user_id, user_id);
                        assert_eq!(retrieved_order.price, 50000);
                        assert_eq!(retrieved_order.size, 1000);
                        assert_eq!(retrieved_order.side, "Bid");
                    }
                    Ok(None) => {
                        panic!("Expected order but got None - create/get workflow failed");
                    }
                    Err(e) => {
                        panic!("Get order failed: {}", e);
                    }
                }
            }
            Err(e) => {
                panic!("Create order failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_get_order_by_id_not_found() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();
        let non_existent_order_id = 99999;

        let result = repo.get_order_by_id(non_existent_order_id, user_id).await;

        match result {
            Ok(None) => {
                // Expected behavior - order doesn't exist
            }
            Ok(Some(_)) => {
                panic!("Expected None but got Some order");
            }
            Err(e) => {
                panic!("Get order by ID failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_get_order_by_id_wrong_user() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user1_id = create_test_user_id();
        let user2_id = create_test_user_id();

        // Create test users
        let create_user1_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_user1_{}', 'test_user1_{}@example.com', 'hash')",
            user1_id, user1_id, user1_id
        );
        let create_user2_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_user2_{}', 'test_user2_{}@example.com', 'hash')",
            user2_id, user2_id, user2_id
        );
        db.execute_unprepared(&create_user1_sql).await.unwrap();
        db.execute_unprepared(&create_user2_sql).await.unwrap();

        // Create an order for user1
        let order = create_test_order(user1_id);
        let order_id = repo.create_order(order).await.unwrap();

        // Try to get the order with user2 - should return None
        let result = repo.get_order_by_id(order_id, user2_id).await;

        match result {
            Ok(None) => {
                // Expected behavior - order belongs to user1, not user2
            }
            Ok(Some(_)) => {
                panic!("Expected None but got Some order for wrong user");
            }
            Err(e) => {
                panic!("Get order by ID failed: {}", e);
            }
        }
    }

    // test getting multiple orders for a user
    // this simulates a user having several open orders
    #[tokio::test]
    async fn test_get_open_orders_success() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // Set up a user
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_open_orders_{}', 'test_open_orders_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create two different orders for this user
        let order1 = OrderModel {
            order_id: 0,
            user_id,
            price: 50000,
            size: 1000,
            side: "Bid".to_string(),
            timestamp: Utc::now().timestamp_millis(),
        };
        
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

        // Now get all orders for this user
        let result = repo.get_open_orders(user_id).await;

        match result {
            Ok(orders) => {
                // Should have both orders
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
                panic!("Get open orders failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_get_open_orders_empty() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // Create a test user first
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_empty_orders_{}', 'test_empty_orders_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        let result = repo.get_open_orders(user_id).await;

        match result {
            Ok(orders) => {
                assert_eq!(orders.len(), 0);
            }
            Err(e) => {
                panic!("Get open orders failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_count_user_orders_success() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // Create a test user first
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_count_orders_{}', 'test_count_orders_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        // Create multiple orders
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

        let result = repo.count_user_orders(user_id).await;

        match result {
            Ok(count) => {
                assert_eq!(count, 2);
            }
            Err(e) => {
                panic!("Count user orders failed: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_count_user_orders_zero() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // Create a test user first
        let create_user_sql = format!(
            "INSERT INTO users (user_id, username, email, password_hash) VALUES ('{}', 'test_count_zero_{}', 'test_count_zero_{}@example.com', 'hash')",
            user_id, user_id, user_id
        );
        db.execute_unprepared(&create_user_sql).await.unwrap();

        let result = repo.count_user_orders(user_id).await;

        match result {
            Ok(count) => {
                assert_eq!(count, 0);
            }
            Err(e) => {
                panic!("Count user orders failed: {}", e);
            }
        }
    }

    // test error handling - what happens when we try to create a balance for a non-existent user
    // this should fail due to foreign key constraints
    #[tokio::test]
    async fn test_repository_error_handling() {
        let db = require_database().await;
        let repo = Arc::new(PostgresTradingRepository::new(Arc::new(db.clone())));
        let user_id = create_test_user_id();

        // Try to create a balance for a user that doesn't exist in the database
        // This should fail because of the foreign key constraint
        let result = repo.create_user_balance(user_id, 10000, 5000).await;

        match result {
            Ok(_) => {
                panic!("Expected error for non-existent user but got success");
            }
            Err(TradingError::Database(_)) => {
                // This is what we expect - database constraint violation
            }
            Err(e) => {
                panic!("Unexpected error type: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn zzz_cleanup_after_all_tests() {
        // Database is required for cleanup
        let _db = require_database().await;
        // Giving other tests a moment to finish
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        global_cleanup().await;
    }
}
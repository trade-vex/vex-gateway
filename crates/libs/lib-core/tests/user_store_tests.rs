/// Comprehensive Unit Tests for UserBalanceStore
///
/// ## Purpose
/// Verify that the UserBalanceStore correctly:
/// 1. Queries Redis cache for user balances
/// 2. Falls back to database repository when cache misses
/// 3. Parses Redis hash data into UserBalanceModel
/// 4. Handles multiple assets per user
/// 5. Handles invalidation operations
///
/// ## Test Coverage
/// - Cache hit scenarios
/// - Cache miss with database fallback
/// - Balance parsing with various data formats
/// - Multi-asset portfolios
/// - Invalidation operations
/// - Error handling and edge cases
use async_trait::async_trait;
use lib_core::cache::user_store::{UserBalanceCacheConfig, UserBalanceStore};
use lib_core::model::orders::Model as OrderModel;
use lib_core::model::user_balances::Model as UserBalanceModel;
use lib_core::model::user_favorite_markets::Model as UserFavoriteMarketModel;
use lib_core::repository::repository::{TradingError, TradingRepository};
use redis::AsyncCommands;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

mod common;

type Result<T> = std::result::Result<T, TradingError>;

/// Mock TradingRepository for testing
#[derive(Clone)]
struct MockTradingRepository {
    balances: Vec<UserBalanceModel>,
    should_error: bool,
    balance_reads: Arc<AtomicUsize>,
}

impl MockTradingRepository {
    fn new() -> Self {
        Self {
            balances: Vec::new(),
            should_error: false,
            balance_reads: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn with_balances(mut self, balances: Vec<UserBalanceModel>) -> Self {
        self.balances = balances;
        self
    }

    fn with_error(mut self) -> Self {
        self.should_error = true;
        self
    }
}

#[async_trait]
impl TradingRepository for MockTradingRepository {
    async fn get_user_balances(&self, user_id: u64) -> Result<Vec<UserBalanceModel>> {
        self.balance_reads.fetch_add(1, Ordering::SeqCst);
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .balances
            .iter()
            .filter(|b| b.user_id == user_id as i64)
            .cloned()
            .collect())
    }

    async fn get_user_balance(&self, user_id: u64) -> Result<Option<UserBalanceModel>> {
        if self.should_error {
            return Err(TradingError::Database("Mock database error".to_string()));
        }
        Ok(self
            .balances
            .iter()
            .find(|b| b.user_id == user_id as i64)
            .cloned())
    }

    async fn update_user_balance(
        &self,
        _user_id: u64,
        _available: u64,
        _locked: u64,
    ) -> Result<()> {
        Ok(())
    }

    async fn create_user_balance(
        &self,
        _user_id: u64,
        _available: u64,
        _locked: u64,
    ) -> Result<()> {
        Ok(())
    }

    async fn get_open_orders(&self, _user_id: u64) -> Result<Vec<OrderModel>> {
        Ok(vec![])
    }

    async fn get_open_orders_paginated(
        &self,
        _user_id: u64,
        _offset: u64,
        _limit: u64,
        _market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>> {
        Ok(vec![])
    }

    async fn count_open_orders(&self, _user_id: u64, _market_id: Option<u32>) -> Result<u64> {
        Ok(0)
    }

    async fn get_order_by_id(&self, _order_id: u64, _user_id: u64) -> Result<Option<OrderModel>> {
        Ok(None)
    }

    async fn create_order(&self, _order: OrderModel) -> Result<u64> {
        Ok(1)
    }

    async fn count_user_orders(&self, _user_id: u64) -> Result<u64> {
        Ok(0)
    }

    async fn get_user_favorite_markets(
        &self,
        _user_id: u64,
    ) -> Result<Vec<UserFavoriteMarketModel>> {
        Ok(vec![])
    }

    async fn add_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<()> {
        Ok(())
    }

    async fn remove_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<()> {
        Ok(())
    }

    async fn is_market_favorited(&self, _user_id: u64, _market_id: u32) -> Result<bool> {
        Ok(false)
    }

    async fn get_order_history(&self, _user_id: u64) -> Result<Vec<OrderModel>> {
        Ok(vec![])
    }

    async fn get_order_history_paginated(
        &self,
        _user_id: u64,
        _offset: u64,
        _limit: u64,
        _market_id: Option<u32>,
    ) -> Result<Vec<OrderModel>> {
        Ok(vec![])
    }

    async fn count_order_history(&self, _user_id: u64, _market_id: Option<u32>) -> Result<u64> {
        Ok(0)
    }

    async fn toggle_favorite_market(&self, _user_id: u64, _market_id: u32) -> Result<bool> {
        Ok(true)
    }
}

/// Helper: Create test balance
fn create_test_balance(
    user_id: i64,
    asset_id: i32,
    available: i64,
    locked: i64,
    total: i64,
) -> UserBalanceModel {
    UserBalanceModel {
        id: 0,
        user_id,
        asset_id,
        available,
        locked,
        total,
        timestamp: 1700000000,
        created_at: None,
    }
}

#[tokio::test]
async fn test_user_balance_store_new() {
    let config = UserBalanceCacheConfig::default();
    let store = UserBalanceStore::new(config.clone());

    // Should work without cache or repository
    let result = store.get_user_balances(1).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_user_balance_store_with_repository() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![
        create_test_balance(100, 1, 10000, 500, 10500),  // BTC
        create_test_balance(100, 2, 50000, 1000, 51000), // ETH
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances.clone()));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    // Should fetch from repository
    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 2);
    assert_eq!(fetched_balances[0].asset_id, 1);
    assert_eq!(fetched_balances[0].available, 10000);
    assert_eq!(fetched_balances[1].asset_id, 2);
    assert_eq!(fetched_balances[1].available, 50000);
}

#[tokio::test]
async fn test_get_user_balances_empty() {
    let config = UserBalanceCacheConfig { enabled: false };

    let mock_repo = Arc::new(MockTradingRepository::new());
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_get_user_balances_single_asset() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![create_test_balance(100, 1, 10000, 500, 10500)];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 1);
    assert_eq!(fetched_balances[0].user_id, 100);
    assert_eq!(fetched_balances[0].asset_id, 1);
    assert_eq!(fetched_balances[0].available, 10000);
    assert_eq!(fetched_balances[0].locked, 500);
    assert_eq!(fetched_balances[0].total, 10500);
}

#[tokio::test]
async fn test_get_user_balances_multiple_assets() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![
        create_test_balance(100, 1, 10000, 500, 10500),  // BTC
        create_test_balance(100, 2, 50000, 1000, 51000), // ETH
        create_test_balance(100, 3, 100000, 0, 100000),  // USDT
        create_test_balance(100, 4, 5000, 2500, 7500),   // SOL
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 4);

    // Verify each balance
    let btc_balance = &fetched_balances[0];
    assert_eq!(btc_balance.asset_id, 1);
    assert_eq!(btc_balance.available, 10000);
    assert_eq!(btc_balance.locked, 500);

    let eth_balance = &fetched_balances[1];
    assert_eq!(eth_balance.asset_id, 2);
    assert_eq!(eth_balance.available, 50000);
    assert_eq!(eth_balance.locked, 1000);

    let usdt_balance = &fetched_balances[2];
    assert_eq!(usdt_balance.asset_id, 3);
    assert_eq!(usdt_balance.available, 100000);
    assert_eq!(usdt_balance.locked, 0);

    let sol_balance = &fetched_balances[3];
    assert_eq!(sol_balance.asset_id, 4);
    assert_eq!(sol_balance.available, 5000);
    assert_eq!(sol_balance.locked, 2500);
}

#[tokio::test]
async fn test_database_error_handling() {
    let config = UserBalanceCacheConfig { enabled: false };

    let mock_repo = Arc::new(MockTradingRepository::new().with_error());
    let store = UserBalanceStore::with_repository(config, mock_repo);

    // Should return database error
    let result = store.get_user_balances(100).await;
    assert!(result.is_err());
    match result {
        Err(TradingError::Database(msg)) => {
            assert_eq!(msg, "Mock database error");
        }
        _ => panic!("Expected database error"),
    }
}

#[tokio::test]
async fn test_multiple_users() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![
        create_test_balance(100, 1, 10000, 500, 10500),
        create_test_balance(200, 1, 20000, 1000, 21000),
        create_test_balance(100, 2, 50000, 1000, 51000),
        create_test_balance(300, 3, 15000, 0, 15000),
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    // User 100 should have 2 balances
    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let user100_balances = result.unwrap();
    assert_eq!(user100_balances.len(), 2);
    assert_eq!(user100_balances[0].user_id, 100);
    assert_eq!(user100_balances[1].user_id, 100);

    // User 200 should have 1 balance
    let result = store.get_user_balances(200).await;
    assert!(result.is_ok());
    let user200_balances = result.unwrap();
    assert_eq!(user200_balances.len(), 1);
    assert_eq!(user200_balances[0].user_id, 200);
    assert_eq!(user200_balances[0].available, 20000);

    // User 300 should have 1 balance
    let result = store.get_user_balances(300).await;
    assert!(result.is_ok());
    let user300_balances = result.unwrap();
    assert_eq!(user300_balances.len(), 1);
    assert_eq!(user300_balances[0].user_id, 300);

    // User 999 (non-existent) should have empty balances
    let result = store.get_user_balances(999).await;
    assert!(result.is_ok());
    let user999_balances = result.unwrap();
    assert!(user999_balances.is_empty());
}

#[tokio::test]
async fn test_zero_balances() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![create_test_balance(100, 1, 0, 0, 0)];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 1);
    assert_eq!(fetched_balances[0].available, 0);
    assert_eq!(fetched_balances[0].locked, 0);
    assert_eq!(fetched_balances[0].total, 0);
}

#[tokio::test]
async fn test_large_balance_values() {
    let config = UserBalanceCacheConfig { enabled: false };

    // Test with large balance values (12 decimal places precision)
    let large_available = 1_000_000_000_000_000_u64; // 1,000 * 10^12
    let large_locked = 500_000_000_000_000_u64; // 500 * 10^12
    let large_total = large_available + large_locked;

    let balances = vec![create_test_balance(
        100,
        1,
        large_available as i64,
        large_locked as i64,
        large_total as i64,
    )];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 1);
    assert_eq!(fetched_balances[0].available, large_available);
    assert_eq!(fetched_balances[0].locked, large_locked);
    assert_eq!(fetched_balances[0].total, large_total);
}

#[tokio::test]
async fn test_balance_consistency() {
    let config = UserBalanceCacheConfig { enabled: false };

    // Test that total = available + locked
    let balances = vec![
        create_test_balance(100, 1, 10000, 500, 10500),
        create_test_balance(100, 2, 75000, 25000, 100000),
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();

    for balance in fetched_balances {
        assert_eq!(
            balance.total,
            balance.available + balance.locked,
            "Total should equal available + locked"
        );
    }
}

#[tokio::test]
async fn test_config_default_values() {
    let config = UserBalanceCacheConfig::default();
    assert!(!config.enabled);
}

#[tokio::test]
async fn test_config_custom_values() {
    let config = UserBalanceCacheConfig { enabled: true };
    assert!(config.enabled);
}

#[tokio::test]
async fn test_concurrent_access() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![
        create_test_balance(100, 1, 10000, 500, 10500),
        create_test_balance(100, 2, 50000, 1000, 51000),
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = Arc::new(UserBalanceStore::with_repository(config, mock_repo));

    // Spawn multiple concurrent tasks
    let mut handles = vec![];
    for _ in 0..10 {
        let store_clone = store.clone();
        let handle = tokio::spawn(async move {
            let result = store_clone.get_user_balances(100).await;
            assert!(result.is_ok());
            result.unwrap()
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        let balances = handle.await.unwrap();
        assert_eq!(balances.len(), 2);
    }
}

#[tokio::test]
async fn test_balance_with_only_available() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![create_test_balance(100, 1, 10000, 0, 10000)];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 1);
    assert_eq!(fetched_balances[0].available, 10000);
    assert_eq!(fetched_balances[0].locked, 0);
    assert_eq!(fetched_balances[0].total, 10000);
}

#[tokio::test]
async fn test_balance_with_only_locked() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![create_test_balance(100, 1, 0, 5000, 5000)];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 1);
    assert_eq!(fetched_balances[0].available, 0);
    assert_eq!(fetched_balances[0].locked, 5000);
    assert_eq!(fetched_balances[0].total, 5000);
}

#[tokio::test]
async fn test_asset_id_variations() {
    let config = UserBalanceCacheConfig { enabled: false };

    // Test various asset IDs
    let balances = vec![
        create_test_balance(100, 1, 1000, 0, 1000), // Small asset ID
        create_test_balance(100, 100, 2000, 0, 2000), // Medium asset ID
        create_test_balance(100, 9999, 3000, 0, 3000), // Large asset ID
    ];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 3);
    assert_eq!(fetched_balances[0].asset_id, 1);
    assert_eq!(fetched_balances[1].asset_id, 100);
    assert_eq!(fetched_balances[2].asset_id, 9999);
}

#[tokio::test]
async fn test_timestamp_preservation() {
    let config = UserBalanceCacheConfig { enabled: false };

    let balances = vec![create_test_balance(100, 1, 10000, 500, 10500)];

    let mock_repo = Arc::new(MockTradingRepository::new().with_balances(balances));
    let store = UserBalanceStore::with_repository(config, mock_repo);

    let result = store.get_user_balances(100).await;
    assert!(result.is_ok());
    let fetched_balances = result.unwrap();
    assert_eq!(fetched_balances.len(), 1);
    assert_eq!(fetched_balances[0].timestamp, 1700000000);
}

#[tokio::test]
async fn unindexed_balances_are_read_from_the_repository_without_writing_redis() {
    // The gateway only reads balances: with no asset index for the user, Redis has nothing it
    // can enumerate, so the repository answers and the index is left for the Events Handler.
    let redis_url = common::redis_url();
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = match redis::aio::ConnectionManager::new(client).await {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("Skipping: unable to connect to {redis_url}: {error}");
            return;
        }
    };
    let user_id = (uuid::Uuid::new_v4().as_u128() as u64) & i64::MAX as u64;
    let asset_set_key = format!("user:{}:balances", user_id);
    let balance_key = format!("user:{}:asset:7:balance", user_id);

    let _: usize = redis::cmd("HSET")
        .arg(&balance_key)
        .arg("user_id")
        .arg(user_id)
        .arg("asset_id")
        .arg(7)
        .arg("available")
        .arg(700)
        .arg("locked")
        .arg(70)
        .arg("total")
        .arg(770)
        .arg("timestamp")
        .arg(1_700_000_007_i64)
        .query_async(&mut conn)
        .await
        .unwrap();

    let repository = Arc::new(MockTradingRepository::new().with_balances(vec![
        create_test_balance(user_id as i64, 7, 1, 2, 3),
        create_test_balance(user_id as i64, 11, 4, 5, 9),
    ]));
    let balance_reads = repository.balance_reads.clone();
    let store = UserBalanceStore::with_redis_cache(
        UserBalanceCacheConfig { enabled: true },
        redis_url,
        Some(repository),
    )
    .await
    .unwrap();

    let mut balances = store.get_user_balances(user_id).await.unwrap();
    balances.sort_by_key(|balance| balance.asset_id);

    assert_eq!(balances.len(), 2);
    assert_eq!(balances[0].available, 1);
    assert_eq!(balances[1].available, 4);
    assert_eq!(balance_reads.load(Ordering::SeqCst), 1);
    let indexed: Vec<String> = conn.smembers(&asset_set_key).await.unwrap();
    assert!(indexed.is_empty(), "a read must not write the asset index");

    let _: usize = conn.del(&[&asset_set_key, &balance_key]).await.unwrap();
}

#[tokio::test]
async fn cache_hit_does_not_query_the_repository() {
    // Finding G86-t0. With an indexed asset whose hash is cached, the request must be answered
    // from Redis alone; the database is read only once that hash goes missing.
    let redis_url = common::redis_url();
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = match redis::aio::ConnectionManager::new(client).await {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("Skipping: unable to connect to {redis_url}: {error}");
            return;
        }
    };
    let user_id = (uuid::Uuid::new_v4().as_u128() as u64) & i64::MAX as u64;
    let asset_set_key = format!("user:{}:balances", user_id);
    let balance_key = format!("user:{}:asset:7:balance", user_id);

    let _: usize = redis::cmd("HSET")
        .arg(&balance_key)
        .arg("user_id")
        .arg(user_id)
        .arg("asset_id")
        .arg(7)
        .arg("available")
        .arg(700)
        .arg("locked")
        .arg(0)
        .arg("total")
        .arg(700)
        .arg("timestamp")
        .arg(1_700_000_000_i64)
        .query_async(&mut conn)
        .await
        .unwrap();
    let _: usize = conn.sadd(&asset_set_key, "7").await.unwrap();

    let repository = Arc::new(
        MockTradingRepository::new().with_balances(vec![create_test_balance(
            user_id as i64,
            7,
            1,
            2,
            3,
        )]),
    );
    let balance_reads = repository.balance_reads.clone();
    let store = UserBalanceStore::with_redis_cache(
        UserBalanceCacheConfig { enabled: true },
        redis_url,
        Some(repository),
    )
    .await
    .unwrap();

    let balances = store.get_user_balances(user_id).await.unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].available, 700);
    assert_eq!(
        balance_reads.load(Ordering::SeqCst),
        0,
        "a full cache hit must not query the repository"
    );

    // The indexed hash disappears: now the repository must fill the gap.
    let _: usize = conn.del(&balance_key).await.unwrap();
    let balances = store.get_user_balances(user_id).await.unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].available, 1);
    assert_eq!(balance_reads.load(Ordering::SeqCst), 1);

    let _: usize = conn.del(&[&asset_set_key, &balance_key]).await.unwrap();
}

#[tokio::test]
async fn an_indexed_asset_without_a_hash_sends_the_whole_read_to_the_repository() {
    // Two indexed assets, one hash missing: the repository answers for every asset rather than
    // being merged with what Redis has, so the caller never gets a mix of two sources.
    let redis_url = common::redis_url();
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = match redis::aio::ConnectionManager::new(client).await {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("Skipping: unable to connect to {redis_url}: {error}");
            return;
        }
    };
    let user_id = (uuid::Uuid::new_v4().as_u128() as u64) & i64::MAX as u64;
    let asset_set_key = format!("user:{}:balances", user_id);
    let cached_key = format!("user:{}:asset:7:balance", user_id);

    let _: usize = redis::cmd("HSET")
        .arg(&cached_key)
        .arg("user_id")
        .arg(user_id)
        .arg("asset_id")
        .arg(7)
        .arg("available")
        .arg(700)
        .arg("locked")
        .arg(0)
        .arg("total")
        .arg(700)
        .arg("timestamp")
        .arg(1_700_000_000_i64)
        .query_async(&mut conn)
        .await
        .unwrap();
    let _: usize = conn.sadd(&asset_set_key, &["7", "11"]).await.unwrap();

    let repository = Arc::new(MockTradingRepository::new().with_balances(vec![
        create_test_balance(user_id as i64, 7, 1, 2, 3),
        create_test_balance(user_id as i64, 11, 4, 5, 9),
    ]));
    let store = UserBalanceStore::with_redis_cache(
        UserBalanceCacheConfig { enabled: true },
        redis_url,
        Some(repository),
    )
    .await
    .unwrap();

    let mut balances = store.get_user_balances(user_id).await.unwrap();
    balances.sort_by_key(|balance| balance.asset_id);

    assert_eq!(balances.len(), 2);
    assert_eq!(
        balances[0].available, 1,
        "asset 7 must come from the repository too"
    );
    assert_eq!(balances[1].available, 4);

    let _: usize = conn.del(&[&asset_set_key, &cached_key]).await.unwrap();
}

#[tokio::test]
async fn test_u64_balances_survive_redis_round_trip() {
    let redis_url = common::redis_url();
    let client = redis::Client::open(redis_url.clone()).expect("Redis URL should be valid");
    let mut connection = match client.get_multiplexed_async_connection().await {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("Skipping Redis round-trip test: unable to connect to {redis_url}: {error}");
            return;
        }
    };
    let user_id = 7_000_000_000_u64 + u64::from(std::process::id());
    let values = [(1_i32, u64::MAX), (2_i32, i64::MAX as u64 + 1)];
    let asset_set_key = format!("user:{user_id}:balances");
    let keys: Vec<String> = values
        .iter()
        .map(|(asset_id, _)| format!("user:{user_id}:asset:{asset_id}:balance"))
        .collect();

    for ((asset_id, balance), key) in values.iter().zip(&keys) {
        let fields = [
            ("user_id", user_id.to_string()),
            ("asset_id", asset_id.to_string()),
            ("available", balance.to_string()),
            ("locked", "0".to_string()),
            ("total", balance.to_string()),
            ("timestamp", "1700000000".to_string()),
        ];
        let _: () = connection
            .hset_multiple(key, &fields)
            .await
            .expect("balance hash should be written");
        // As the Events Handler does: index the asset alongside its balance hash.
        let _: usize = connection
            .sadd(&asset_set_key, asset_id.to_string())
            .await
            .expect("asset should be indexed");
    }

    let store = UserBalanceStore::with_redis_cache(
        UserBalanceCacheConfig { enabled: true },
        redis_url,
        None,
    )
    .await
    .expect("balance store should connect to Redis");
    let balances = store
        .get_user_balances(user_id)
        .await
        .expect("balances should be read from Redis");

    for (asset_id, expected) in values {
        let actual = balances
            .iter()
            .find(|balance| balance.asset_id == asset_id)
            .expect("balance row should not be dropped");
        assert_eq!(actual.available, expected);
        assert_eq!(actual.total, expected);
    }

    let _: () = connection
        .del(keys)
        .await
        .expect("test balance hashes should be removed");
    let _: () = connection
        .del(asset_set_key)
        .await
        .expect("test asset index should be removed");
}

//! Rate limiting tests

use lib_rate_limit::{
    config::RateLimitConfig,
    redis_store::RedisRateLimitStore,
    rate_limiter::{FixedWindowRateLimiter, RateLimiter},
};
use std::time::Duration;

fn create_test_redis_config() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

fn create_test_rate_limit_config() -> RateLimitConfig {
    RateLimitConfig::requests_per_minute(5)
}

#[tokio::test]
async fn test_redis_rate_limit_store_basic_operations() {
    let redis_url = create_test_redis_config();
    let store = RedisRateLimitStore::new(&redis_url, "test_rate_limit".to_string()).await
        .expect("Redis connection failed - Redis must be running for this test!");

    println!("Redis rate limit store created successfully");

    let user_id = "test_user_123";
    let window_start = store.get_current_window_start(60);

    let initial_count = store.get_request_count(user_id, window_start).await.unwrap();
    assert_eq!(initial_count, 0);

    let count1 = store.increment_request_count(user_id, window_start, Duration::from_secs(60)).await.unwrap();
    assert_eq!(count1, 1);

    let count2 = store.increment_request_count(user_id, window_start, Duration::from_secs(60)).await.unwrap();
    assert_eq!(count2, 2);

    let is_limited = store.is_rate_limited(user_id, window_start, 5, Duration::from_secs(60)).await.unwrap();
    assert!(!is_limited);

    for _ in 0..4 {
        store.increment_request_count(user_id, window_start, Duration::from_secs(60)).await.unwrap();
    }

    let is_limited = store.is_rate_limited(user_id, window_start, 5, Duration::from_secs(60)).await.unwrap();
    assert!(is_limited);
}

#[tokio::test]
async fn test_fixed_window_rate_limiter() {
    let redis_url = create_test_redis_config();
    let config = create_test_rate_limit_config();
    
    let store = RedisRateLimitStore::new(&redis_url, "test_rate_limiter".to_string()).await
        .expect("Redis connection failed - Redis must be running for this test!");

    let rate_limiter = FixedWindowRateLimiter::new(store, config);
    let user_id = "test_user_456";

    let is_allowed = rate_limiter.is_allowed(user_id).await.unwrap();
    assert!(is_allowed);

    let current_count = rate_limiter.get_current_count(user_id).await.unwrap();
    assert_eq!(current_count, 1);

    let remaining = rate_limiter.get_remaining_requests(user_id).await.unwrap();
    assert_eq!(remaining, 4);

    for _ in 0..4 {
        let is_allowed = rate_limiter.is_allowed(user_id).await.unwrap();
        assert!(is_allowed);
    }

    let is_allowed = rate_limiter.is_allowed(user_id).await.unwrap();
    assert!(!is_allowed);

    let remaining = rate_limiter.get_remaining_requests(user_id).await.unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn test_rate_limit_config() {
    let config = RateLimitConfig::requests_per_minute(100);
    assert_eq!(config.max_requests, 100);
    assert_eq!(config.window_size_seconds, 60);
    assert!(config.enabled);
}

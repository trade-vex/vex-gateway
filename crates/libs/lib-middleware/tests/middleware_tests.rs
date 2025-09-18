use lib_middleware::api_key::{
    middleware::{ApiKeyStore, CacheConfig, CacheMetrics, RouteSecurityConfig},
    security::{SecurityType, ApiKeyConfig},
};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn test_route_security_config_creation() {
    let config = RouteSecurityConfig {
        path_pattern: "/api/v1/orders".to_string(),
        security_type: SecurityType::Trade,
    };
    
    assert_eq!(config.path_pattern, "/api/v1/orders");
    assert_eq!(config.security_type, SecurityType::Trade);
}

#[test]
fn test_route_security_config_clone() {
    let original = RouteSecurityConfig {
        path_pattern: "/api/v1/user".to_string(),
        security_type: SecurityType::UserData,
    };
    
    let cloned = original.clone();
    assert_eq!(original.path_pattern, cloned.path_pattern);
    assert_eq!(original.security_type, cloned.security_type);
}

#[test]
fn test_route_security_config_debug() {
    let config = RouteSecurityConfig {
        path_pattern: "/api/v1/stream".to_string(),
        security_type: SecurityType::UserStream,
    };
    
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("/api/v1/stream"));
    assert!(debug_str.contains("UserStream"));
}

#[test]
fn test_cache_config_default() {
    let config = CacheConfig::default();
    
    assert_eq!(config.l1_max_size, 100_000);
    assert_eq!(config.l1_ttl, Duration::from_secs(300)); // 5 minutes
}

#[test]
fn test_cache_config_custom() {
    let config = CacheConfig {
        l1_max_size: 2000,
        l1_ttl: Duration::from_secs(600),
        l2_ttl: Duration::from_secs(1800),
        l2_enabled: false,
    };
    
    assert_eq!(config.l1_max_size, 2000);
    assert_eq!(config.l1_ttl, Duration::from_secs(600));
}

#[test]
fn test_cache_config_clone() {
    let config = CacheConfig {
        l1_max_size: 500,
        l1_ttl: Duration::from_secs(120),
        l2_ttl: Duration::from_secs(1800),
        l2_enabled: false,
    };
    
    let cloned = config.clone();
    assert_eq!(config.l1_max_size, cloned.l1_max_size);
    assert_eq!(config.l1_ttl, cloned.l1_ttl);
}

#[test]
fn test_cache_metrics_new() {
    let metrics = CacheMetrics::new();
    
    assert_eq!(metrics.l1_hits.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_eq!(metrics.l1_misses.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_eq!(metrics.db_queries.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn test_cache_metrics_operations() {
    let metrics = CacheMetrics::new();
    
    // Test increment operations
    metrics.l1_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    metrics.l1_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    metrics.db_queries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    
    assert_eq!(metrics.l1_hits.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(metrics.l1_misses.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(metrics.db_queries.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn test_cache_metrics_hit_ratios() {
    let metrics = CacheMetrics::new();
    
    // Add some hits and misses
    for _ in 0..7 {
        metrics.l1_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    for _ in 0..3 {
        metrics.l1_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    
    let l1_ratio = metrics.get_l1_hit_rate();
    
    assert!((l1_ratio - 0.7).abs() < 0.01); // 7/10 = 0.7
}

#[test]
fn test_cache_metrics_hit_ratio_no_requests() {
    let metrics = CacheMetrics::new();
    
    // With no requests, hit ratio should be 0.0
    assert_eq!(metrics.get_l1_hit_rate(), 0.0);
}

#[test]
fn test_cache_metrics_basic_usage() {
    let metrics = CacheMetrics::new();
    
    // Add some sample data
    metrics.l1_hits.fetch_add(2, std::sync::atomic::Ordering::Relaxed);
    metrics.l1_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    metrics.db_queries.fetch_add(2, std::sync::atomic::Ordering::Relaxed);
    
    assert_eq!(metrics.l1_hits.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert_eq!(metrics.l1_misses.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(metrics.db_queries.load(std::sync::atomic::Ordering::Relaxed), 2);
    
    let hit_rate = metrics.get_l1_hit_rate();
    assert!((hit_rate - 0.6666666666666666).abs() < 0.01); // 2/3
}

#[tokio::test]
async fn test_api_key_store_with_api_keys() {
    let api_key_config = ApiKeyConfig::new("test-key".to_string())
        .with_hmac_secret("secret".to_string())
        .with_permissions(vec![SecurityType::Trade]);
    
    let config = CacheConfig::default();
    let store = ApiKeyStore::new(config)
        .with_api_keys(vec![api_key_config.clone()]);
    
    // Verify the API key was added to memory store
    let result = store.get_api_key("test-key").await;
    assert!(result.is_some());
    
    let retrieved_config = result.unwrap();
    assert_eq!(retrieved_config.api_key, "test-key");
    assert_eq!(retrieved_config.permissions, vec![SecurityType::Trade]);
}

#[tokio::test]
async fn test_api_key_store_get_not_found() {
    let config = CacheConfig::default();
    let store = ApiKeyStore::new(config);
    
    let result = store.get_api_key("non-existent-key").await;
    assert!(result.is_none());
}

#[tokio::test]
async fn test_api_key_store_concurrent_access() {
    let config = CacheConfig::default();
    let store = Arc::new(ApiKeyStore::new(config));
    
    let mut handles = vec![];
    
    // Spawn multiple tasks that try to get the same non-existent key concurrently
    for i in 0..10 {
        let store_clone = Arc::clone(&store);
        let handle = tokio::spawn(async move {
            let key = format!("concurrent-key-{}", i);
            let result = store_clone.get_api_key(&key).await;
            assert!(result.is_none());
        });
        
        handles.push(handle);
    }
    
    // Wait for all tasks to complete
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_api_key_store_with_valid_api_key() {
    let api_key_config = ApiKeyConfig::new("valid-key".to_string())
        .with_hmac_secret("test-secret".to_string())
        .with_permissions(vec![SecurityType::Trade, SecurityType::UserData])
        .with_recv_window(10000)
        .with_description("Test API key".to_string());
    
    let config = CacheConfig::default();
    let store = ApiKeyStore::new(config)
        .with_api_keys(vec![api_key_config]);
    
    let result = store.get_api_key("valid-key").await;
    assert!(result.is_some());
    
    let retrieved = result.unwrap();
    assert_eq!(retrieved.api_key, "valid-key");
    assert_eq!(retrieved.recv_window, 10000);
    assert_eq!(retrieved.description, Some("Test API key".to_string()));
    assert!(retrieved.has_permission(SecurityType::Trade));
    assert!(retrieved.has_permission(SecurityType::UserData));
    assert!(!retrieved.has_permission(SecurityType::UserStream));
}

#[tokio::test]
async fn test_api_key_store_cache_behavior() {
    let api_key_config = ApiKeyConfig::new("cache-test".to_string())
        .with_hmac_secret("cache-secret".to_string());
    
    let config = CacheConfig {
        l1_max_size: 10,
        l1_ttl: Duration::from_secs(1), // Short TTL for testing
        l2_ttl: Duration::from_secs(1800),
        l2_enabled: false,
    };
    let store = ApiKeyStore::new(config)
        .with_api_keys(vec![api_key_config]);
    
    // First access should cache the key
    let result1 = store.get_api_key("cache-test").await;
    assert!(result1.is_some());
    
    // Second access should hit cache
    let result2 = store.get_api_key("cache-test").await;
    assert!(result2.is_some());
    
    // Both results should be the same
    assert_eq!(result1.unwrap().api_key, result2.unwrap().api_key);
}

#[test]
fn test_api_key_store_clone() {
    let config = CacheConfig::default();
    let store = ApiKeyStore::new(config);
    
    // Store should be cloneable (Arc-based internals)
    let _cloned_store = store.clone();
    
    // This test just ensures the clone compiles and works
}

#[test]
fn test_api_key_store_send_sync() {
    // Ensure ApiKeyStore is Send + Sync for async usage
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    
    assert_send::<ApiKeyStore>();
    assert_sync::<ApiKeyStore>();
}

#[test]
fn test_cache_metrics_send_sync() {
    // Ensure CacheMetrics is Send + Sync for async usage
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    
    assert_send::<CacheMetrics>();
    assert_sync::<CacheMetrics>();
}
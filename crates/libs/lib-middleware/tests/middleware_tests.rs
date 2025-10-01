// use lib_middleware::api_key::{
//     middleware::{ApiKeyStore, CacheConfig, CacheMetrics, RouteSecurityConfig},
//     redis_cache::{RedisApiKeyCache, RedisCacheConfig},
//     security::{SecurityType, ApiKeyConfig, KeyMaterial},
//     cache::ApiKeyCache,
// };
// use std::sync::Arc;
// use std::time::Duration;

// #[test]
// fn test_route_security_config_creation() {
//     let config = RouteSecurityConfig {
//         path_pattern: "/api/v1/orders".to_string(),
//         security_type: SecurityType::Trade,
//     };
    
//     assert_eq!(config.path_pattern, "/api/v1/orders");
//     assert_eq!(config.security_type, SecurityType::Trade);
// }

// #[test]
// fn test_route_security_config_clone() {
//     let original = RouteSecurityConfig {
//         path_pattern: "/api/v1/user".to_string(),
//         security_type: SecurityType::UserData,
//     };
    
//     let cloned = original.clone();
//     assert_eq!(original.path_pattern, cloned.path_pattern);
//     assert_eq!(original.security_type, cloned.security_type);
// }

// #[test]
// fn test_route_security_config_debug() {
//     let config = RouteSecurityConfig {
//         path_pattern: "/api/v1/stream".to_string(),
//         security_type: SecurityType::UserStream,
//     };
    
//     let debug_str = format!("{:?}", config);
//     assert!(debug_str.contains("/api/v1/stream"));
//     assert!(debug_str.contains("UserStream"));
// }

// #[test]
// fn test_cache_config_default() {
//     let config = CacheConfig::default();
    
//     assert_eq!(config.l1_max_size, 100_000);
//     assert_eq!(config.l1_ttl, Duration::from_secs(300)); // 5 minutes
// }

// #[test]
// fn test_cache_config_custom() {
//     let config = CacheConfig {
//         l1_max_size: 2000,
//         l1_ttl: Duration::from_secs(600),
//         l2_ttl: Duration::from_secs(1800),
//         l2_enabled: false,
//     };
    
//     assert_eq!(config.l1_max_size, 2000);
//     assert_eq!(config.l1_ttl, Duration::from_secs(600));
// }

// #[test]
// fn test_cache_config_clone() {
//     let config = CacheConfig {
//         l1_max_size: 500,
//         l1_ttl: Duration::from_secs(120),
//         l2_ttl: Duration::from_secs(1800),
//         l2_enabled: false,
//     };
    
//     let cloned = config.clone();
//     assert_eq!(config.l1_max_size, cloned.l1_max_size);
//     assert_eq!(config.l1_ttl, cloned.l1_ttl);
// }

// #[test]
// fn test_cache_metrics_new() {
//     let metrics = CacheMetrics::new();
    
//     assert_eq!(metrics.l1_hits.load(std::sync::atomic::Ordering::Relaxed), 0);
//     assert_eq!(metrics.l1_misses.load(std::sync::atomic::Ordering::Relaxed), 0);
//     assert_eq!(metrics.db_queries.load(std::sync::atomic::Ordering::Relaxed), 0);
// }

// #[test]
// fn test_cache_metrics_operations() {
//     let metrics = CacheMetrics::new();
    
//     // Test increment operations
//     metrics.l1_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
//     metrics.l1_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
//     metrics.db_queries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    
//     assert_eq!(metrics.l1_hits.load(std::sync::atomic::Ordering::Relaxed), 1);
//     assert_eq!(metrics.l1_misses.load(std::sync::atomic::Ordering::Relaxed), 1);
//     assert_eq!(metrics.db_queries.load(std::sync::atomic::Ordering::Relaxed), 1);
// }

// #[test]
// fn test_cache_metrics_hit_ratios() {
//     let metrics = CacheMetrics::new();
    
//     // Add some hits and misses
//     for _ in 0..7 {
//         metrics.l1_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
//     }
//     for _ in 0..3 {
//         metrics.l1_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
//     }
    
//     let l1_ratio = metrics.get_l1_hit_rate();
    
//     assert!((l1_ratio - 0.7).abs() < 0.01); // 7/10 = 0.7
// }

// #[test]
// fn test_cache_metrics_hit_ratio_no_requests() {
//     let metrics = CacheMetrics::new();
    
//     // With no requests, hit ratio should be 0.0
//     assert_eq!(metrics.get_l1_hit_rate(), 0.0);
// }

// #[test]
// fn test_cache_metrics_basic_usage() {
//     let metrics = CacheMetrics::new();
    
//     // Add some sample data
//     metrics.l1_hits.fetch_add(2, std::sync::atomic::Ordering::Relaxed);
//     metrics.l1_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
//     metrics.db_queries.fetch_add(2, std::sync::atomic::Ordering::Relaxed);
    
//     assert_eq!(metrics.l1_hits.load(std::sync::atomic::Ordering::Relaxed), 2);
//     assert_eq!(metrics.l1_misses.load(std::sync::atomic::Ordering::Relaxed), 1);
//     assert_eq!(metrics.db_queries.load(std::sync::atomic::Ordering::Relaxed), 2);
    
//     let hit_rate = metrics.get_l1_hit_rate();
//     assert!((hit_rate - 0.6666666666666666).abs() < 0.01); // 2/3
// }

// #[tokio::test]
// async fn test_api_key_store_with_api_keys() {
//     let api_key_config = ApiKeyConfig::new("test-key".to_string())
//         .with_hmac_secret("secret".to_string())
//         .with_permissions(vec![SecurityType::Trade]);
    
//     let config = CacheConfig::default();
//     let store = ApiKeyStore::new(config)
//         .with_api_keys(vec![api_key_config.clone()]);
    
//     // Verify the API key was added to memory store
//     let result = store.get_api_key("test-key").await;
//     assert!(result.is_some());
    
//     let retrieved_config = result.unwrap();
//     assert_eq!(retrieved_config.api_key, "test-key");
//     assert_eq!(retrieved_config.permissions, vec![SecurityType::Trade]);
// }

// #[tokio::test]
// async fn test_api_key_store_get_not_found() {
//     let config = CacheConfig::default();
//     let store = ApiKeyStore::new(config);
    
//     let result = store.get_api_key("non-existent-key").await;
//     assert!(result.is_none());
// }

// #[tokio::test]
// async fn test_api_key_store_concurrent_access() {
//     let config = CacheConfig::default();
//     let store = Arc::new(ApiKeyStore::new(config));
    
//     let mut handles = vec![];
    
//     // Spawn multiple tasks that try to get the same non-existent key concurrently
//     for i in 0..10 {
//         let store_clone = Arc::clone(&store);
//         let handle = tokio::spawn(async move {
//             let key = format!("concurrent-key-{}", i);
//             let result = store_clone.get_api_key(&key).await;
//             assert!(result.is_none());
//         });
        
//         handles.push(handle);
//     }
    
//     // Wait for all tasks to complete
//     for handle in handles {
//         handle.await.unwrap();
//     }
// }

// #[tokio::test]
// async fn test_api_key_store_with_valid_api_key() {
//     let api_key_config = ApiKeyConfig::new("valid-key".to_string())
//         .with_hmac_secret("test-secret".to_string())
//         .with_permissions(vec![SecurityType::Trade, SecurityType::UserData])
//         .with_recv_window(10000)
//         .with_description("Test API key".to_string());
    
//     let config = CacheConfig::default();
//     let store = ApiKeyStore::new(config)
//         .with_api_keys(vec![api_key_config]);
    
//     let result = store.get_api_key("valid-key").await;
//     assert!(result.is_some());
    
//     let retrieved = result.unwrap();
//     assert_eq!(retrieved.api_key, "valid-key");
//     assert_eq!(retrieved.recv_window, 10000);
//     assert_eq!(retrieved.description, Some("Test API key".to_string()));
//     assert!(retrieved.has_permission(SecurityType::Trade));
//     assert!(retrieved.has_permission(SecurityType::UserData));
//     assert!(!retrieved.has_permission(SecurityType::UserStream));
// }

// #[tokio::test]
// async fn test_api_key_store_cache_behavior() {
//     let api_key_config = ApiKeyConfig::new("cache-test".to_string())
//         .with_hmac_secret("cache-secret".to_string());
    
//     let config = CacheConfig {
//         l1_max_size: 10,
//         l1_ttl: Duration::from_secs(1), // Short TTL for testing
//         l2_ttl: Duration::from_secs(1800),
//         l2_enabled: false,
//     };
//     let store = ApiKeyStore::new(config)
//         .with_api_keys(vec![api_key_config]);
    
//     // First access should cache the key
//     let result1 = store.get_api_key("cache-test").await;
//     assert!(result1.is_some());
    
//     // Second access should hit cache
//     let result2 = store.get_api_key("cache-test").await;
//     assert!(result2.is_some());
    
//     // Both results should be the same
//     assert_eq!(result1.unwrap().api_key, result2.unwrap().api_key);
// }

// #[test]
// fn test_api_key_store_clone() {
//     let config = CacheConfig::default();
//     let store = ApiKeyStore::new(config);
    
//     // Store should be cloneable (Arc-based internals)
//     let _cloned_store = store.clone();
    
//     // This test just ensures the clone compiles and works
// }

// #[test]
// fn test_api_key_store_send_sync() {
//     // Ensure ApiKeyStore is Send + Sync for async usage
//     fn assert_send<T: Send>() {}
//     fn assert_sync<T: Sync>() {}
    
//     assert_send::<ApiKeyStore>();
//     assert_sync::<ApiKeyStore>();
// }

// #[test]
// fn test_cache_metrics_send_sync() {
//     // Ensure CacheMetrics is Send + Sync for async usage
//     fn assert_send<T: Send>() {}
//     fn assert_sync<T: Sync>() {}
    
//     assert_send::<CacheMetrics>();
//     assert_sync::<CacheMetrics>();
// }

// /// Tests for Redis L2 cache
// /// Build a sample API key config for testing
// fn create_test_api_key() -> ApiKeyConfig {
//     ApiKeyConfig::new("test_key_123".to_string())
//         .with_hmac_secret("test_secret".to_string())
//         .with_permissions(vec![SecurityType::Trade, SecurityType::UserData])
//         .with_recv_window(5000)
//         .with_enabled(true)
//         .with_description("Test API key for L2 cache".to_string())
// }

// /// Basic Redis config pointing to local instance
// fn create_test_redis_config() -> RedisCacheConfig {
//     RedisCacheConfig {
//         url: "redis://localhost:6379".to_string(),
//         key_prefix: "test_api_key:".to_string(),
//         max_connections: 5,
//         min_connections: 1,
//         connection_timeout_ms: 1000,
//         max_lifetime_secs: 300,
//         idle_timeout_secs: 60,
//         retry_count: 3,
//     }
// }

// /// Cache config with L2 enabled + short TTLs for quick tests
// fn create_l2_cache_config() -> CacheConfig {
//     CacheConfig {
//         l1_max_size: 10,
//         l1_ttl: Duration::from_secs(1),  // expire fast
//         l2_ttl: Duration::from_secs(30), // also short
//         l2_enabled: true,
//     }
// }

// #[tokio::test]
// async fn test_l2_cache_basic_operations() {
//     // Needs a running Redis instance
//     let redis_config = create_test_redis_config();
//     let redis_cache = RedisApiKeyCache::new(redis_config).await
//         .expect("❌ Redis connection failed - Redis must be running for this test!");

//     let api_key = create_test_api_key();
//     let key = "test_key_123";

//     // Set a key
//     let result = redis_cache.set(key, &api_key, Duration::from_secs(60)).await;
//     assert!(result.is_ok(), "Failed to set API key in Redis cache");

//     // Read it back
//     let retrieved = redis_cache.get(key).await;
//     assert!(retrieved.is_ok(), "Failed to get API key from Redis cache");
//     let retrieved_key = retrieved.unwrap();
//     assert!(retrieved_key.is_some(), "API key not found in Redis cache");
//     assert_eq!(retrieved_key.unwrap().api_key, api_key.api_key);

//     // Delete the key
//     let delete_result = redis_cache.delete(key).await;
//     assert!(delete_result.is_ok(), "Failed to delete API key from Redis cache");

//     // Make sure it's gone
//     let after_delete = redis_cache.get(key).await;
//     assert!(after_delete.is_ok(), "Failed to check after deletion");
//     assert!(after_delete.unwrap().is_none(), "API key should be deleted");
// }

// #[tokio::test]
// async fn test_l2_cache_ttl_expiration() {
//     let redis_config = create_test_redis_config();
//     let redis_cache = RedisApiKeyCache::new(redis_config).await
//         .expect("❌ Redis connection failed - Redis must be running for this test!");

//     let api_key = create_test_api_key();
//     let key = "ttl_test_key";

//     // Store with 1s TTL
//     let result = redis_cache.set(key, &api_key, Duration::from_secs(1)).await;
//     assert!(result.is_ok(), "Failed to set API key with TTL");

//     // Should exist right away
//     let retrieved = redis_cache.get(key).await;
//     assert!(retrieved.is_ok(), "Failed to get API key immediately after set");
//     assert!(retrieved.unwrap().is_some(), "API key should be available immediately");

//     // Wait past TTL
//     tokio::time::sleep(Duration::from_millis(1500)).await;

//     // Should be expired
//     let after_expiry = redis_cache.get(key).await;
//     assert!(after_expiry.is_ok(), "Failed to check after TTL expiry");
//     assert!(after_expiry.unwrap().is_none(), "API key should be expired");
// }

// #[tokio::test]
// async fn test_l2_cache_stats_tracking() {
//     let redis_config = create_test_redis_config();
//     let redis_cache = RedisApiKeyCache::new(redis_config).await
//         .expect("❌ Redis connection failed - Redis must be running for this test!");

//     let api_key = create_test_api_key();
//     let key = "stats_test_key";

//     // Stats should start empty
//     let initial_stats = redis_cache.stats();
//     assert_eq!(initial_stats.hits, 0);
//     assert_eq!(initial_stats.misses, 0);
//     assert_eq!(initial_stats.sets, 0);
//     assert_eq!(initial_stats.deletes, 0);

//     // Setting a key bumps `sets`
//     let _ = redis_cache.set(key, &api_key, Duration::from_secs(60)).await;
//     let after_set_stats = redis_cache.stats();
//     assert_eq!(after_set_stats.sets, 1);

//     // Getting it back bumps `hits`
//     let _ = redis_cache.get(key).await;
//     let after_get_stats = redis_cache.stats();
//     assert_eq!(after_get_stats.hits, 1);

//     // Getting something that doesn’t exist bumps `misses`
//     let _ = redis_cache.get("non_existent_key").await;
//     let after_miss_stats = redis_cache.stats();
//     assert_eq!(after_miss_stats.misses, 1);

//     // Deleting bumps `deletes`
//     let _ = redis_cache.delete(key).await;
//     let after_delete_stats = redis_cache.stats();
//     assert_eq!(after_delete_stats.deletes, 1);
// }

// #[tokio::test]
// async fn test_l2_cache_serialization_roundtrip() {
//     let redis_config = create_test_redis_config();
//     let redis_cache = RedisApiKeyCache::new(redis_config).await
//         .expect("❌ Redis connection failed - Redis must be running for this test!");

//     // Try different API key configs to ensure proper serialization
//     let test_cases = vec![
//         // HMAC key
//         ApiKeyConfig::new("hmac_key".to_string())
//             .with_hmac_secret("hmac_secret".to_string())
//             .with_permissions(vec![SecurityType::Trade])
//             .with_recv_window(5000),
        
//         // RSA key
//         ApiKeyConfig::new("rsa_key".to_string())
//             .with_rsa_public_key("-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA...\n-----END PUBLIC KEY-----".to_string())
//             .with_permissions(vec![SecurityType::UserData])
//             .with_recv_window(10000),
        
//         // Ed25519 key
//         ApiKeyConfig::new("ed25519_key".to_string())
//             .with_ed25519_public_key("-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA...\n-----END PUBLIC KEY-----".to_string())
//             .with_permissions(vec![SecurityType::UserStream])
//             .with_recv_window(60000),
        
//         // Key with IP blacklist
//         ApiKeyConfig::new("blacklist_key".to_string())
//             .with_hmac_secret("blacklist_secret".to_string())
//             .with_permissions(vec![SecurityType::Trade, SecurityType::UserData])
//             .with_ip_blacklist(vec!["192.168.1.1".to_string(), "10.0.0.1".to_string()])
//             .with_recv_window(5000),
//     ];

//     for (i, api_key) in test_cases.into_iter().enumerate() {
//         let key = format!("serialization_test_{}", i);
        
//         // Store the key
//         let set_result = redis_cache.set(&key, &api_key, Duration::from_secs(60)).await;
//         assert!(set_result.is_ok(), "Failed to set API key {}: {:?}", i, set_result);

//         // Fetch it back
//         let get_result = redis_cache.get(&key).await;
//         assert!(get_result.is_ok(), "Failed to get API key {}: {:?}", i, get_result);
        
//         let retrieved = get_result.unwrap();
//         assert!(retrieved.is_some(), "API key {} not found after roundtrip", i);
        
//         let retrieved_key = retrieved.unwrap();
        
//         // Fields should match
//         assert_eq!(retrieved_key.api_key, api_key.api_key);
//         assert_eq!(retrieved_key.permissions, api_key.permissions);
//         assert_eq!(retrieved_key.recv_window, api_key.recv_window);
//         assert_eq!(retrieved_key.enabled, api_key.enabled);
//         assert_eq!(retrieved_key.description, api_key.description);
//         assert_eq!(retrieved_key.ip_blacklist, api_key.ip_blacklist);
        
//         // Key material should also match
//         match (&retrieved_key.key_material, &api_key.key_material) {
//             (Some(ret), Some(orig)) => {
//                 match (ret, orig) {
//                     (KeyMaterial::HmacSecret(ret_val), KeyMaterial::HmacSecret(orig_val)) => {
//                         assert_eq!(ret_val, orig_val);
//                     }
//                     (KeyMaterial::RsaPublicKey(ret_val), KeyMaterial::RsaPublicKey(orig_val)) => {
//                         assert_eq!(ret_val, orig_val);
//                     }
//                     (KeyMaterial::Ed25519PublicKey(ret_val), KeyMaterial::Ed25519PublicKey(orig_val)) => {
//                         assert_eq!(ret_val, orig_val);
//                     }
//                     _ => panic!("Key material types don't match"),
//                 }
//             }
//             (None, None) => {} // both None
//             _ => panic!("Key material presence doesn't match"),
//         }

//         // Clean up
//         let _ = redis_cache.delete(&key).await;
//     }
// }

// #[tokio::test]
// async fn test_l2_cache_error_handling() {
//     // Pointing to an invalid Redis host
//     let invalid_config = RedisCacheConfig {
//         url: "redis://invalid-host:9999".to_string(),
//         key_prefix: "test:".to_string(),
//         max_connections: 1,
//         min_connections: 1,
//         connection_timeout_ms: 100, // tiny timeout
//         max_lifetime_secs: 60,
//         idle_timeout_secs: 30,
//         retry_count: 1,
//     };

//     let redis_cache = match RedisApiKeyCache::new(invalid_config).await {
//         Ok(cache) => cache,
//         Err(_) => {
//             println!("Skipping L2 cache error handling test - Redis connection failed as expected");
//             return;
//         }
//     };

//     let api_key = create_test_api_key();
//     let key = "error_test_key";

//     // These should fail
//     let set_result = redis_cache.set(key, &api_key, Duration::from_secs(60)).await;
//     assert!(set_result.is_err(), "Set should fail with invalid config");

//     let get_result = redis_cache.get(key).await;
//     assert!(get_result.is_err(), "Get should fail with invalid config");

//     let delete_result = redis_cache.delete(key).await;
//     assert!(delete_result.is_err(), "Delete should fail with invalid config");

//     // Stats should still be available
//     let stats = redis_cache.stats();
//     assert_eq!(stats.errors, 0); // errors tracked outside of cache
// }

// #[tokio::test]
// async fn test_l2_cache_ping_health_check() {
//     let redis_config = create_test_redis_config();
//     let redis_cache = RedisApiKeyCache::new(redis_config).await
//         .expect("❌ Redis connection failed - Redis must be running for this test!");

//     // Ping should work with a live Redis
//     let ping_result = redis_cache.ping().await;
//     assert!(ping_result.is_ok(), "Ping should succeed with valid Redis connection");
// }

// #[tokio::test]
// async fn test_l2_cache_key_prefixing() {
//     let redis_config = RedisCacheConfig {
//         url: "redis://localhost:6379".to_string(),
//         key_prefix: "my_app:api_keys:".to_string(),
//         max_connections: 5,
//         min_connections: 1,
//         connection_timeout_ms: 1000,
//         max_lifetime_secs: 300,
//         idle_timeout_secs: 60,
//         retry_count: 3,
//     };

//     let redis_cache = RedisApiKeyCache::new(redis_config).await
//         .expect("❌ Redis connection failed - Redis must be running for this test!");

//     let api_key = create_test_api_key();
//     let key = "prefix_test_key";

//     // Store with prefix
//     let set_result = redis_cache.set(key, &api_key, Duration::from_secs(60)).await;
//     assert!(set_result.is_ok(), "Failed to set API key with prefix");

//     // Fetch with prefix
//     let get_result = redis_cache.get(key).await;
//     assert!(get_result.is_ok(), "Failed to get API key with prefix");
//     assert!(get_result.unwrap().is_some(), "API key not found with prefix");

//     // Clean up
//     let _ = redis_cache.delete(key).await;
// }

// // Test L1 + L2 + (fake) L3 hierarchy
// #[tokio::test]
// async fn test_cache_hierarchy_l1_l2_l3() {
//     // Full hierarchy test (L1 + L2 + fake DB as L3).
//     // Requires Redis, skips DB for now.
//     let cache_config = create_l2_cache_config();
//     let redis_config = create_test_redis_config();
    
//     let store = ApiKeyStore::with_redis_cache(cache_config, redis_config, None).await
//         .expect("❌ Redis connection failed - Redis must be running for this test!");

//     let api_key = create_test_api_key();
//     let key = "test_key_123";

//     // Pretend DB already has the key
//     let store_with_key = store.with_api_keys(vec![api_key.clone()]);

//     // First lookup: miss L1/L2, hit L3
//     let result1 = store_with_key.get_api_key(key).await;
//     assert!(result1.is_some(), "Should find key in memory store");

//     // Check metrics (loose assertions since stats may bleed across tests)
//     let stats = store_with_key.metrics().get_cache_stats();
//     assert!(stats.misses >= 1);
//     assert!(stats.hits >= 0);

//     // Second lookup: should hit L1
//     let result2 = store_with_key.get_api_key(key).await;
//     assert!(result2.is_some(), "Should hit L1 cache");

//     let stats2 = store_with_key.metrics().get_cache_stats();
//     assert!(stats2.hits >= 1);
//     assert!(stats2.misses >= 1);
// }

// #[tokio::test]
// async fn test_cache_invalidation_l1_l2() {
//     let cache_config = create_l2_cache_config();
//     let redis_config = create_test_redis_config();
    
//     let store = ApiKeyStore::with_redis_cache(cache_config, redis_config, None).await
//         .expect("❌ Redis connection failed - Redis must be running for this test!");

//     let api_key = create_test_api_key();
//     let key = "test_key_123";
//     let store_with_key = store.with_api_keys(vec![api_key.clone()]);

//     // First fetch populates caches
//     let result1 = store_with_key.get_api_key(key).await;
//     assert!(result1.is_some());

//     // Second fetch should hit L1
//     let result2 = store_with_key.get_api_key(key).await;
//     assert!(result2.is_some());

//     // Invalidate the key
//     store_with_key.invalidate(key).await;

//     // Next fetch should miss L1 and fall back to L2
//     let result3 = store_with_key.get_api_key(key).await;
//     assert!(result3.is_some());

//     // Metrics should show new misses
//     let stats = store_with_key.metrics().get_cache_stats();
//     assert!(stats.misses > 1);
// }

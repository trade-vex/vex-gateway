use actix_web::{web, App, HttpResponse};
use lib_middleware::api_key::{
    middleware::{ApiKeyMiddleware, ApiKeyStore, CacheConfig, RouteSecurityConfig},
    security::{ApiKeyConfig, SecurityType},
};
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

#[actix_web::test]
async fn test_route_security_does_not_match_partial_segment() {
    let middleware = ApiKeyMiddleware::new().with_route_configs(vec![RouteSecurityConfig {
        path_pattern: "/api/order".to_string(),
        security_type: SecurityType::Trade,
    }]);
    let app = actix_web::test::init_service(
        App::new()
            .wrap(middleware)
            .route("/api/orderXYZ", web::get().to(HttpResponse::Ok)),
    )
    .await;

    let request = actix_web::test::TestRequest::get()
        .uri("/api/orderXYZ")
        .to_request();
    let response = actix_web::test::call_service(&app, request).await;

    assert_eq!(response.status(), actix_web::http::StatusCode::OK);
}

#[actix_web::test]
async fn test_root_route_pattern_matches_every_path() {
    let middleware = ApiKeyMiddleware::new().with_route_configs(vec![RouteSecurityConfig {
        path_pattern: "/".to_string(),
        security_type: SecurityType::Trade,
    }]);
    let app = actix_web::test::init_service(
        App::new()
            .wrap(middleware)
            .route("/api/anything", web::get().to(HttpResponse::Ok)),
    )
    .await;

    let request = actix_web::test::TestRequest::get()
        .uri("/api/anything")
        .to_request();
    let error = actix_web::test::try_call_service(&app, request)
        .await
        .expect_err("a \"/\" rule must secure every path, so a request without an API key fails");

    assert_eq!(
        error.as_response_error().status_code(),
        actix_web::http::StatusCode::UNAUTHORIZED
    );
}

#[test]
fn test_cache_config_default() {
    let config = CacheConfig::default();

    assert_eq!(config.l1_max_size, 100_000);
    assert_eq!(config.l1_ttl, Duration::from_secs(5));
}

#[tokio::test]
async fn test_invalidated_cached_key_stops_authenticating() {
    let store = ApiKeyStore::new(CacheConfig::default());
    let api_key =
        ApiKeyConfig::new("revoked-key".to_string()).with_hmac_secret("revoked-secret".to_string());

    store.add_api_key(api_key).await;
    assert!(store.get_api_key("revoked-key").await.unwrap().is_some());

    store.invalidate("revoked-key").await;

    assert!(store.get_api_key("revoked-key").await.unwrap().is_none());
}

#[tokio::test]
async fn test_cache_invalidation_does_not_affect_unrelated_key() {
    let store = ApiKeyStore::new(CacheConfig::default());
    let revoked_key =
        ApiKeyConfig::new("revoked-key".to_string()).with_hmac_secret("revoked-secret".to_string());
    let unrelated_key = ApiKeyConfig::new("unrelated-key".to_string())
        .with_hmac_secret("unrelated-secret".to_string());

    store.add_api_key(revoked_key).await;
    store.add_api_key(unrelated_key).await;

    store.invalidate("revoked-key").await;

    assert!(store.get_api_key("unrelated-key").await.unwrap().is_some());
}

/// An L1 entry older than `l1_ttl` is looked up again: it is how a revocation on another replica
/// reaches this one. The store has no repository and an empty memory store, so a stale entry is
/// the only way the key could still be found.
#[tokio::test]
async fn cached_key_expires_after_ttl() {
    let store = ApiKeyStore::new(CacheConfig {
        l1_max_size: 1,
        l1_ttl: Duration::from_millis(20),
    });
    store
        .add_api_key(ApiKeyConfig::new("ttl-key".to_string()))
        .await;
    assert!(store.get_api_key("ttl-key").await.unwrap().is_some());

    tokio::time::sleep(Duration::from_millis(40)).await;

    assert!(
        store.get_api_key("ttl-key").await.unwrap().is_none(),
        "an expired L1 entry must not authenticate"
    );
}

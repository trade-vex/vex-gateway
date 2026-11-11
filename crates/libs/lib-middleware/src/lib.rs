pub mod api_key;
pub mod client_ip;
pub mod metrics;
pub mod models;
pub mod rate_limit;
pub mod repository;

pub use api_key::{
    error::ApiKeyError,
    middleware::{ApiKeyMiddleware, ApiKeyStore, CacheConfig, CacheMetrics, RouteSecurityConfig},
    security::{ApiKeyConfig, KeyMaterial, SecurityType},
    signature::{SignatureAlgorithm, SignatureVerifier},
};

pub use client_ip::TrustedProxies;

pub use models::api_key::{KeyMaterialType, Model as ApiKeyModel, NewApiKey};

pub use repository::api_key_repository::{ApiKeyRepository, PostgresApiKeyRepository};

/// A connection to the tests' Redis: `REDIS_URL`, or the default local port when it is unset. CI
/// sets `REDIS_URL` to its Redis service.
#[cfg(test)]
async fn test_redis() -> redis::aio::ConnectionManager {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(url).expect("REDIS_URL must be a Redis URL");
    redis::aio::ConnectionManager::new(client)
        .await
        .expect("these tests need the Redis at REDIS_URL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_api_exports() {
        // Ensure all public exports are accessible
        let _security_type = SecurityType::Trade;
        let _api_key_config = ApiKeyConfig::new("test".to_string());
        let _error = ApiKeyError::NotFound;
        let _algorithm = SignatureAlgorithm::HmacSha256;

        // Verify enums can be compared
        assert_eq!(_security_type, SecurityType::Trade);
        assert_ne!(_security_type, SecurityType::None);

        // Verify error display
        assert_eq!(_error.to_string(), "API key not found");

        // Verify algorithm equality
        assert_eq!(_algorithm, SignatureAlgorithm::HmacSha256);
    }

    #[test]
    fn test_api_key_config_basic_usage() {
        let config = ApiKeyConfig::new("test-key".to_string())
            .with_hmac_secret("secret".to_string())
            .with_permissions(vec![SecurityType::Trade, SecurityType::UserData]);

        assert_eq!(config.api_key, "test-key");
        assert!(config.has_permission(SecurityType::Trade));
        assert!(config.has_permission(SecurityType::UserData));
        assert!(!config.has_permission(SecurityType::UserStream));
        assert!(config.is_valid());
    }

    #[test]
    fn test_signature_verifier_basic_functionality() {
        let payload = "test=value&timestamp=123456";
        let secret = "test-secret";

        let signature = SignatureVerifier::generate_hmac(payload, secret).unwrap();
        let is_valid = SignatureVerifier::verify_hmac(payload, &signature, secret).unwrap();

        assert!(is_valid);
    }
}

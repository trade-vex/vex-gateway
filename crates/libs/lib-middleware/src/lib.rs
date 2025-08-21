pub mod api_key;
pub mod models;
pub mod repository;

pub use api_key::{
    middleware::{ApiKeyMiddleware, RouteSecurityConfig, ApiKeyStore, CacheConfig, CacheMetrics},
    security::{SecurityType, ApiKeyConfig, KeyMaterial},
    error::ApiKeyError,
    signature::{SignatureVerifier, SignatureAlgorithm},
};

pub use models::{
    api_key::{Model as ApiKeyModel, NewApiKey, KeyMaterialType},
};

pub use repository::{
    api_key_repository::{ApiKeyRepository, PostgresApiKeyRepository},
};

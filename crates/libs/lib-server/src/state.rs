//! lib-server/src/state.rs
//!
//! Central application state containing all dependencies

use deadpool_redis::Pool as RedisPool;
use jsonwebtoken::{DecodingKey, EncodingKey};
use lib_auth::config::{AuthConfig, JwtConfig, OAuthProviderConfig};
use lib_auth::repository::{PostgresUserRepository, UserRepository};
use lib_auth::service::{EmailService, OAuthService, TokenService};
use lib_core::CorrelationService;
use lib_core::Publisher;
use lib_middleware::{
    ApiKeyMiddleware, SecurityType,
    repository::api_key_repository::{ApiKeyRepository, PostgresApiKeyRepository},
};
use sea_orm::DatabaseConnection;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::Mutex as TokioMutex;
use url::Url;

/// Central application state containing all dependencies
///
/// This struct follows the Dependency Injection pattern by centralizing
/// all app dependencies in one place for clean handler interfaces.
#[derive(Clone)]
pub struct AppState {
    // Database and infrastructure
    pub db: Arc<DatabaseConnection>,
    pub redis_pool: Arc<RedisPool>,
    pub http_client: reqwest::Client,

    // Auth-specific services
    pub user_repo: Arc<dyn UserRepository>,

    // Auth configuration
    pub jwt_config: JwtConfig,
    pub encoding_key: EncodingKey,
    pub decoding_key: DecodingKey,
    pub oauth_providers: HashMap<String, OAuthProviderConfig>,
    pub auth_config: AuthConfig,
    pub token_service: TokenService,
    pub oauth_service: OAuthService,
    pub email_service: EmailService,

    // Gateway publisher for order processing
    pub gateway_publisher: Arc<Mutex<Publisher>>,

    // API key middleware for dynamic updates
    pub api_key_middleware: Arc<TokioMutex<ApiKeyMiddleware>>,

    // API key repository for database operations
    pub api_key_repository: Arc<dyn ApiKeyRepository>,

    // Correlation service for matching responses to requests
    pub correlation_service: CorrelationService,
}

impl AppState {
    /// Create a new AppState with all dependencies initialized
    pub async fn new(
        config: &AuthConfig,
        gateway_publisher: Publisher,
        correlation_service: CorrelationService,
    ) -> Result<Arc<Self>, Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("Initializing AppState with configuration");

        // Initialize database connection
        let db = Self::init_database(&config.database).await?;

        // Initialize Redis connection
        let redis_pool = Self::init_redis(&config.redis).await?;

        // Initialize HTTP client
        let http_client = Self::init_http_client()?;

        // Initialize JWT keys
        let (encoding_key, decoding_key) = Self::init_jwt_keys(&config.jwt)?;

        // Initialize auth-specific services
        let user_repo = Arc::new(PostgresUserRepository::new(Arc::new(db.clone())));

        // Initialize API key repository
        let api_key_repository: Arc<dyn ApiKeyRepository> =
            Arc::new(PostgresApiKeyRepository::new(Arc::new(db.clone())));

        // Initialize API key middleware with database repository
        let api_key_middleware = Self::init_api_key_middleware(&api_key_repository);

        // Initialize services
        let token_service = TokenService::new(
            Arc::new(config.jwt.clone()),
            Arc::new(redis_pool.clone()),
            Arc::new(encoding_key.clone()),
            Arc::new(decoding_key.clone()),
        );

        let oauth_service = OAuthService::new(
            Arc::new(redis_pool.clone()),
            http_client.clone(),
            config.oauth.providers.clone(),
            Arc::new(token_service.clone()),
            user_repo.clone(),
            config.oauth.redirect_base.clone(),
            config.server.frontend_url.clone(),
        )
        .await?;

        let email_service = EmailService::new(
            &config.email,
            Arc::new(redis_pool.clone()),
            config.otp.otp_length,
            config.otp.email_verification_expiry_minutes,
        )?;

        tracing::info!("AppState initialization completed successfully");

        Ok(Arc::new(Self {
            db: Arc::new(db),
            redis_pool: Arc::new(redis_pool),
            http_client,
            user_repo,
            jwt_config: config.jwt.clone(),
            encoding_key,
            decoding_key,
            oauth_providers: config.oauth.providers.clone(),
            auth_config: config.clone(),
            token_service,
            oauth_service,
            email_service,
            gateway_publisher: Arc::new(Mutex::new(gateway_publisher)),
            api_key_middleware: Arc::new(TokioMutex::new(api_key_middleware)),
            api_key_repository,
            correlation_service,
        }))
    }

    /// Sanitize a URL string to remove credentials and sensitive information
    fn sanitize_url(url_str: &str) -> String {
        match Url::parse(url_str) {
            Ok(mut url) => {
                // Remove username and password
                let _ = url.set_username("");
                let _ = url.set_password(None);

                // For database URLs, we can show scheme, host, port, and path (database name)
                let scheme = url.scheme();
                let host = url.host_str().unwrap_or("<unknown>");
                let port = url.port();
                let path = url.path();

                if let Some(port) = port {
                    format!("{}://{}:{}{}", scheme, host, port, path)
                } else {
                    // Use default ports if not specified
                    let default_port = match scheme {
                        "postgresql" | "postgres" => ":5432",
                        "redis" | "rediss" => ":6379",
                        _ => "",
                    };
                    format!("{}://{}{}{}", scheme, host, default_port, path)
                }
            }
            Err(_) => "<invalid-url>".to_string(),
        }
    }

    /// Initialize JWT keys from configuration
    fn init_jwt_keys(
        config: &lib_auth::config::JwtConfig,
    ) -> Result<(EncodingKey, DecodingKey), Box<dyn std::error::Error + Send + Sync>> {
        use std::fs;
        use std::path::Path;

        let current_dir = std::env::current_dir().unwrap_or_else(|_| "unknown".into());
        tracing::info!("Current working directory: {:?}", current_dir);
        tracing::info!("Looking for private key at: {}", config.private_key_path);
        tracing::info!("Looking for public key at: {}", config.public_key_path);

        if !Path::new(&config.private_key_path).exists() {
            return Err(format!(
                "Private key file not found: {}. Current directory: {:?}. Please run: openssl genpkey -algorithm RSA -out {} -pkcs8",
                config.private_key_path, current_dir, config.private_key_path
            ).into());
        }

        if !Path::new(&config.public_key_path).exists() {
            return Err(format!(
                "Public key file not found: {}. Current directory: {:?}. Please run: openssl rsa -pubout -in {} -out {}",
                config.public_key_path, current_dir, config.private_key_path, config.public_key_path
            ).into());
        }

        let private_key = fs::read(&config.private_key_path).map_err(|e| {
            format!(
                "Failed to read private key from {}: {}",
                config.private_key_path, e
            )
        })?;

        let public_key = fs::read(&config.public_key_path).map_err(|e| {
            format!(
                "Failed to read public key from {}: {}",
                config.public_key_path, e
            )
        })?;

        let encoding_key = EncodingKey::from_rsa_pem(&private_key).map_err(|e| {
            format!(
                "Invalid private key format in {}: {}",
                config.private_key_path, e
            )
        })?;

        let decoding_key = DecodingKey::from_rsa_pem(&public_key).map_err(|e| {
            format!(
                "Invalid public key format in {}: {}",
                config.public_key_path, e
            )
        })?;

        tracing::info!("Successfully loaded JWT keys");
        Ok((encoding_key, decoding_key))
    }

    /// Initialize database connection
    async fn init_database(
        config: &lib_auth::config::DatabaseConfig,
    ) -> Result<DatabaseConnection, Box<dyn std::error::Error + Send + Sync>> {
        use sea_orm::Database;
        let sanitized_url = Self::sanitize_url(&config.url);
        tracing::info!("Connecting to database: {}", sanitized_url);
        Database::connect(&config.url)
            .await
            .map_err(|e| format!("Failed to connect to database: {}", e).into())
    }

    /// Initialize Redis connection pool
    async fn init_redis(
        config: &lib_auth::config::RedisConfig,
    ) -> Result<RedisPool, Box<dyn std::error::Error + Send + Sync>> {
        use deadpool_redis::{Config as RedisConfig, Runtime};
        let sanitized_url = Self::sanitize_url(&config.url);
        tracing::info!("Connecting to Redis: {}", sanitized_url);
        let redis_config = RedisConfig::from_url(&config.url);
        redis_config
            .create_pool(Some(Runtime::Tokio1))
            .map_err(|e| format!("Failed to create Redis pool: {}", e).into())
    }

    /// Initialize HTTP client for external API calls
    fn init_http_client() -> Result<reqwest::Client, Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("Initializing HTTP client");
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e).into())
    }

    /// Create ApiKeyMiddleware using AppState dependencies
    pub fn init_api_key_middleware(api_key_repo: &Arc<dyn ApiKeyRepository>) -> ApiKeyMiddleware {
        use lib_middleware::api_key::middleware::CacheConfig;

        // Create cache configuration
        let cache_config = CacheConfig {
            l1_max_size: 1000,                           // Cache up to 1000 API keys in memory
            l1_ttl: std::time::Duration::from_secs(300), // 5 minute TTL
        };

        // Create middleware with database repository for dynamic loading
        ApiKeyMiddleware::new()
            .with_cache_config(cache_config)
            .with_repository(api_key_repo.clone())
            .with_default_security(SecurityType::Trade)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_database_url() {
        // Test PostgreSQL URL with credentials
        let url = "postgresql://user:password@localhost:5432/mydb?sslmode=require";
        let sanitized = AppState::sanitize_url(url);
        assert_eq!(sanitized, "postgresql://localhost:5432/mydb");

        // Test PostgreSQL URL without port
        let url = "postgresql://user:pass@localhost/mydb";
        let sanitized = AppState::sanitize_url(url);
        assert_eq!(sanitized, "postgresql://localhost:5432/mydb");
    }

    #[test]
    fn test_sanitize_redis_url() {
        // Test Redis URL with credentials
        let url = "redis://user:password@localhost:6379/0";
        let sanitized = AppState::sanitize_url(url);
        assert_eq!(sanitized, "redis://localhost:6379/0");

        // Test Redis URL without credentials
        let url = "redis://localhost:6379/1";
        let sanitized = AppState::sanitize_url(url);
        assert_eq!(sanitized, "redis://localhost:6379/1");
    }

    #[test]
    fn test_sanitize_invalid_url() {
        let url = "not a valid url";
        let sanitized = AppState::sanitize_url(url);
        assert_eq!(sanitized, "<invalid-url>");
    }
}

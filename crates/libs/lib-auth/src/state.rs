use crate::config::{AuthConfig, OAuthProviderConfig};
use crate::error::auth_error::AuthError;
use crate::repository::{PostgresUserRepository, UserRepository};
use deadpool_redis::{Config as RedisConfig, Pool as RedisPool, Runtime};
use jsonwebtoken::{DecodingKey, EncodingKey};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub user_repo: Arc<dyn UserRepository>,
    pub db_pool: Arc<PgPool>,
    pub redis_pool: Arc<RedisPool>,
    pub jwt_config: crate::config::JwtConfig,
    pub encoding_key: EncodingKey,
    pub decoding_key: DecodingKey,
    pub oauth_providers: HashMap<String, OAuthProviderConfig>,
    pub http_client: reqwest::Client,
    pub server_config: crate::config::ServerConfig,
}

impl AppState {
    pub async fn new(config: AuthConfig) -> Result<Arc<Self>, AuthError> {
        // Database connection
        let db_pool = PgPoolOptions::new()
            .max_connections(config.database.max_connections)
            .connect(&config.database.url)
            .await
            .map_err(|e| AuthError::Database(format!("Failed to connect to database: {}", e)))?;

        // Redis connection
        let redis_config = RedisConfig::from_url(&config.redis.url);
        let redis_pool = redis_config
            .create_pool(Some(Runtime::Tokio1))
            .map_err(|e| AuthError::Redis(format!("Failed to create Redis pool: {}", e)))?;

        // Create the repository and store it in the state
        let user_repo: Arc<dyn UserRepository> =
            Arc::new(PostgresUserRepository::new(Arc::new(db_pool.clone())));

        // JWT keys
        let (encoding_key, decoding_key) = load_jwt_keys(&config.jwt)?;

        // HTTP client for OAuth requests
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| {
                AuthError::InternalError(format!("Failed to create HTTP client: {}", e))
            })?;

        Ok(Arc::new(Self {
            user_repo,
            db_pool: Arc::new(db_pool),
            redis_pool: Arc::new(redis_pool),
            jwt_config: config.jwt,
            encoding_key,
            decoding_key,
            oauth_providers: config.oauth.providers,
            http_client,
            server_config: config.server,
        }))
    }
}

fn load_jwt_keys(
    config: &crate::config::JwtConfig,
) -> Result<(EncodingKey, DecodingKey), AuthError> {
    use std::fs;
    use std::path::Path;

    // Show the current working directory for debugging
    let current_dir = std::env::current_dir().unwrap_or_else(|_| "unknown".into());
    tracing::info!("Current working directory: {:?}", current_dir);
    tracing::info!("Looking for private key at: {}", config.private_key_path);
    tracing::info!("Looking for public key at: {}", config.public_key_path);

    // Check if files exist before trying to read them
    if !Path::new(&config.private_key_path).exists() {
        return Err(AuthError::Config(format!(
            "Private key file not found: {}. Current directory: {:?}. Please run: openssl genpkey -algorithm RSA -out {} -pkcs8",
            config.private_key_path, current_dir, config.private_key_path
        )));
    }

    if !Path::new(&config.public_key_path).exists() {
        return Err(AuthError::Config(format!(
            "Public key file not found: {}. Current directory: {:?}. Please run: openssl rsa -pubout -in {} -out {}",
            config.public_key_path, current_dir, config.private_key_path, config.public_key_path
        )));
    }

    let private_key = fs::read(&config.private_key_path).map_err(|e| {
        AuthError::Config(format!(
            "Failed to read private key from {}: {}",
            config.private_key_path, e
        ))
    })?;

    let public_key = fs::read(&config.public_key_path).map_err(|e| {
        AuthError::Config(format!(
            "Failed to read public key from {}: {}",
            config.public_key_path, e
        ))
    })?;

    let encoding_key = EncodingKey::from_rsa_pem(&private_key).map_err(|e| {
        AuthError::Config(format!(
            "Invalid private key format in {}: {}",
            config.private_key_path, e
        ))
    })?;

    let decoding_key = DecodingKey::from_rsa_pem(&public_key).map_err(|e| {
        AuthError::Config(format!(
            "Invalid public key format in {}: {}",
            config.public_key_path, e
        ))
    })?;

    tracing::info!("Successfully loaded JWT keys");
    Ok((encoding_key, decoding_key))
}

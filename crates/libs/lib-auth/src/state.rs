use crate::config::AuthConfig;
use crate::error::AuthError;
use crate::keys::{load_private_key, load_public_key};
use crate::config::JwtConfig; // Added for clarity
use deadpool_redis::{Config as RedisConfig, Pool as RedisPool, Runtime};
use jsonwebtoken::{DecodingKey, EncodingKey};
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl};
use std::collections::HashMap;
use std::sync::Arc;

/// The shared state accessible by all handlers and middleware.
#[derive(Clone)]
pub struct AppState {
    pub oidc_clients: Arc<HashMap<String, CoreClient>>,
    pub redis_pool: Arc<RedisPool>,
    pub encoding_key: Arc<EncodingKey>,
    pub decoding_key: Arc<DecodingKey>,
    pub jwt_config: Arc<JwtConfig>,
}

impl AppState {
    /// Creates a new AppState instance by loading all configurations and initializing clients.
    pub async fn new(config: AuthConfig) -> Result<Self, AuthError> {
        // 1. Initialize OIDC clients
        let mut oidc_clients = HashMap::new();
        for (name, provider_config) in &config.providers {
            let provider_metadata = CoreProviderMetadata::discover_async(
                IssuerUrl::new(provider_config.issuer_url.clone())
                    .map_err(|e| AuthError::OidcDiscovery(e.to_string()))?,
                openidconnect::reqwest::async_http_client, // <-- CORRECTED LINE
            )
            .await
            .map_err(|e| AuthError::OidcDiscovery(e.to_string()))?;

            let redirect_url = format!(
                "{}/auth/sso/{}/callback",
                config.server.base_url, name
            );

            let client = CoreClient::from_provider_metadata(
                provider_metadata,
                ClientId::new(provider_config.client_id.clone()),
                Some(ClientSecret::new(provider_config.client_secret.clone())),
            )
            .set_redirect_uri(
                RedirectUrl::new(redirect_url)
                    .map_err(|e| AuthError::Config(format!("Invalid redirect URL: {}", e)))?,
            );
            oidc_clients.insert(name.clone(), client);
        }

        // 2. Initialize Redis pool
        let redis_cfg = RedisConfig::from_url(&config.redis.url);
        let redis_pool = redis_cfg
            .create_pool(Some(Runtime::Tokio1))
            .map_err(|e| AuthError::Config(format!("Failed to create Redis pool: {}", e)))?;

        // 3. Load cryptographic keys
        let encoding_key = load_private_key(&config.jwt.private_key_path)?;
        let decoding_key = load_public_key(&config.jwt.public_key_path)?;

        Ok(AppState {
            oidc_clients: Arc::new(oidc_clients),
            redis_pool: Arc::new(redis_pool),
            encoding_key: Arc::new(encoding_key),
            decoding_key: Arc::new(decoding_key),
            jwt_config: Arc::new(config.jwt),
        })
    }
}
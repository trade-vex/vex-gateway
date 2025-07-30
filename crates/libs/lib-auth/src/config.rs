use serde::Deserialize;
use std::collections::HashMap;
use config::{Config, Environment};

#[derive(Deserialize, Clone, Debug)]
pub struct AuthConfig {
    pub providers: HashMap<String, OidcProviderConfig>,
    pub jwt: JwtConfig,
    pub redis: RedisConfig,
    pub server: ServerConfig,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ServerConfig {
    pub base_url: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct OidcProviderConfig {
    pub client_id: String,
    pub client_secret: String,
    pub issuer_url: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct JwtConfig {
    pub private_key_path: String,
    pub public_key_path: String,
    pub access_token_ttl_minutes: i64,
    pub refresh_token_ttl_days: i64,
    pub issuer: String,
    pub audience: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RedisConfig {
    pub url: String,
}


impl AuthConfig {
    /// Initializes configuration by loading from a .env file and then environment variables.
    pub fn init() -> Result<Self, config::ConfigError> {
        // 1. Load the .env file into the environment.
        // When running tests, cargo sets CARGO_MANIFEST_DIR, which we use to find the .env file.
        if let Ok(path) = std::env::var("CARGO_MANIFEST_DIR") {
            dotenvy::from_path(format!("{}/src/.env", path)).ok();
        } else {
            dotenvy::dotenv().ok();
        }

        // 2. Build the configuration source purely from environment variables.
        let settings = Config::builder()
            .add_source(Environment::with_prefix("APP").separator("__"))
            .build()?;

        // 3. Deserialize into our struct.
        settings.try_deserialize()
    }
}
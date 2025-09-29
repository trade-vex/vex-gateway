use serde::Deserialize;
use secrecy::SecretString;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
#[derive(Deserialize, Clone, Debug)]
pub struct JwtConfig {
    pub private_key_path: String,
    pub public_key_path: String,
    pub access_token_ttl_minutes: i64,
    pub refresh_token_ttl_days: i64,
    pub issuer: String,
    pub audience: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AuthConfig {
    pub jwt: JwtConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub oauth: OAuthConfig,
    pub server: ServerConfig,
    pub email: EmailConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OAuthConfig {
    pub providers: HashMap<String, OAuthProviderConfig>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub client_secret: SecretString,
    pub auth_url: String,
    pub token_url: String,
    pub user_info_url: String,
    pub scopes: Vec<String>,
    pub response_type: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RedisConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ServerConfig {
    pub base_url: String,
    pub port: u16,
}

#[derive(Deserialize, Debug, Clone)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: SecretString,
    pub from_email: String,
}

impl AuthConfig {
    pub fn init() -> Result<Self, ConfigError> {
        Ok(Self {
            jwt: JwtConfig::from_env()?,
            database: DatabaseConfig::from_env()?,
            redis: RedisConfig::from_env()?,
            oauth: OAuthConfig::from_env()?,
            server: ServerConfig::from_env()?,
            email: EmailConfig::from_env()?,
        })
    }

    /// Create development defaults without requiring environment variables
    pub fn development_defaults() -> Self {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            database: DatabaseConfig {
                url: "postgres://myuser:mypassword@localhost/mydb".to_string(),
                max_connections: 10,
            },
            redis: RedisConfig {
                url: "redis://127.0.0.1:6379".to_string(),
                max_connections: 10,
            },
            jwt: JwtConfig {
                private_key_path: manifest_dir
                    .join("src/keys/private_key.pem")
                    .to_string_lossy()
                    .to_string(),
                public_key_path: manifest_dir
                    .join("src/keys/public_key.pem")
                    .to_string_lossy()
                    .to_string(),
                access_token_ttl_minutes: 60,
                refresh_token_ttl_days: 30,
                issuer: "vex-gateway-dev".to_string(),
                audience: "vex-api-dev".to_string(),
            },
            oauth: OAuthConfig {
                providers: std::collections::HashMap::new(), // Empty for development
            },
            email: EmailConfig {
                smtp_host: "smtp.example.com".to_string(),
                smtp_port: 587,
                smtp_username: "user@example.com".to_string(),
                smtp_password: SecretString::new("password".to_string()),
                from_email: "noreply@example.com".to_string(),
            },
            server: ServerConfig {
                base_url: "http://localhost:8081".to_string(),
                port: 8081,
            },
        }
    }
}

impl JwtConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            private_key_path: env::var("JWT_PRIVATE_KEY_PATH")?,
            public_key_path: env::var("JWT_PUBLIC_KEY_PATH")?,
            access_token_ttl_minutes: env::var("JWT_ACCESS_TOKEN_TTL_MINUTES")?.parse()?,
            refresh_token_ttl_days: env::var("JWT_REFRESH_TOKEN_TTL_DAYS")?.parse()?,
            issuer: env::var("JWT_ISSUER")?,
            audience: env::var("JWT_AUDIENCE")?,
        })
    }
}

impl OAuthConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut providers = HashMap::new();

        // Google OAuth 2.0
        if let (Ok(client_id), Ok(client_secret)) = (
            env::var("GOOGLE_CLIENT_ID"),
            env::var("GOOGLE_CLIENT_SECRET"),
        ) {
            providers.insert(
                "google".to_string(),
                OAuthProviderConfig {
                    client_id,
                    client_secret: SecretString::new(client_secret),
                    auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                    token_url: "https://oauth2.googleapis.com/token".to_string(),
                    user_info_url: "https://www.googleapis.com/oauth2/v2/userinfo".to_string(),
                    scopes: vec!["email".to_string(), "profile".to_string()],
                    response_type: "code".to_string(),
                },
            );
        }

        // GitHub OAuth 2.0
        if let (Ok(client_id), Ok(client_secret)) = (
            env::var("GITHUB_CLIENT_ID"),
            env::var("GITHUB_CLIENT_SECRET"),
        ) {
            providers.insert(
                "github".to_string(),
                OAuthProviderConfig {
                    client_id,
                    client_secret: SecretString::new(client_secret),
                    auth_url: "https://github.com/login/oauth/authorize".to_string(),
                    token_url: "https://github.com/login/oauth/access_token".to_string(),
                    user_info_url: "https://api.github.com/user".to_string(),
                    scopes: vec!["user:email".to_string()],
                    response_type: "code".to_string(),
                },
            );
        }

        // X (Twitter) OAuth 2.0
        if let (Ok(client_id), Ok(client_secret)) = (
            env::var("TWITTER_CLIENT_ID"),
            env::var("TWITTER_CLIENT_SECRET"),
        ) {
            providers.insert(
                "twitter".to_string(),
                OAuthProviderConfig {
                    client_id,
                    client_secret: SecretString::new(client_secret),
                    auth_url: "https://twitter.com/i/oauth2/authorize".to_string(),
                    token_url: "https://api.twitter.com/2/oauth2/token".to_string(),
                    user_info_url: "https://api.twitter.com/2/users/me".to_string(),
                    scopes: vec!["tweet.read".to_string(), "users.read".to_string()],
                    response_type: "code".to_string(),
                },
            );
        }

        Ok(Self { providers })
    }
}

impl DatabaseConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            url: env::var("DATABASE_URL")?,
            max_connections: env::var("DATABASE_MAX_CONNECTIONS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()?,
        })
    }
}

impl RedisConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            url: env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            max_connections: env::var("REDIS_MAX_CONNECTIONS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()?,
        })
    }
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            base_url: env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8081".to_string()),
            port: env::var("PORT")
                .unwrap_or_else(|_| "8081".to_string())
                .parse()?,
        })
    }
}

impl EmailConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            smtp_host: env::var("SMTP_HOST")?,
            smtp_port: env::var("SMTP_PORT")?.parse()?,
            smtp_username: env::var("SMTP_USERNAME")?,
            smtp_password: SecretString::new(env::var("SMTP_PASSWORD")?),
            from_email: env::var("FROM_EMAIL")?,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Environment variable error: {0}")]
    EnvVar(#[from] env::VarError),
    #[error("Parse error: {0}")]
    Parse(#[from] std::num::ParseIntError),
}

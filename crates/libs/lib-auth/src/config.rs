use lib_middleware::TrustedProxies;
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
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
    pub otp: OtpConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OAuthConfig {
    pub providers: HashMap<String, OAuthProviderConfig>,
    pub redirect_base: String,
}

/// The providers `auth.oauth.providers` may list: `OAuthService` fixes their scopes, user-info
/// endpoints and profile parsing (lib-config validation).
pub const OAUTH_PROVIDERS: [&str; 3] = ["google", "github", "twitter"];

#[derive(Deserialize, Debug, Clone)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub client_secret: SecretString,
    pub auth_url: String,
    pub token_url: String,
    pub user_info_url: String,
    #[serde(deserialize_with = "list")]
    pub scopes: Vec<String>,
    pub response_type: String,
    /// Optional OpenID Connect discovery URL (e.g., for Google)
    pub issuer_url: Option<String>,
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
    pub frontend_url: String,
    /// Proxy addresses or CIDR ranges whose `X-Forwarded-For` is believed when resolving the
    /// client IP for rate limits and API-key IP checks. Empty trusts none (the TCP peer is used);
    /// production requires at least one (lib-config validation).
    #[serde(default, deserialize_with = "list")]
    pub trusted_proxies: Vec<String>,
}

impl ServerConfig {
    /// `trusted_proxies`, parsed. The error names an invalid entry by its position, never by its
    /// value.
    pub fn parsed_trusted_proxies(&self) -> Result<TrustedProxies, String> {
        TrustedProxies::parse(&self.trusted_proxies)
    }
}

/// A list setting: a TOML array, or a comma-separated string, which is how an `APP__` environment
/// variable sets one (`APP__AUTH__SERVER__TRUSTED_PROXIES=10.0.1.0/24,10.0.2.0/24`). Items are
/// trimmed and empty ones dropped, so an empty string is an empty list.
fn list<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    use serde::de::{self, MapAccess, SeqAccess, Visitor};
    use std::fmt;

    struct List;

    impl<'de> Visitor<'de> for List {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of strings, or a comma-separated string")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
            Ok(value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(String::from)
                .collect())
        }

        fn visit_seq<A: SeqAccess<'de>>(self, items: A) -> Result<Self::Value, A::Error> {
            Vec::deserialize(de::value::SeqAccessDeserializer::new(items))
        }

        /// A table, such as one an indexed `APP__…__TRUSTED_PROXIES__0` variable makes, is an
        /// error. Its entries are first read as ignored values, so lib-config names each of them.
        fn visit_map<M: MapAccess<'de>>(self, mut entries: M) -> Result<Self::Value, M::Error> {
            while entries.next_entry::<String, de::IgnoredAny>()?.is_some() {}
            Err(de::Error::invalid_type(de::Unexpected::Map, &self))
        }
    }

    deserializer.deserialize_any(List)
}

#[derive(Deserialize, Debug, Clone)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: SecretString,
    pub from_email: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OtpConfig {
    pub otp_length: usize,
    pub email_verification_expiry_minutes: i64,
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
            otp: OtpConfig::from_env()?,
        })
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

        // Google OAuth 2.0 / OpenID Connect
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
                    user_info_url: "https://openidconnect.googleapis.com/v1/userinfo".to_string(),
                    scopes: vec![
                        "openid".to_string(),
                        "email".to_string(),
                        "profile".to_string(),
                    ],
                    response_type: "code".to_string(),
                    issuer_url: Some("https://accounts.google.com".to_string()),
                },
            );
        }

        // GitHub OAuth 2.0 (no OIDC support)
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
                    issuer_url: None, // GitHub doesn't support OIDC discovery
                },
            );
        }

        // X (Twitter) OAuth 2.0 (no OIDC support)
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
                    issuer_url: None, // Twitter doesn't support OIDC discovery
                },
            );
        }

        Ok(Self {
            providers,
            redirect_base: env::var("OAUTH_REDIRECT_BASE")
                .unwrap_or_else(|_| "http://localhost:8081".to_string()),
        })
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
            frontend_url: env::var("FRONTEND_URL")
                .unwrap_or_else(|_| "http://localhost:3000".to_string()),
            trusted_proxies: trusted_proxies_from_env().unwrap_or_default(),
        })
    }
}

/// `TRUSTED_PROXIES`, a comma-separated list of proxy addresses or CIDR ranges, if set.
pub fn trusted_proxies_from_env() -> Option<Vec<String>> {
    env::var("TRUSTED_PROXIES").ok().map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect()
    })
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

impl OtpConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            otp_length: env::var("OTP_LENGTH")
                .unwrap_or_else(|_| "6".to_string())
                .parse()?,
            email_verification_expiry_minutes: env::var("EMAIL_VERIFICATION_EXPIRY_MINUTES")
                .unwrap_or_else(|_| "15".to_string())
                .parse()?,
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

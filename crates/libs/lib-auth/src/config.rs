use lib_middleware::TrustedProxies;
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashMap;
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
    /// Optional, like `trusted_proxies`, its only setting.
    #[serde(default)]
    pub server: ServerConfig,
    pub email: EmailConfig,
    pub otp: OtpConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OAuthConfig {
    pub providers: HashMap<String, OAuthProviderConfig>,
    pub redirect_base: String,
    /// Where a completed web OAuth login lands. Required when `providers` is non-empty
    /// (lib-config validation).
    pub frontend_url_web: Option<String>,
    /// The mobile app's landing URL (usually a custom-scheme deep link) that receives the one-time
    /// login code. Required when `providers` is non-empty (lib-config validation).
    pub frontend_url_app: Option<String>,
}

/// The providers `auth.oauth.providers` may list: `OAuthService` fixes their scopes, user-info
/// endpoints and profile parsing (lib-config validation).
pub const OAUTH_PROVIDERS: [&str; 3] = ["google", "github", "twitter"];

/// An OAuth provider's client. The scopes it requests and the user-info endpoint it reads are
/// fixed per provider (`google`, `github`, `twitter`) in `OAuthService`.
#[derive(Deserialize, Debug, Clone)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub client_secret: SecretString,
    pub auth_url: String,
    pub token_url: String,
    /// Optional OpenID Connect discovery URL (e.g., for Google)
    pub issuer_url: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct ServerConfig {
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

/// `trusted_proxies`, the only list setting: a TOML array, or a comma-separated string, which is
/// how an `APP__` environment variable sets it
/// (`APP__AUTH__SERVER__TRUSTED_PROXIES=10.0.1.0/24,10.0.2.0/24`). Items are trimmed and empty
/// ones dropped, so an empty string is an empty list.
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

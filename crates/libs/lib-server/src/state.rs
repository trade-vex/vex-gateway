//! lib-server/src/state.rs
//!
//! Central application state containing all dependencies

use clickhouse::Client as ClickHouseClient;
use jsonwebtoken::{DecodingKey, EncodingKey};
use lib_auth::config::{AuthConfig, JwtConfig, RedisConfig};
use lib_auth::repository::{PostgresUserRepository, UserRepository};
use lib_auth::service::{EmailService, OAuthService, SessionService, TokenService};
use lib_config::ClickHouseConfig;
use lib_core::repository::repository::{ClickHousePortfolioRepository, PortfolioRepository};
use lib_core::{CorrelationService, EngineLink, OrderIdempotencyStore};
use lib_core::{
    OrderStore, PostgresTradingRepository, TradingRepository, UserBalanceStore,
    cache::scaling_cache::ScalingCache,
};
use lib_middleware::api_key::middleware::CacheConfig;
use lib_middleware::{
    ApiKeyMiddleware, SecurityType, TrustedProxies,
    repository::api_key_repository::{ApiKeyRepository, PostgresApiKeyRepository},
};
use redis::aio::ConnectionManager;
use reqwest::Client;
use sea_orm::DatabaseConnection;
use secrecy::ExposeSecret;
use std::sync::Arc;
use tracing::{debug, info};
use url::Url;

/// Central application state containing all dependencies
///
/// This struct follows the Dependency Injection pattern by centralizing
/// all app dependencies in one place for clean handler interfaces.
#[derive(Clone)]
pub struct AppState {
    // Database and infrastructure
    pub db: Arc<DatabaseConnection>,
    pub redis_conn: ConnectionManager,

    // DB Repository
    pub user_repo: Arc<dyn UserRepository>,
    pub api_key_repository: Arc<dyn ApiKeyRepository>,
    pub portfolio_repository: Arc<dyn PortfolioRepository>,
    pub trading_repository: Arc<dyn TradingRepository>,

    // Auth configuration
    pub jwt_config: JwtConfig,
    pub decoding_key: DecodingKey,
    pub token_service: TokenService,
    pub oauth_service: OAuthService,
    pub email_service: EmailService,
    pub session_service: SessionService,

    // The link to vex-core that orders are published on. Requests read it without a lock; the
    // engine link supervisor swaps it when it reconnects.
    pub engine_link: EngineLink,

    // API key middleware for dynamic updates
    pub api_key_middleware: ApiKeyMiddleware,

    // Proxies whose X-Forwarded-For is believed when resolving the client IP
    pub trusted_proxies: TrustedProxies,

    // Correlation service for matching responses to requests
    pub correlation_service: Arc<CorrelationService>,

    // Short-lived order placement outcomes keyed by client idempotency key
    pub order_idempotency_store: Arc<OrderIdempotencyStore>,

    // The API responses for the read model's balances and orders
    pub balance_store: UserBalanceStore,
    pub order_store: OrderStore,
}

impl AppState {
    /// Create a new AppState with all dependencies initialized
    pub async fn new(
        config: &AuthConfig,
        clickhouse: &ClickHouseConfig,
        engine_link: EngineLink,
        correlation_service: CorrelationService,
    ) -> Result<Arc<Self>, Box<dyn std::error::Error + Send + Sync>> {
        info!("Initializing App State");
        let trusted_proxies = config.server.parsed_trusted_proxies()?;

        // Initialize database connection
        let db = Arc::new(Self::init_database(&config.database).await?);

        // Initialize Redis connection
        let redis_conn = Self::init_redis(&config.redis).await?;
        let order_idempotency_store = Arc::new(OrderIdempotencyStore::new(redis_conn.clone()));

        // Initialize HTTP client
        let http_client = Self::init_http_client()?;

        // Initialize JWT keys
        let (encoding_key, decoding_key) = Self::init_jwt_keys(&config.jwt)?;

        // Initialize auth-specific services
        let user_repo = Arc::new(PostgresUserRepository::new(db.clone()));

        // Initialize API key repository
        let api_key_repository: Arc<dyn ApiKeyRepository> =
            Arc::new(PostgresApiKeyRepository::new(db.clone()));

        // Initialize portfolio repository (ClickHouse)
        let portfolio_repository: Arc<dyn PortfolioRepository> =
            Arc::new(Self::init_portfolio_repository(clickhouse));

        // Initialize API key middleware with database repository; signature replay protection
        // uses the gateway's Redis connection
        let api_key_middleware =
            Self::init_api_key_middleware(&api_key_repository, redis_conn.clone());

        // Initialize services
        let token_service = TokenService::new(
            Arc::new(config.jwt.clone()),
            redis_conn.clone(),
            Arc::new(encoding_key),
            Arc::new(decoding_key.clone()),
        );

        let oauth_service = OAuthService::new(
            redis_conn.clone(),
            http_client.clone(),
            &config.oauth,
            Arc::new(token_service.clone()),
            user_repo.clone(),
        )
        .await?;

        let email_service = EmailService::new(&config.email, redis_conn.clone(), &config.otp)?;

        // Initialize trading repository
        let trading_repo: Arc<dyn TradingRepository> =
            Arc::new(PostgresTradingRepository::new(db.clone()));

        // Initialize scaling cache and load scaling factors from database
        let scaling_cache = Arc::new(ScalingCache::new(db.clone()));
        scaling_cache
            .load_all()
            .await
            .map_err(|e| format!("Failed to load scaling cache: {}", e))?;
        info!("Scaling cache initialized and loaded");

        // Balances and orders come from the read model (trading_repo); these descale them
        let balance_store = UserBalanceStore::new(scaling_cache.clone());
        let order_store = OrderStore::new(scaling_cache);

        // Initialize session service with 30-minute inactivity TTL
        let session_service = SessionService::new(redis_conn.clone(), 30 * 60);

        tracing::info!("AppState initialization completed successfully");

        Ok(Arc::new(Self {
            db,
            redis_conn,
            user_repo,
            jwt_config: config.jwt.clone(),
            decoding_key,
            token_service,
            oauth_service,
            email_service,
            session_service,
            engine_link,
            api_key_middleware,
            trusted_proxies,
            api_key_repository,
            portfolio_repository,
            trading_repository: trading_repo,
            correlation_service: Arc::new(correlation_service),
            order_idempotency_store,
            balance_store,
            order_store,
        }))
    }

    /// Initialize JWT keys from configuration
    fn init_jwt_keys(
        config: &lib_auth::config::JwtConfig,
    ) -> Result<(EncodingKey, DecodingKey), Box<dyn std::error::Error + Send + Sync>> {
        use std::fs;
        use std::path::Path;

        let current_dir = std::env::current_dir().unwrap_or_else(|_| "unknown".into());
        info!("Current working directory: {:?}", current_dir);
        debug!("Looking for private key at: {}", config.private_key_path);
        debug!("Looking for public key at: {}", config.public_key_path);

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

        info!("Successfully loaded JWT keys");
        Ok((encoding_key, decoding_key))
    }

    /// Initialize database connection
    async fn init_database(
        config: &lib_auth::config::DatabaseConfig,
    ) -> Result<DatabaseConnection, Box<dyn std::error::Error + Send + Sync>> {
        use sea_orm::Database;
        let sanitized_url = Self::sanitize_url(&config.url);
        info!("Connecting to database: {}", sanitized_url);
        Database::connect(&config.url)
            .await
            .map_err(|e| format!("Failed to connect to database: {}", e).into())
    }

    /// The ClickHouse portfolio repository: `clickhouse.user` reading `clickhouse.database`, which
    /// is also the database each query names before its tables
    fn init_portfolio_repository(config: &ClickHouseConfig) -> ClickHousePortfolioRepository {
        info!(
            "Connecting to ClickHouse: {} (database {}, user {})",
            Self::sanitize_url(&config.url),
            config.database,
            config.user
        );
        let client = ClickHouseClient::default()
            .with_url(&config.url)
            .with_user(&config.user)
            .with_password(config.password.expose_secret())
            .with_database(&config.database);
        ClickHousePortfolioRepository::new_with_schema(client, &config.database)
    }

    /// Initialize Redis connection manager
    async fn init_redis(
        config: &RedisConfig,
    ) -> Result<ConnectionManager, Box<dyn std::error::Error + Send + Sync>> {
        let sanitized_url = Self::sanitize_url(&config.url);
        info!("Connecting to Redis: {}", sanitized_url);
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| format!("Failed to create Redis client: {}", e))?;
        ConnectionManager::new(client)
            .await
            .map_err(|e| format!("Failed to create Redis connection manager: {}", e).into())
    }

    /// Initialize HTTP client for Oauth service
    fn init_http_client() -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
        info!("Initializing HTTP client");
        Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e).into())
    }

    /// Create ApiKeyMiddleware using AppState dependencies; `redis_conn` (`auth.redis.url`) holds
    /// the signature replay guards
    pub fn init_api_key_middleware(
        api_key_repo: &Arc<dyn ApiKeyRepository>,
        redis_conn: ConnectionManager,
    ) -> ApiKeyMiddleware {
        // Create cache configuration
        // This should be loaded from config , hardcoded config should be removed
        let cache_config = CacheConfig {
            l1_max_size: 100000,
            l1_ttl: std::time::Duration::from_secs(300),
        };

        // Create middleware with database repository for dynamic loading
        ApiKeyMiddleware::new()
            .with_cache_config(cache_config)
            .with_repository(api_key_repo.clone())
            .with_replay_store(redis_conn)
            .with_default_security(SecurityType::Trade)
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

    /// Compile-time: the handlers share the one `EngineLink` directly. A lock around it would
    /// serialise every order and cancel publish across all workers, and it needs none: a request
    /// reads the current publisher without a lock and offers once to a concurrent Aeron
    /// publication through `&self`.
    #[test]
    fn handlers_share_the_engine_link_without_a_lock() {
        fn shared_directly(_field: fn(&AppState) -> &EngineLink) {}
        shared_directly(|state| &state.engine_link);
    }

    /// The repository built from `[clickhouse]` sends its user, password and database with a query
    /// and names that database before the table.
    #[tokio::test]
    async fn portfolio_repository_reads_the_configured_database_as_the_configured_user() {
        use lib_core::PnlResolution;
        use secrecy::SecretString;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // A ClickHouse stand-in that records the head of the first request and fails it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = ClickHouseConfig {
            url: format!("http://{}", listener.local_addr().unwrap()),
            user: "gateway_reader".to_string(),
            password: SecretString::new("Sup3r-S3cret".to_string()),
            database: "analytics".to_string(),
        };
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.windows(4).any(|end| end == b"\r\n\r\n") {
                let mut buffer = [0; 4096];
                let read = stream.read(&mut buffer).await.unwrap();
                assert_ne!(
                    read, 0,
                    "the connection closed before the request head ended"
                );
                head.extend_from_slice(&buffer[..read]);
            }
            stream
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8(head).unwrap()
        });

        let result = AppState::init_portfolio_repository(&config)
            .get_balance_chart(1, 0, PnlResolution::Day)
            .await;
        assert!(result.is_err());

        let head = server.await.unwrap();
        let header = |name: &str| {
            head.lines().find_map(|line| {
                let (key, value) = line.split_once(": ")?;
                key.eq_ignore_ascii_case(name).then_some(value)
            })
        };
        assert_eq!(header("X-ClickHouse-User"), Some("gateway_reader"));
        assert_eq!(header("X-ClickHouse-Key"), Some("Sup3r-S3cret"));

        // A short read-only query travels in the URL.
        let target = head.split(' ').nth(1).unwrap();
        let url = Url::parse(&format!("http://clickhouse{target}")).unwrap();
        let parameter = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        assert_eq!(parameter("database").as_deref(), Some("analytics"));
        let query = parameter("query").unwrap();
        assert!(
            query.contains("FROM analytics.balances_per_minute"),
            "{query}"
        );
    }

    #[test]
    fn test_sanitize_invalid_url() {
        let url = "not a valid url";
        let sanitized = AppState::sanitize_url(url);
        assert_eq!(sanitized, "<invalid-url>");
    }

    #[test]
    fn development_without_trusted_proxies_uses_the_peer_ip() {
        // The committed development example, which leaves trusted_proxies unset.
        let config_dir =
            std::env::temp_dir().join(format!("gw-dev-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("development.toml"),
            include_str!("../../../../config/development.toml.example"),
        )
        .unwrap();
        let config = lib_config::Config::load(
            lib_config::Environment::Development,
            Some(config_dir.clone()),
        );
        std::fs::remove_dir_all(&config_dir).unwrap();
        let config = config.expect("development needs no trusted proxies");
        assert!(config.auth.server.trusted_proxies.is_empty());

        // The same parse AppState::new applies to the configured list.
        let trusted_proxies = config.auth.server.parsed_trusted_proxies().unwrap();
        let req = actix_web::test::TestRequest::default()
            .peer_addr("192.0.2.10:1234".parse().unwrap())
            .append_header(("X-Forwarded-For", "203.0.113.20"))
            .to_http_request();

        assert_eq!(
            trusted_proxies.client_ip(&req),
            Some("192.0.2.10".parse().unwrap())
        );
    }
}

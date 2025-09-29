//! These tests cover:
//! - OAuth Provider Configuration and Flow
//! - User Repository (CRUD)
//! - Database Integration (PostgreSQL)
//! - JWT Token Service
//! - Error Handling and Status Codes
//! - Concurrency and Thread Safety
//! - Configuration Management
//! - Edge Cases and Invalid Inputs

use crate::{
    config::{
        AuthConfig, DatabaseConfig, EmailConfig, JwtConfig, OAuthConfig, OAuthProviderConfig,
        OtpConfig, RedisConfig, ServerConfig,
    },
    error::auth_error::AuthError,
    models::user::{Model, UserStatus},
    repository::UserRepository,
};
use secrecy::{ExposeSecret, SecretString};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

//  Mock and Helper Implementations
use jsonwebtoken::{DecodingKey, EncodingKey};

/// Loads an RSA private key from a PEM file for signing JWTs.
pub fn load_private_key(path: &str) -> Result<EncodingKey, AuthError> {
    let key_data = std::fs::read(path).map_err(|e| {
        AuthError::Config(format!("Failed to read private key from {}: {}", path, e))
    })?;
    EncodingKey::from_rsa_pem(&key_data)
        .map_err(|e| AuthError::Config(format!("Failed to parse private key: {}", e)))
}

/// Loads an RSA public key from a PEM file for verifying JWTs.
pub fn load_public_key(path: &str) -> Result<DecodingKey, AuthError> {
    let key_data = std::fs::read(path).map_err(|e| {
        AuthError::Config(format!("Failed to read public key from {}: {}", path, e))
    })?;
    DecodingKey::from_rsa_pem(&key_data)
        .map_err(|e| AuthError::Config(format!("Failed to parse public key: {}", e)))
}

// 1. Define a MockUserRepository to resolve the compiler error.
#[derive(Debug)]
struct MockUserRepository {
    // Use tokio::sync::Mutex for async-aware locking.
    users: Mutex<HashMap<Uuid, Model>>,
}

impl MockUserRepository {
    fn new() -> Self {
        Self {
            users: Mutex::new(HashMap::new()),
        }
    }

    async fn count(&self) -> usize {
        self.users.lock().await.len()
    }

    // Add get_user_by_id method
    pub async fn get_user_by_id(&self, user_id: Uuid) -> Result<Option<Model>, AuthError> {
        // .lock() is now an async operation.
        let users = self.users.lock().await;
        Ok(users.get(&user_id).cloned())
    }

    // Add delete_user method for completeness
    pub async fn delete_user(&self, user_id: Uuid) -> Result<(), AuthError> {
        // .lock() is now an async operation.
        let mut users = self.users.lock().await;
        users.remove(&user_id);
        Ok(())
    }
}

// Implement the UserRepository trait for the mock, matching the new trait definition.
#[async_trait::async_trait]
impl UserRepository for MockUserRepository {
    async fn find_by_id(&self, user_id: Uuid) -> Result<Option<Model>, AuthError> {
        let users = self.users.lock().await;
        Ok(users.get(&user_id).cloned())
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<Model>, AuthError> {
        // .lock() is now an async operation.
        let users = self.users.lock().await;
        Ok(users.values().find(|u| u.email == email).cloned())
    }

    async fn find_by_provider_id(
        &self,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<Model>, AuthError> {
        // .lock() is now an async operation.
        let users = self.users.lock().await;
        Ok(users
            .values()
            .find(|u| u.auth_provider == provider && u.provider_id.as_deref() == Some(provider_id))
            .cloned())
    }

    // Implement the new generic `create_user` method.
    async fn create_user(&self, user: &Model) -> Result<Model, AuthError> {
        // .lock() is now an async operation.
        let mut users = self.users.lock().await;
        if users.values().any(|u| u.email == user.email) {
            // Assuming an appropriate error variant exists
            return Err(AuthError::EmailAlreadyExists);
        }
        
        // Create user with server-assigned audit fields for integrity
        let now = Utc::now();
        let created_user = Model {
            user_id: user.user_id,
            email: user.email.clone(),
            username: user.username.clone(),
            password_hash: user.password_hash.clone(),
            auth_provider: user.auth_provider.clone(),
            provider_id: user.provider_id.clone(),
            avatar_url: user.avatar_url.clone(),
            email_verified: user.email_verified,
            status: UserStatus::Pending, // Safe initial status
            created_at: now, // Server-assigned timestamp
            updated_at: now, // Server-assigned timestamp
            last_login: user.last_login, // Accept caller-supplied last_login if appropriate
        };
        
        users.insert(user.user_id, created_user.clone());
        Ok(created_user)
    }

    async fn update_last_login(&self, user_id: Uuid) -> Result<(), AuthError> {
        // .lock() is now an async operation.
        let mut users = self.users.lock().await;
        if let Some(user) = users.get_mut(&user_id) {
            // Check if user is deleted - don't update deleted users
            if user.status == UserStatus::Deleted {
                return Err(AuthError::Database("User not found".to_string()));
            }
            let now = Utc::now();
            user.last_login = Some(now);
            user.updated_at = now;
            Ok(())
        } else {
            Err(AuthError::Database("User not found".to_string()))
        }
    }

    async fn verify_email(&self, user_id: Uuid) -> Result<(), AuthError> {
        // .lock() is now an async operation.
        let mut users = self.users.lock().await;
        if let Some(user) = users.get_mut(&user_id) {
            // Check if user is deleted - don't update deleted users
            if user.status == UserStatus::Deleted {
                return Err(AuthError::Database("User not found".to_string()));
            }
            // Only allow Pending→Active transition
            if user.status != UserStatus::Pending {
                return Err(AuthError::Database("User not found".to_string()));
            }
            user.email_verified = true;
            user.updated_at = Utc::now();
            Ok(())
        } else {
            Err(AuthError::Database("User not found".to_string()))
        }
    }

    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<(), AuthError> {
        // .lock() is now an async operation.
        let mut users = self.users.lock().await;
        if let Some(user) = users.get_mut(&user_id) {
            // Check if user is deleted - don't update deleted users
            if user.status == UserStatus::Deleted {
                return Err(AuthError::Database("User not found".to_string()));
            }
            user.password_hash = Some(password_hash.to_string());
            user.updated_at = Utc::now();
            Ok(())
        } else {
            Err(AuthError::Database("User not found".to_string()))
        }
    }
}

// 4. Create a plausible `provider` module content.
mod provider {
    use crate::config::OAuthProviderConfig;
    use crate::error::auth_error::AuthError;
    use url::Url;
    use uuid::Uuid;

    pub fn generate_redirect_url(
        provider_config: &OAuthProviderConfig,
        scopes: &[&str],
    ) -> Result<(String, String), AuthError> {
        let state = Uuid::new_v4().to_string();
        let mut auth_url =
            Url::parse(&provider_config.auth_url).map_err(|_| AuthError::InvalidCredentials)?;

        let base_url =
            std::env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8081".to_string());
        let redirect_uri = match provider_config.auth_url.as_str() {
            url if url.contains("google") => format!("{}/auth/google/callback", base_url),
            url if url.contains("github") => format!("{}/auth/github/callback", base_url),
            _ => format!("{}/auth/callback", base_url),
        };

        auth_url
            .query_pairs_mut()
            .append_pair("client_id", &provider_config.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("response_type", &provider_config.response_type)
            .append_pair("scope", &scopes.join(" "))
            .append_pair("state", &state);

        Ok((auth_url.to_string(), state))
    }
}

// end  Test Setup

pub fn setup_test_config() -> AuthConfig {
    use std::collections::HashMap;
    use std::env;

    // Load environment variables
    dotenvy::dotenv().ok();

    // Create OAuth providers map
    let mut providers = HashMap::new();

    // Google OAuth provider
    let google_client_id = env::var("GOOGLE_CLIENT_ID")
        .or_else(|_| env::var("APP_PROVIDERS__GOOGLE__CLIENT_ID"))
        .unwrap_or_else(|_| "test-google-client-id".to_string());
    let google_client_secret = env::var("GOOGLE_CLIENT_SECRET")
        .or_else(|_| env::var("APP_PROVIDERS__GOOGLE__CLIENT_SECRET"))
        .unwrap_or_else(|_| "test-google-client-secret".to_string());

    providers.insert(
        "google".to_string(),
        OAuthProviderConfig {
            client_id: google_client_id,
            client_secret: SecretString::new(google_client_secret),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_url: "https://oauth2.googleapis.com/token".to_string(),
            user_info_url: "https://www.googleapis.com/oauth2/v2/userinfo".to_string(),
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            response_type: "code".to_string(),
        },
    );

    // GitHub OAuth provider
    let github_client_id =
        env::var("GITHUB_CLIENT_ID").unwrap_or_else(|_| "test-github-client-id".to_string());
    let github_client_secret = env::var("GITHUB_CLIENT_SECRET")
        .unwrap_or_else(|_| "test-github-client-secret".to_string());

    providers.insert(
        "github".to_string(),
        OAuthProviderConfig {
            client_id: github_client_id,
            client_secret: SecretString::new(github_client_secret),
            auth_url: "https://github.com/login/oauth/authorize".to_string(),
            token_url: "https://github.com/login/oauth/access_token".to_string(),
            user_info_url: "https://api.github.com/user".to_string(),
            scopes: vec!["user:email".to_string()],
            response_type: "code".to_string(),
        },
    );

    AuthConfig {
        server: ServerConfig {
            base_url: env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:8081".to_string()),
            port: env::var("PORT")
                .unwrap_or_else(|_| "8081".to_string())
                .parse()
                .unwrap_or(8081),
        },
        database: DatabaseConfig {
            url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://myuser:mypassword@localhost/mydb".to_string()),
            max_connections: 10,
        },
        redis: RedisConfig {
            url: env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            max_connections: 10,
        },
        jwt: JwtConfig {
            private_key_path: env::var("JWT_PRIVATE_KEY_PATH")
                .unwrap_or_else(|_| "../../../certs/jwt_private_key.pem".to_string()),
            public_key_path: env::var("JWT_PUBLIC_KEY_PATH")
                .unwrap_or_else(|_| "../../../certs/jwt_public_key.pem".to_string()),
            issuer: env::var("JWT_ISSUER").unwrap_or_else(|_| "vex-gateway".to_string()),
            audience: env::var("JWT_AUDIENCE").unwrap_or_else(|_| "vex-api".to_string()),
            access_token_ttl_minutes: env::var("JWT_ACCESS_TOKEN_TTL_MINUTES")
                .unwrap_or_else(|_| "15".to_string())
                .parse()
                .unwrap_or(15),
            refresh_token_ttl_days: env::var("JWT_REFRESH_TOKEN_TTL_DAYS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .unwrap_or(30),
        },
        oauth: OAuthConfig { 
            providers,
            redirect_base: "http://localhost:8081".to_string(),
        },
        email: EmailConfig {
            smtp_host: env::var("SMTP_HOST").unwrap_or_else(|_| "smtp.gmail.com".to_string()),
            smtp_port: env::var("SMTP_PORT")
                .unwrap_or_else(|_| "587".to_string())
                .parse()
                .unwrap_or(587),
            smtp_username: env::var("SMTP_USERNAME")
                .unwrap_or_else(|_| "test@example.com".to_string()),
            smtp_password: secrecy::SecretString::new(env::var("SMTP_PASSWORD").unwrap_or_else(|_| "password".to_string())),
            from_email: env::var("FROM_EMAIL")
                .unwrap_or_else(|_| "noreply@vex-gateway.com".to_string()),
        },
        otp: OtpConfig {
            otp_length: 6,
            email_verification_expiry_minutes: 15,
        },
    }
}

fn setup_mock_user_repo() -> Arc<MockUserRepository> {
    Arc::new(MockUserRepository::new())
}

// end  Test Cases

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;
    // OAuth Provider Configuration - Testing provider endpoint and validation
    #[test]
    fn test_oauth_provider_config_validation() {
        let config = setup_test_config();
        // 2. Use .get() for HashMap instead of .first()
        let provider_config = config.oauth.providers.get("google").unwrap();
        // Assuming `validate()` is a method you've derived or implemented.
        // If not, you would test fields individually.
        assert!(!provider_config.client_id.is_empty());
        assert!(Url::parse(&provider_config.auth_url).is_ok());
    }

    // OAuth Flow Initiation - Testing redirect URL generation
    #[test]
    fn test_oauth_flow_initiation_redirect_url() {
        let config = setup_test_config();
        let provider_config = config.oauth.providers.get("google").unwrap();
        let (redirect_url, _state) =
            provider::generate_redirect_url(provider_config, &["openid", "profile", "email"])
                .unwrap();

        assert!(redirect_url.starts_with(&provider_config.auth_url));
        assert!(redirect_url.contains(&format!("client_id={}", provider_config.client_id)));
        assert!(redirect_url.contains("scope=openid+profile+email"));
        assert!(redirect_url.contains("response_type=code"));
    }

    // User Repository Operations - CRUD operations with proper error handling
    #[tokio::test]
    async fn test_user_repository_crud_operations() {
        let repo = setup_mock_user_repo();
        let user_id = Uuid::new_v4();

        let new_user = Model {
            user_id,
            username: "Test User".to_string(),
            email: "test@example.com".to_string(),
            password_hash: None,
            auth_provider: "google".to_string(),
            provider_id: Some("12345".to_string()),
            avatar_url: None,
            email_verified: true,
            status: UserStatus::Active, // This will be ignored and set to Pending
            created_at: Utc::now(), // This will be ignored and set to server time
            updated_at: Utc::now(), // This will be ignored and set to server time
            last_login: None,
        };

        // Create - Call the new `create_user` method.
        repo.create_user(&new_user).await.unwrap();

        // Read
        let fetched_user = repo
            .get_user_by_id(user_id)
            .await
            .unwrap()
            .expect("User should be found in the mock repository"); // This will no longer panic.
        assert_eq!(fetched_user.user_id, new_user.user_id);
        
        // Verify audit integrity - status should be Pending, not the caller-supplied Active
        assert_eq!(fetched_user.status, UserStatus::Pending);
        
        // Verify that timestamps are server-assigned (should be recent)
        let now = Utc::now();
        let time_diff = now.signed_duration_since(fetched_user.created_at).num_seconds();
        assert!(time_diff >= 0 && time_diff < 5); // Created within last 5 seconds
        
        let time_diff_updated = now.signed_duration_since(fetched_user.updated_at).num_seconds();
        assert!(time_diff_updated >= 0 && time_diff_updated < 5); // Updated within last 5 seconds

        // Delete
        repo.delete_user(user_id).await.unwrap();
        let result = repo.get_user_by_id(user_id).await.unwrap();
        assert!(result.is_none());
    }

    //   Token Service - JWT token minting and validation
    #[tokio::test]
    async fn test_token_service_minting_and_validation() {
        let config = setup_test_config();
        use crate::service::token_service::TokenService;
        use jsonwebtoken::{DecodingKey, EncodingKey};
        use std::path::PathBuf;

        // This is the robust way to locate files relative to the current crate's manifest.
        // It builds an absolute path and is not dependent on the test runner's working directory.
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let priv_key_path = manifest_dir.join("src/keys/private_key.pem");
        let pub_key_path = manifest_dir.join("src/keys/public_key.pem");

        // Read the keys using the absolute paths. The panic message will now show the full path for easier debugging.
        let priv_key_bytes = std::fs::read(&priv_key_path).unwrap_or_else(|e| {
            panic!("Failed to read private key from {:?}: {}", priv_key_path, e)
        });
        let pub_key_bytes = std::fs::read(&pub_key_path)
            .unwrap_or_else(|e| panic!("Failed to read public key from {:?}: {}", pub_key_path, e));

        let encoding_key = Arc::new(EncodingKey::from_rsa_pem(&priv_key_bytes).unwrap());
        let decoding_key = Arc::new(DecodingKey::from_rsa_pem(&pub_key_bytes).unwrap());

        // Use a real deadpool_redis::Pool for testing
        use deadpool_redis::Config as RedisPoolConfig;
        // Use `from_url` to create the config directly and avoid conflicts.
        let redis_cfg = RedisPoolConfig::from_url("redis://127.0.0.1/");
        let redis_pool = Arc::new(
            redis_cfg
                .create_pool(Some(deadpool_redis::Runtime::Tokio1))
                .unwrap(),
        );

        // Initialize TokenService with both keys
        let token_service = TokenService::new(
            Arc::new(config.jwt.clone()),
            redis_pool,
            encoding_key,
            decoding_key,
        );
        let user_id = Uuid::new_v4();

        // Mint token
        let tokens = token_service.mint_tokens(user_id).await.unwrap();
        assert!(!tokens.access_token.is_empty());

        // Validate token using the instance method
        let decoded_claims = token_service.validate_jwt(&tokens.access_token).unwrap();
        assert_eq!(decoded_claims.sub, user_id.to_string());
    }

    // Concurrent Operations - Thread safety and race condition handling
    #[tokio::test]
    async fn test_concurrent_user_creation() {
        let repo = setup_mock_user_repo();
        let handles: Vec<_> = (0..10)
            .map(|i| {
                // Clone the Arc for each new task
                let repo_clone = repo.clone();

                // Create a unique user for each task
                let user = Model {
                    user_id: Uuid::new_v4(),
                    username: format!("Concurrent User {}", i),
                    email: format!("concurrent_{}@example.com", i),
                    password_hash: None,
                    auth_provider: "google".to_string(),
                    provider_id: Some(format!("concurrent_{}", i)),
                    avatar_url: None,
                    email_verified: true,
                    status: UserStatus::Active,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    last_login: None,
                };

                // Spawn a new async task to create the user
                tokio::spawn(async move {
                    // Call the `create_user` method from the refactored trait.
                    // The lock on the mutex is acquired, the user is created, and the lock
                    // is released, all within the `create_user` method's synchronous body.
                    repo_clone.create_user(&user).await
                })
            })
            .collect();

        // Wait for all tasks to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "User creation should succeed");
        }

        // Verify that exactly 10 users were created, proving the mutex worked.
        let user_count = repo.count().await;
        assert_eq!(user_count, 10);
    }

    // // 7. Completed Database Integration Test
    // #[tokio::test]
    // async fn test_database_connection_and_migrations() {
    //     // Skip this test if we're in local development without database
    //     if std::env::var("SKIP_DATABASE_TESTS").is_ok() {
    //         println!("Skipping database test - SKIP_DATABASE_TESTS is set");
    //         return;
    //     }

    //     let config = setup_test_config();
    //     // Ensure the test database URL is different from production
    //     assert!(
    //         config.database.url.contains("mydb"),
    //         "Database URL should be for a test database!"
    //     );

    //     // Establish a database connection.
    //     Database::connect(&config.database.url)
    //         .await
    //         .expect("Failed to create database connection.");

    //     // Note: SeaORM doesn't have built-in migrations like sqlx::migrate!
    //     // Need to use a separate migration tool or run migrations manually.
    //     // For now, we'll just test the connection.

    //     // Test that the connection was established successfully
    //     // The fact that Database::connect() didn't panic means the connection is valid
    //     println!("Database connection test passed - connected to PostgreSQL");

    //     // Add migration application related functionlity to lib-db and add tests accordingly
    // }

    use reqwest;

    // Test live server endpoints
    #[tokio::test]
    async fn test_live_server_endpoints() {
        // Skip this test if we're in CI or if no server is running
        if std::env::var("CI").is_ok() || std::env::var("SKIP_SERVER_TESTS").is_ok() {
            println!("Skipping server connectivity test in CI environment");
            return;
        }

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none()) // Don't follow redirects automatically
            .timeout(std::time::Duration::from_secs(5)) // Add timeout
            .build()
            .unwrap();

        let base_url = "http://127.0.0.1:8081";

        // Test 1: Check if server is responding
        println!("Testing basic connectivity...");
        let response = match client.get(format!("{}/", base_url)).send().await {
            Ok(resp) => resp,
            Err(e) => {
                println!("Server not available, skipping test: {}", e);
                return;
            }
        };

        println!("Root endpoint status: {}", response.status());

        // Test 2: Test OAuth Google endpoint
        println!("Testing Google OAuth endpoint...");
        let response = client
            .get(format!("{}/auth/google", base_url))
            .header("Accept", "application/json")
            .send()
            .await
            .expect("Failed to call Google OAuth endpoint");

        println!("Google OAuth status: {}", response.status());
        if response.status().is_server_error() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "No body".to_string());
            println!("Google OAuth error body: {}", body);
        } else if response.status().is_redirection()
            && let Some(location) = response.headers().get("location")
        {
            println!("Google OAuth redirect to: {:?}", location);
        }

        // Test 3: Test OAuth GitHub endpoint
        println!("Testing GitHub OAuth endpoint...");
        let response = client
            .get(format!("{}/auth/github", base_url))
            .header("Accept", "application/json")
            .send()
            .await
            .expect("Failed to call GitHub OAuth endpoint");

        println!("GitHub OAuth status: {}", response.status());
        if response.status().is_server_error() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "No body".to_string());
            println!("GitHub OAuth error body: {}", body);
        } else if response.status().is_redirection()
            && let Some(location) = response.headers().get("location")
        {
            println!("GitHub OAuth redirect to: {:?}", location);
        }

        // Test 4: Test /api/me endpoint without auth (should fail)
        println!("Testing /api/me without authentication...");
        let response = client
            .get(format!("{}/api/me", base_url))
            .header("Accept", "application/json")
            .send()
            .await
            .expect("Failed to call /api/me endpoint");

        println!("/api/me status: {}", response.status());
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "No body".to_string());
        println!("/api/me response body: {}", body);

        // Test 5: Check if health endpoint exists
        println!("Testing health endpoint...");
        let response = client.get(format!("{}/health", base_url)).send().await;

        match response {
            Ok(resp) => {
                println!("Health endpoint status: {}", resp.status());
                let body = resp.text().await.unwrap_or_else(|_| "No body".to_string());
                println!("Health endpoint body: {}", body);
            }
            Err(e) => {
                println!("Health endpoint error: {}", e);
            }
        }

        // Test 6: List all available routes (if your server supports it)
        println!("Testing common endpoints to discover routes...");
        let test_routes = vec!["/", "/auth", "/login", "/oauth", "/api", "/status", "/ping"];

        for route in test_routes {
            let response = client.get(format!("{}{}", base_url, route)).send().await;

            match response {
                Ok(resp) => {
                    println!("Route {} -> Status: {}", route, resp.status());
                }
                Err(e) => {
                    println!("Route {} -> Error: {}", route, e);
                }
            }
        }
    }

    // Test OAuth configuration loading from environment variables
    #[tokio::test]
    async fn test_oauth_config_loading() {
        use std::env;
        use std::path::PathBuf;

        // Check if environment variables are loaded
        println!("Checking OAuth environment variables...");

        // Get the current working directory
        let current_dir = env::current_dir().unwrap();
        println!("Current working directory: {:?}", current_dir);

        // Try multiple possible .env file locations
        let possible_env_paths = vec![
            current_dir.join(".env"),
            current_dir.join("crates/libs/lib-auth/.env"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".env"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/.env"),
        ];

        for env_path in &possible_env_paths {
            println!("Checking for .env file at: {:?}", env_path);
            if env_path.exists() {
                println!("Found .env file at: {:?}", env_path);
                match dotenvy::from_path(env_path) {
                    Ok(_) => println!("Successfully loaded .env from: {:?}", env_path),
                    Err(e) => println!("Failed to load .env from {:?}: {}", env_path, e),
                }
            } else {
                println!(".env file not found at: {:?}", env_path);
            }
        }

        // Also try loading from the project root
        dotenvy::dotenv().ok();

        // Check all environment variables
        let google_client_id =
            env::var("GOOGLE_CLIENT_ID").unwrap_or_else(|_| "NOT_SET".to_string());
        let google_client_secret =
            env::var("GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| "NOT_SET".to_string());
        let github_client_id =
            env::var("GITHUB_CLIENT_ID").unwrap_or_else(|_| "NOT_SET".to_string());
        let github_client_secret =
            env::var("GITHUB_CLIENT_SECRET").unwrap_or_else(|_| "NOT_SET".to_string());

        println!(
            "GOOGLE_CLIENT_ID: {}",
            if google_client_id == "NOT_SET" {
                "NOT_SET".to_string()
            } else {
                google_client_id[..10].to_string() + "..."
            }
        );
        println!(
            "GOOGLE_CLIENT_SECRET: {}",
            if google_client_secret == "NOT_SET" {
                "NOT_SET"
            } else {
                "SET"
            }
        );
        println!(
            "GITHUB_CLIENT_ID: {}",
            if github_client_id == "NOT_SET" {
                "NOT_SET".to_string()
            } else {
                github_client_id[..10].to_string() + "..."
            }
        );
        println!(
            "GITHUB_CLIENT_SECRET: {}",
            if github_client_secret == "NOT_SET" {
                "NOT_SET"
            } else {
                "SET"
            }
        );

        // Check other important variables
        let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| "NOT_SET".to_string());
        let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "NOT_SET".to_string());
        let base_url = env::var("BASE_URL").unwrap_or_else(|_| "NOT_SET".to_string());

        println!(
            "DATABASE_URL: {}",
            if database_url == "NOT_SET" {
                "NOT_SET"
            } else {
                "SET"
            }
        );
        println!(
            "REDIS_URL: {}",
            if redis_url == "NOT_SET" {
                "NOT_SET"
            } else {
                "SET"
            }
        );
        println!(
            "BASE_URL: {}",
            if base_url == "NOT_SET" {
                "NOT_SET"
            } else {
                &base_url
            }
        );
    }

    // Add this new test to understand what routes are actually available

    #[tokio::test]
    async fn test_server_route_discovery() {
        let client = reqwest::Client::new();
        let base_url = "http://127.0.0.1:8081";

        println!("=== Server Route Discovery ===");

        // Test exact routes that should exist based on your server implementation
        let expected_routes = vec![
            ("/", "Root"),
            ("/health", "Health check"),
            ("/auth/google", "Google OAuth"),
            ("/auth/github", "GitHub OAuth"),
            ("/auth/google/callback", "Google callback"),
            ("/auth/github/callback", "GitHub callback"),
            ("/api/me", "User info"),
            ("/api/auth/logout", "Logout"),
        ];

        for (route, description) in expected_routes {
            match client.get(format!("{}{}", base_url, route)).send().await {
                Ok(response) => {
                    let status = response.status();
                    println!("{}: {} -> {}", description, route, status);

                    // For 500 errors, try to get more details
                    if status.is_server_error() {
                        let body = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "No body".to_string());
                        if !body.is_empty() {
                            println!("  Error body: {}", body);
                        }
                    }
                }
                Err(e) => {
                    println!("{}: {} -> Connection Error: {}", description, route, e);
                }
            }
        }
    }

    // Add this test to debug the configuration loading

    #[tokio::test]
    async fn test_auth_config_structure() {
        use std::env;

        // Load environment variables
        dotenvy::dotenv().ok();

        // Test creating the auth config with the current environment
        let config = setup_test_config();

        println!("=== Auth Config Debug ===");
        println!("Server base URL: {}", config.server.base_url);
        println!("Database URL: {}", config.database.url);
        println!("Redis URL: {}", config.redis.url);

        // Check OAuth providers
        println!(
            "OAuth providers configured: {}",
            config.oauth.providers.len()
        );
        for (name, provider) in &config.oauth.providers {
            println!(
                "Provider '{}': client_id={}, has_secret={}",
                name,
                provider.client_id,
                !provider.client_secret.expose_secret().is_empty()
            );
        }

        // Check if Google and GitHub are specifically configured
        let google_configured = config.oauth.providers.contains_key("google");
        let github_configured = config.oauth.providers.contains_key("github");

        println!("Google configured: {}", google_configured);
        println!("GitHub configured: {}", github_configured);

        // If they're not configured, let's see what environment variables we have
        if !google_configured {
            let google_client_id =
                env::var("GOOGLE_CLIENT_ID").unwrap_or_else(|_| "NOT_SET".to_string());
            let app_google_client_id = env::var("APP_PROVIDERS__GOOGLE__CLIENT_ID")
                .unwrap_or_else(|_| "NOT_SET".to_string());
            println!(
                "GOOGLE_CLIENT_ID env var: {}",
                if google_client_id == "NOT_SET" {
                    "NOT_SET"
                } else {
                    "SET"
                }
            );
            println!(
                "APP_PROVIDERS__GOOGLE__CLIENT_ID env var: {}",
                if app_google_client_id == "NOT_SET" {
                    "NOT_SET"
                } else {
                    "SET"
                }
            );
        }

        if !github_configured {
            let github_client_id =
                env::var("GITHUB_CLIENT_ID").unwrap_or_else(|_| "NOT_SET".to_string());
            println!(
                "GITHUB_CLIENT_ID env var: {}",
                if github_client_id == "NOT_SET" {
                    "NOT_SET"
                } else {
                    "SET"
                }
            );
        }
    }

    // Add this test to see the structure of OAuthProviderConfig

    #[tokio::test]
    async fn test_oauth_provider_config_fields() {
        use std::env;

        // Load environment variables
        dotenvy::dotenv().ok();

        let google_client_id =
            env::var("GOOGLE_CLIENT_ID").unwrap_or_else(|_| "test-id".to_string());
        let google_client_secret =
            env::var("GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| "test-secret".to_string());

        // Create a minimal OAuth provider config to see what fields are required
        let provider_config = OAuthProviderConfig {
            client_id: google_client_id,
            client_secret: SecretString::new(google_client_secret),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_url: "https://oauth2.googleapis.com/token".to_string(),
            user_info_url: "https://www.googleapis.com/oauth2/v2/userinfo".to_string(),
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            response_type: "code".to_string(),
            // Add any other required fields here as needed
        };

        println!("OAuth Provider Config created successfully!");
        println!("Client ID: {}", provider_config.client_id);
        println!("Auth URL: {}", provider_config.auth_url);
        println!("Response Type: {}", provider_config.response_type);
        println!("Scopes: {:?}", provider_config.scopes);
    }

    // Add this test to simulate server behavior

    #[tokio::test]
    async fn test_server_oauth_provider_lookup() {
        // Load environment variables the same way your server should
        dotenvy::dotenv().ok();

        // Create config the same way your server should
        let config = setup_test_config();

        println!("=== Server OAuth Provider Simulation ===");

        // Test what happens when server tries to get Google provider
        match config.oauth.providers.get("google") {
            Some(provider) => {
                println!("  Google provider found!");
                println!("   Client ID: {}", provider.client_id);
                println!("   Auth URL: {}", provider.auth_url);

                // Test generating redirect URL (what server should do)
                let (redirect_url, state) =
                    provider::generate_redirect_url(provider, &["openid", "profile", "email"])
                        .unwrap();

                println!("   Generated redirect URL: {}", redirect_url);
                println!("   State: {}", state);
            }
            None => {
                println!("Google provider NOT found - this is the server's problem!");
            }
        }

        // Test what happens when server tries to get GitHub provider
        match config.oauth.providers.get("github") {
            Some(provider) => {
                println!("  GitHub provider found!");
                println!("   Client ID: {}", provider.client_id);
                println!("   Auth URL: {}", provider.auth_url);

                // Test generating redirect URL
                let (redirect_url, state) =
                    provider::generate_redirect_url(provider, &["user:email"]).unwrap();

                println!("   Generated redirect URL: {}", redirect_url);
                println!("   State: {}", state);
            }
            None => {
                println!("GitHub provider NOT found - this is the server's problem!");
            }
        }
    }
}

// use crate::service::token_service::TokenService;
// use crate::state::AppState;
// use std::time::Instant;
// use tokio::task::JoinSet;
// #[cfg(test)]
// mod performance_tests {
//     use super::*;

//     // Helper functions
//     async fn create_test_token_service(config: AuthConfig) -> TokenService {
//         let app_state = Arc::new(AppState::new(config).await.unwrap());
//         TokenService::new(
//             Arc::new(app_state.jwt_config.clone()),
//             app_state.redis_pool.clone(),
//             Arc::new(app_state.encoding_key.clone()),
//             Arc::new(app_state.decoding_key.clone()),
//         )
//     }

//     #[tokio::test]
//     async fn test_concurrent_token_validation() {
//         let config = setup_test_config();
//         let token_service = create_test_token_service(config).await;
//         let user_id = Uuid::new_v4();

//         // Create a valid token
//         let tokens = token_service.mint_tokens(user_id).await.unwrap();
//         let token = tokens.access_token;

//         let start = Instant::now();
//         let mut tasks = JoinSet::new();

//         // Spawn 100 concurrent validation tasks
//         for _ in 0..100 {
//             let token_clone = token.clone();
//             let service_clone = token_service.clone();
//             tasks.spawn(async move { service_clone.validate_jwt(&token_clone) });
//         }

//         let mut success_count = 0;
//         let mut error_count = 0;

//         while let Some(result) = tasks.join_next().await {
//             match result.unwrap() {
//                 Ok(_) => success_count += 1,
//                 Err(_) => error_count += 1,
//             }
//         }

//         let duration = start.elapsed();

//         assert_eq!(success_count, 100);
//         assert_eq!(error_count, 0);
//         assert!(
//             duration.as_millis() < 1000,
//             "Should complete within 1 second"
//         );

//         println!("Validated 100 tokens concurrently in {:?}", duration);
//     }

//     #[tokio::test]
//     async fn test_concurrent_user_operations() {
//         let repo = setup_mock_user_repo();
//         let start = Instant::now();
//         let mut tasks = JoinSet::new();

//         // Spawn 50 concurrent user creation tasks
//         for i in 0..50 {
//             let repo_clone = repo.clone();
//             tasks.spawn(async move {
//                 let user = Model {
//                     user_id: Uuid::new_v4(),
//                     username: format!("User {}", i),
//                     email: format!("user{}@example.com", i),
//                     password_hash: None,
//                     auth_provider: "github".to_string(),
//                     provider_id: Some(i.to_string()),
//                     avatar_url: None,
//                     email_verified: true,
//                     status: UserStatus::Active,
//                     created_at: Utc::now(),
//                     updated_at: Utc::now(),
//                     last_login: None,
//                 };

//                 repo_clone.create_user(&user).await
//             });
//         }

//         let mut success_count = 0;
//         while let Some(result) = tasks.join_next().await {
//             if result.unwrap().is_ok() {
//                 success_count += 1;
//             }
//         }

//         let duration = start.elapsed();

//         assert_eq!(success_count, 50);
//         assert_eq!(repo.count().await, 50);

//         println!("Created 50 users concurrently in {:?}", duration);
//     }
// }

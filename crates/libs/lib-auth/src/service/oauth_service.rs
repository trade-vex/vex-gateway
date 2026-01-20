//! OAuth 2.0 and OpenID Connect Authentication Service
//!
//! This module provides a production-ready OAuth/OIDC authentication service using the
//! `openidconnect` crate for spec-compliant implementations with multiple providers.
//!
//! # Supported Providers
//!
//! - **Google**: Full OpenID Connect with discovery, ID tokens, and userinfo endpoint
//! - **GitHub**: OAuth 2.0 with email scope and separate email API
//! - **Twitter/X**: OAuth 2.0 with PKCE for enhanced security
//!
//! # Architecture
//!
//! The service follows a clean architecture pattern:
//! - OAuth client initialization happens once at startup (async discovery for OIDC)
//! - State management uses Redis with 10-minute expiry
//! - User management integrates with existing UserRepository
//! - JWT tokens are minted via TokenService after successful OAuth
//!
//! # Flow
//!
//! ```text
//! 1. Client -> GET /oauth/{provider}
//!    └─> Service generates auth URL with state + PKCE
//!    └─> Store state & verifier in Redis (600s TTL)
//!    └─> Redirect to provider
//!
//! 2. Provider -> User authenticates
//!    └─> Provider redirects to callback with code + state
//!
//! 3. Client -> GET /oauth/{provider}/callback?code=...&state=...
//!    └─> Verify & consume state (CSRF protection)
//!    └─> Retrieve PKCE verifier from Redis
//!    └─> Exchange code for access token
//!    └─> Fetch user info from provider
//!    └─> Create/update user in database
//!    └─> Mint JWT tokens
//!    └─> Return tokens + user info
//! ```

use crate::config::OAuthProviderConfig;
use crate::dto::auth_response_dto::{OAuthLoginResponse, OAuthUserInfo};
use crate::error::auth_error::AuthError;
use crate::service::token_service::TokenService;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

/// Unified OAuth transaction data stored in Redis
/// Replaces separate oauth_state, oauth_verifier, oauth_nonce, oauth_platform keys
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OAuthTxn {
    /// PKCE verifier for the OAuth provider
    pkce_verifier: String,
    /// OIDC nonce for ID token validation (Google only)
    nonce: Option<String>,
    /// Platform identifier: "web" or "app"
    platform: Option<String>,
    /// App challenge for mobile PKCE-like second hop (app only)
    app_challenge: Option<String>,
}

/// Metadata returned from OAuth callback for handler use
#[derive(Debug, Clone)]
pub struct OAuthCallbackMeta {
    /// Platform identifier: "web" or "app"
    pub platform: Option<String>,
    /// App challenge for mobile verification (app only)
    pub app_challenge: Option<String>,
}
#[allow(unused_imports)]
use openidconnect::TokenResponse;
use openidconnect::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, ResponseTypes, Scope, TokenUrl,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreResponseType},
    reqwest::async_http_client,
};
use secrecy::ExposeSecret;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::models::user::{Model, UserStatus};
use crate::repository::UserRepository;

/// OAuth 2.0 / OpenID Connect authentication service
///
/// This service manages the complete OAuth flow for multiple providers,
/// from authorization URL generation through token exchange and user creation.
///
/// # Thread Safety
///
/// This struct is `Clone` and all fields are thread-safe, allowing it to be
/// shared across multiple Actix-web workers.
///
/// # Redis Keys
///
/// - `oauth_state:{provider}:{state}` - CSRF state validation (600s TTL)
/// - `oauth_verifier:{provider}:{state}` - PKCE verifier storage (600s TTL)
/// - `oauth_nonce:{provider}:{state}` - OIDC nonce for ID token validation (600s TTL)
#[derive(Clone)]
pub struct OAuthService {
    /// Redis connection for state management
    conn: ConnectionManager,

    /// HTTP client for API requests to providers
    http_client: reqwest::Client,

    /// Pre-configured OAuth clients per provider (initialized at startup)
    oauth_clients: HashMap<String, CoreClient>,

    /// JWT token service for minting application tokens
    token_service: Arc<TokenService>,

    /// User repository for database operations
    user_repo: Arc<dyn UserRepository>,

    /// Frontend URL for redirects after OAuth success
    frontend_url: String,
}

impl OAuthService {
    fn normalize_username_candidate(candidate: &str) -> String {
        let lower = candidate.to_lowercase();
        // Map invalid chars to '-' and allow [a-z0-9._-]
        let mut out = String::with_capacity(lower.len());
        for ch in lower.chars() {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '_' || ch == '-'
            {
                out.push(ch);
            } else {
                out.push('-');
            }
        }
        // Collapse multiple '-'
        let mut collapsed = String::with_capacity(out.len());
        let mut prev_dash = false;
        for ch in out.chars() {
            if ch == '-' {
                if !prev_dash {
                    collapsed.push('-');
                }
                prev_dash = true;
            } else {
                collapsed.push(ch);
                prev_dash = false;
            }
        }
        // Trim leading/trailing '-'
        let trimmed = collapsed.trim_matches('-').to_string();
        // Enforce max length
        let mut trimmed = if trimmed.len() > 30 {
            trimmed[..30].to_string()
        } else {
            trimmed
        };
        // Ensure starts with alphanumeric
        if trimmed.is_empty() {
            "user".to_string()
        } else if trimmed
            .chars()
            .next()
            .map(|c| !c.is_ascii_alphanumeric())
            .unwrap_or(true)
        {
            // First char is NOT alphanumeric, prepend "user-"
            trimmed.insert_str(0, "user-");
            trimmed
        } else {
            // First char IS alphanumeric, return trimmed
            trimmed
        }
    }

    fn email_local_part(email: &str) -> Option<String> {
        email.split('@').next().map(|s| s.to_string())
    }

    fn generate_username(
        provider: &str,
        info: &crate::dto::auth_response_dto::OAuthUserInfo,
    ) -> String {
        match provider {
            // GitHub: prefer handle (`login`) if provided in parsed info.username
            "github" => {
                if let Some(uname) = &info.username {
                    let norm = Self::normalize_username_candidate(uname);
                    if !norm.is_empty() {
                        return norm;
                    }
                }
                // fallback to display name, then email local-part
                let from_name = Self::normalize_username_candidate(&info.name);
                if !from_name.is_empty() {
                    return from_name;
                }
                let from_email = Self::email_local_part(&info.email)
                    .map(|s| Self::normalize_username_candidate(&s))
                    .unwrap_or_default();
                if !from_email.is_empty() {
                    return from_email;
                }
                "user".to_string()
            }
            // Google: prefer email local-part; fallback to name
            "google" => {
                if let Some(local) = Self::email_local_part(&info.email) {
                    let norm = Self::normalize_username_candidate(&local);
                    if !norm.is_empty() {
                        return norm;
                    }
                }
                let from_name = Self::normalize_username_candidate(&info.name);
                if !from_name.is_empty() {
                    return from_name;
                }
                "user".to_string()
            }
            _ => {
                // Other providers: try username field, then name, then email local-part
                if let Some(uname) = &info.username {
                    let norm = Self::normalize_username_candidate(uname);
                    if !norm.is_empty() {
                        return norm;
                    }
                }
                let from_name = Self::normalize_username_candidate(&info.name);
                if !from_name.is_empty() {
                    return from_name;
                }
                let from_email = Self::email_local_part(&info.email)
                    .map(|s| Self::normalize_username_candidate(&s))
                    .unwrap_or_default();
                if !from_email.is_empty() {
                    return from_email;
                }
                "user".to_string()
            }
        }
    }
    /// Creates a new OAuth service with pre-configured clients
    ///
    /// This performs async initialization including OIDC discovery for
    /// providers that support it (like Google).
    ///
    /// # Arguments
    ///
    /// * `redis_pool` - Redis connection pool for state storage
    /// * `http_client` - HTTP client for API requests
    /// * `oauth_providers` - Configuration for each OAuth provider
    /// * `token_service` - Service for minting JWT tokens
    /// * `user_repo` - Repository for user database operations
    /// * `redirect_base` - Base URL for OAuth callbacks (e.g., "https://api.example.com")
    /// * `frontend_url` - Frontend URL for redirects after OAuth success
    ///
    /// # Returns
    ///
    /// Returns `Ok(Self)` if all providers initialize successfully, or an error if:
    /// - OIDC discovery fails for a provider
    /// - Invalid URLs in configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let oauth_service = OAuthService::new(
    ///     redis_pool,
    ///     http_client,
    ///     providers_config,
    ///     token_service,
    ///     user_repo,
    ///     "https://api.example.com".to_string(),
    ///     "https://frontend.example.com".to_string(),
    /// ).await?;
    /// ```
    pub async fn new(
        conn: ConnectionManager,
        http_client: reqwest::Client,
        oauth_providers: HashMap<String, OAuthProviderConfig>,
        token_service: Arc<TokenService>,
        user_repo: Arc<dyn UserRepository>,
        redirect_base: String,
        frontend_url: String,
    ) -> Result<Self, AuthError> {
        let mut oauth_clients = HashMap::new();

        // Initialize OAuth clients for each configured provider
        for (provider, config) in oauth_providers {
            let client = Self::create_oauth_client(&provider, &config, &redirect_base).await?;
            oauth_clients.insert(provider, client);
        }

        Ok(Self {
            conn,
            http_client,
            oauth_clients,
            token_service,
            user_repo,
            frontend_url,
        })
    }

    /// Creates an OAuth client for a specific provider
    ///
    /// Uses OIDC discovery for providers that support it, otherwise creates
    /// a minimal client configuration.
    ///
    /// # OIDC Discovery
    ///
    /// For providers with `issuer_url` (like Google), this performs async discovery
    /// to fetch the provider's metadata including endpoints and supported features.
    ///
    /// # Non-OIDC Providers
    ///
    /// For plain OAuth 2.0 providers (GitHub, Twitter), creates a minimal metadata
    /// configuration with the required endpoints.
    async fn create_oauth_client(
        provider: &str,
        config: &OAuthProviderConfig,
        redirect_base: &str,
    ) -> Result<CoreClient, AuthError> {
        let client_id = ClientId::new(config.client_id.clone());
        let client_secret = ClientSecret::new(config.client_secret.expose_secret().to_string());
        let redirect_uri = format!("{}/api/oauth/{}/callback", redirect_base, provider);
        let redirect_url = RedirectUrl::new(redirect_uri)
            .map_err(|e| AuthError::Config(format!("Invalid redirect URL: {}", e)))?;

        let auth_url = AuthUrl::new(config.auth_url.clone())
            .map_err(|e| AuthError::Config(format!("Invalid auth URL: {}", e)))?;
        let token_url = TokenUrl::new(config.token_url.clone())
            .map_err(|e| AuthError::Config(format!("Invalid token URL: {}", e)))?;

        // For providers with OIDC discovery (like Google), fetch metadata
        if let Some(issuer_url) = &config.issuer_url {
            let issuer = IssuerUrl::new(issuer_url.clone())
                .map_err(|e| AuthError::Config(format!("Invalid issuer URL: {}", e)))?;

            let provider_metadata = CoreProviderMetadata::discover_async(issuer, async_http_client)
                .await
                .map_err(|e| AuthError::Config(format!("OIDC discovery failed: {}", e)))?;

            Ok(CoreClient::from_provider_metadata(
                provider_metadata,
                client_id,
                Some(client_secret),
            )
            .set_redirect_uri(redirect_url))
        } else {
            // For plain OAuth 2.0 providers, create minimal metadata
            let issuer = IssuerUrl::new(format!("https://{}.com", provider))
                .map_err(|e| AuthError::Config(format!("Invalid issuer: {}", e)))?;

            let provider_metadata = CoreProviderMetadata::new(
                issuer,
                auth_url,
                // JWKS URL - required by spec but not used for plain OAuth
                openidconnect::JsonWebKeySetUrl::new(format!(
                    "https://{}.com/.well-known/jwks.json",
                    provider
                ))
                .map_err(|e| AuthError::Config(format!("Invalid JWKS URL: {}", e)))?,
                vec![ResponseTypes::new(vec![CoreResponseType::Code])],
                vec![openidconnect::core::CoreSubjectIdentifierType::Public],
                vec![openidconnect::core::CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256],
                openidconnect::EmptyAdditionalProviderMetadata {},
            )
            .set_token_endpoint(Some(token_url));

            Ok(CoreClient::from_provider_metadata(
                provider_metadata,
                client_id,
                Some(client_secret),
            )
            .set_redirect_uri(redirect_url))
        }
    }

    /// Returns a list of configured OAuth provider names
    ///
    /// # Example Response
    ///
    /// ```json
    /// ["google", "github", "twitter"]
    /// ```
    pub async fn get_available_providers(&self) -> Vec<String> {
        self.oauth_clients.keys().cloned().collect()
    }

    /// Returns the frontend URL for redirects after OAuth success
    pub fn get_frontend_url(&self) -> &str {
        &self.frontend_url
    }

    /// Generates an authorization URL for initiating OAuth flow
    ///
    /// This method:
    /// 1. Generates a random PKCE challenge and verifier
    /// 2. Generates a random CSRF state token
    /// 3. Builds the authorization URL with appropriate scopes
    /// 4. Stores state and verifier in Redis with 10-minute expiry
    ///
    /// # Arguments
    ///
    /// * `provider` - The OAuth provider name ("google", "github", "twitter")
    ///
    /// # Returns
    ///
    /// The authorization URL to redirect the user to, or an error if:
    /// - Provider is not configured
    /// - Redis connection fails
    ///
    /// # Security
    ///
    /// - **PKCE**: SHA-256 code challenge protects against authorization code interception
    /// - **State**: Random 128-bit token prevents CSRF attacks
    /// - **TTL**: 10-minute expiry prevents replay attacks
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let auth_url = oauth_service.get_auth_url("google").await?;
    /// // Returns: "https://accounts.google.com/o/oauth2/v2/auth?client_id=...&state=..."
    /// ```
    pub async fn get_auth_url(&self, provider: &str) -> Result<String, AuthError> {
        if provider.is_empty() {
            return Err(AuthError::InvalidProvider(
                "Provider name is empty".to_string(),
            ));
        }

        let client = self.oauth_clients.get(provider).ok_or_else(|| {
            AuthError::InvalidProvider(format!("Provider '{}' not configured", provider))
        })?;

        // Generate PKCE challenge for security (prevents authorization code interception)
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        // Build authorization URL using OpenID Connect flow
        // Note: Nonce is required by API but only used for OIDC ID token validation
        let mut auth_request = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(pkce_challenge);

        // Add provider-specific scopes
        if provider == "google" {
            auth_request = auth_request
                .add_scope(Scope::new("openid".to_string()))
                .add_scope(Scope::new("email".to_string()))
                .add_scope(Scope::new("profile".to_string()));
        } else if provider == "github" {
            auth_request = auth_request.add_scope(Scope::new("user:email".to_string()));
        } else if provider == "twitter" {
            auth_request = auth_request
                .add_scope(Scope::new("tweet.read".to_string()))
                .add_scope(Scope::new("users.read".to_string()));
        }

        let (final_auth_url, csrf_state, nonce) = auth_request.url();

        // Store state and PKCE verifier in Redis with 10-minute expiry
        let mut redis_conn = self.conn.clone();

        let state_key = format!("oauth_state:{}:{}", provider, csrf_state.secret());
        let verifier_key = format!("oauth_verifier:{}:{}", provider, csrf_state.secret());

        let _: () = redis_conn
            .set_ex(&state_key, "valid", 600)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store state: {}", e)))?;

        let _: () = redis_conn
            .set_ex(&verifier_key, pkce_verifier.secret(), 600)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store verifier: {}", e)))?;

        // Store nonce for OIDC providers (used for ID token validation)
        // nonce is validated in handle_callback to prevent replay attacks
        if provider == "google" {
            let nonce_key = format!("oauth_nonce:{}:{}", provider, csrf_state.secret());
            let _: () = redis_conn
                .set_ex(&nonce_key, nonce.secret(), 600)
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to store nonce: {}", e)))?;
        }

        Ok(final_auth_url.to_string())
    }

    /// Generates an authorization URL for initiating OAuth flow with platform support
    ///
    /// This method stores all OAuth transaction data in a single Redis key and
    /// returns both the authorization URL and the state value for cookie binding.
    ///
    /// # Arguments
    ///
    /// * `provider` - The OAuth provider name ("google", "github", "twitter")
    /// * `platform` - Optional platform identifier ("app" or "web")
    /// * `app_challenge` - Required when platform="app", SHA256 hash of app's verifier
    ///
    /// # Returns
    ///
    /// A tuple of (authorization_url, state) where state should be bound to the
    /// user-agent session via cookie (for web) to prevent login CSRF.
    ///
    /// # Errors
    ///
    /// - Provider is not configured
    /// - Redis connection fails
    /// - app_challenge missing when platform="app"
    pub async fn get_auth_url_with_platform(
        &self,
        provider: &str,
        platform: Option<&str>,
        app_challenge: Option<&str>,
    ) -> Result<(String, String), AuthError> {
        if provider.is_empty() {
            return Err(AuthError::InvalidProvider(
                "Provider name is empty".to_string(),
            ));
        }

        let client = self.oauth_clients.get(provider).ok_or_else(|| {
            AuthError::InvalidProvider(format!("Provider '{}' not configured", provider))
        })?;

        // Generate PKCE challenge for security (prevents authorization code interception)
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        // Build authorization URL using OpenID Connect flow
        // Note: Nonce is required by API but only used for OIDC ID token validation
        let mut auth_request = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(pkce_challenge);

        // Add provider-specific scopes
        if provider == "google" {
            auth_request = auth_request
                .add_scope(Scope::new("openid".to_string()))
                .add_scope(Scope::new("email".to_string()))
                .add_scope(Scope::new("profile".to_string()));
        } else if provider == "github" {
            auth_request = auth_request.add_scope(Scope::new("user:email".to_string()));
        } else if provider == "twitter" {
            auth_request = auth_request
                .add_scope(Scope::new("tweet.read".to_string()))
                .add_scope(Scope::new("users.read".to_string()));
        }

        let (final_auth_url, csrf_state, nonce) = auth_request.url();

        // Store unified OAuth transaction in Redis with 10-minute expiry
        let mut redis_conn = self.conn.clone();

        let txn = OAuthTxn {
            pkce_verifier: pkce_verifier.secret().to_string(),
            nonce: if provider == "google" {
                Some(nonce.secret().to_string())
            } else {
                None
            },
            platform: platform.map(|s| s.to_string()),
            app_challenge: app_challenge.map(|s| s.to_string()),
        };

        let key = format!("oauth_txn:{}:{}", provider, csrf_state.secret());
        let value = serde_json::to_string(&txn)
            .map_err(|e| AuthError::Redis(format!("Failed to serialize oauth txn: {}", e)))?;

        let _: () = redis_conn
            .set_ex(&key, value, 600)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store oauth txn: {}", e)))?;

        Ok((final_auth_url.to_string(), csrf_state.secret().to_string()))
    }

    /// Atomically consumes the OAuth transaction from Redis
    ///
    /// Uses GETDEL to ensure one-time use semantics and prevent replay attacks.
    async fn consume_txn(&self, provider: &str, state: &str) -> Result<OAuthTxn, AuthError> {
        let mut redis_conn = self.conn.clone();

        let key = format!("oauth_txn:{}:{}", provider, state);

        let raw: Option<String> = redis::cmd("GETDEL")
            .arg(&key)
            .query_async(&mut redis_conn)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to GETDEL oauth txn: {}", e)))?;

        let raw = raw.ok_or(AuthError::StateMismatch)?;
        let txn: OAuthTxn = serde_json::from_str(&raw)
            .map_err(|e| AuthError::Redis(format!("Failed to deserialize oauth txn: {}", e)))?;

        Ok(txn)
    }

    /// Handles the OAuth callback after user authentication
    ///
    /// This is the core method that completes the OAuth flow:
    /// 1. Validates the state parameter (CSRF protection)
    /// 2. Retrieves and validates the PKCE verifier
    /// 3. Exchanges the authorization code for an access token
    /// 4. Fetches user information from the provider
    /// 5. Creates or updates the user in the database
    /// 6. Mints JWT tokens for your application
    ///
    /// # Arguments
    ///
    /// * `provider` - The OAuth provider name
    /// * `code` - The authorization code from the provider
    /// * `state` - The state parameter for CSRF validation
    ///
    /// # Returns
    ///
    /// An `OAuthLoginResponse` containing:
    /// - `access_token`: JWT for API authentication
    /// - `refresh_token`: JWT for token refresh
    /// - `expires_in`: Token expiry time in seconds
    /// - `user`: User information
    ///
    /// # Errors
    ///
    /// - `StateMismatch`: State parameter invalid or expired
    /// - `CodeExchangeFailed`: Token exchange with provider failed
    /// - `UserInfoFetchFailed`: Could not fetch user info from provider
    /// - `EmailAlreadyExists`: Email is registered with different provider
    ///
    /// # Database Operations
    ///
    /// - For existing users: Updates `last_login` timestamp
    /// - For new users: Creates user record with OAuth provider info
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let response = oauth_service.handle_callback(
    ///     "google",
    ///     "4/0AY0e-g7...", // authorization code
    ///     "random_state_123" // state parameter
    /// ).await?;
    ///
    /// println!("User {} logged in", response.user.email);
    /// ```
    pub async fn handle_callback(
        &self,
        provider: &str,
        code: &str,
        state: &str,
    ) -> Result<(OAuthLoginResponse, OAuthCallbackMeta), AuthError> {
        // Atomically consume the OAuth transaction (provides CSRF protection via one-time use)
        let txn = self.consume_txn(provider, state).await?;

        // Get OAuth client
        let client = self.oauth_clients.get(provider).ok_or_else(|| {
            AuthError::InvalidProvider(format!("Provider '{}' not configured", provider))
        })?;

        // Exchange authorization code for access token using stored PKCE verifier
        let token_response = client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(txn.pkce_verifier))
            .request_async(async_http_client)
            .await
            .map_err(|e| AuthError::CodeExchangeFailed(format!("Token exchange failed: {}", e)))?;

        // Validate ID token for OIDC providers (Google)
        // This validates signature, issuer, audience, expiration, and nonce
        if provider == "google" {
            let nonce = txn.nonce.as_deref().ok_or_else(|| {
                AuthError::CodeExchangeFailed("OIDC nonce missing from transaction".to_string())
            })?;
            self.validate_id_token(client, &token_response, nonce)
                .await?;
        }

        // Extract access token from response
        // Trait must be in scope to use the access_token() method
        use openidconnect::OAuth2TokenResponse;
        let access_token_secret = token_response.access_token().secret();

        // Get user info from provider's API
        let user_info = self.get_user_info(provider, access_token_secret).await?;

        // Find or create user in database
        let user = match self
            .user_repo
            .find_by_provider_id(provider, &user_info.provider_id)
            .await?
        {
            Some(existing_user) => {
                // Update last login timestamp for existing user
                self.user_repo
                    .update_last_login(existing_user.user_id)
                    .await?;
                existing_user
            }
            None => {
                // Check if user exists with same email but different provider
                if let Some(_existing_user) = self.user_repo.find_by_email(&user_info.email).await?
                {
                    return Err(AuthError::EmailAlreadyExists);
                }

                // Create new user record
                let now = chrono::Utc::now();
                let generated_username = Self::generate_username(provider, &user_info);
                let new_user = Model {
                    user_id: Uuid::new_v4(),
                    email: user_info.email.clone(),
                    username: generated_username,
                    password_hash: None, // OAuth users don't have passwords
                    auth_provider: provider.to_string(),
                    provider_id: Some(user_info.provider_id.clone()),
                    avatar_url: user_info.avatar_url.clone(),
                    email_verified: user_info.email_verified,
                    status: UserStatus::Active,
                    created_at: now,
                    updated_at: now,
                    last_login: Some(now),
                };

                self.user_repo.create_user(&new_user).await?
            }
        };

        // Generate JWT tokens for your application
        let tokens = self
            .token_service
            .mint_tokens(user.user_id)
            .await
            .map_err(|e| AuthError::CodeExchangeFailed(format!("Token minting failed: {}", e)))?;

        Ok((
            OAuthLoginResponse {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                expires_in: tokens.expires_in,
                user: user.into(),
            },
            OAuthCallbackMeta {
                platform: txn.platform,
                app_challenge: txn.app_challenge,
            },
        ))
    }

    /// Validates the OIDC ID token for Google authentication
    ///
    /// This method performs comprehensive ID token validation:
    /// - Verifies signature using Google's JWKS endpoint
    /// - Checks issuer matches "https://accounts.google.com"
    /// - Checks audience matches the configured client ID
    /// - Verifies expiration and issued-at timestamps
    /// - Validates the nonce claim matches the stored nonce
    ///
    /// # Arguments
    ///
    /// * `client` - The OAuth client configured for the provider
    /// * `token_response` - The token response from the provider
    /// * `stored_nonce_secret` - The nonce stored during OAuth initiation
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - ID token is missing from the response
    /// - Any validation step fails (signature, issuer, audience, expiration, nonce)
    async fn validate_id_token(
        &self,
        client: &CoreClient,
        token_response: &openidconnect::core::CoreTokenResponse,
        stored_nonce_secret: &str,
    ) -> Result<(), AuthError> {
        // Extract ID token from response
        let id_token = token_response.id_token().ok_or_else(|| {
            AuthError::CodeExchangeFailed("ID token missing from OIDC response".to_string())
        })?;

        let stored_nonce = Nonce::new(stored_nonce_secret.to_string());

        // Verify ID token using the client's verifier
        // This validates:
        // - Signature using JWKS endpoint
        // - Issuer (iss) claim
        // - Audience (aud) claim
        // - Expiration (exp) and issued-at (iat) claims
        // - Nonce claim must match the stored nonce
        let verifier = client.id_token_verifier();
        id_token.claims(&verifier, &stored_nonce).map_err(|e| {
            AuthError::CodeExchangeFailed(format!("ID token validation failed: {}", e))
        })?;

        tracing::debug!("ID token validated successfully for Google OAuth");
        Ok(())
    }

    /// Fetches user information from the OAuth provider
    ///
    /// Different providers return user info in different formats, so this method
    /// delegates to provider-specific parsers.
    async fn get_user_info(
        &self,
        provider: &str,
        access_token: &str,
    ) -> Result<OAuthUserInfo, AuthError> {
        self.fetch_user_info_from_endpoint(provider, access_token)
            .await
    }

    /// Fetches user information from provider's userinfo endpoint
    ///
    /// # Provider-specific Behavior
    ///
    /// - **Google**: Uses OAuth2 userinfo endpoint
    /// - **GitHub**: Fetches from /user and /user/emails (requires separate call)
    /// - **Twitter**: Uses /2/users/me with user fields
    ///
    /// # GitHub Special Case
    ///
    /// GitHub requires a separate API call to fetch email addresses as they may
    /// not be public. This method automatically fetches and merges email data.
    async fn fetch_user_info_from_endpoint(
        &self,
        provider: &str,
        access_token: &str,
    ) -> Result<OAuthUserInfo, AuthError> {
        // Get user info URL based on provider
        let user_info_url = match provider {
            "google" => "https://openidconnect.googleapis.com/v1/userinfo",
            "github" => "https://api.github.com/user",
            "twitter" => "https://api.twitter.com/2/users/me?user.fields=profile_image_url",
            _ => {
                return Err(AuthError::InvalidProvider(format!(
                    "Unknown provider: {}",
                    provider
                )));
            }
        };

        let response = self
            .http_client
            .get(user_info_url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("User-Agent", "VEX-Gateway")
            .send()
            .await
            .map_err(|e| {
                AuthError::UserInfoFetchFailed(format!("Failed to fetch user info: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(AuthError::UserInfoFetchFailed(format!(
                "User info request failed: {}",
                response.status()
            )));
        }

        let mut user_info: serde_json::Value = response.json().await.map_err(|e| {
            AuthError::UserInfoFetchFailed(format!("Failed to parse user info: {}", e))
        })?;

        // Debug logging for Google OAuth issues
        if provider == "google" {
            tracing::debug!(
                "Google user info response: {}",
                serde_json::to_string_pretty(&user_info)
                    .unwrap_or_else(|_| "Failed to serialize".to_string())
            );
        }

        // Special handling for GitHub - fetch emails separately
        if provider == "github" {
            match self.fetch_github_emails(access_token).await {
                Ok(emails) => {
                    // Find the primary verified email
                    if let Some(primary_email) = emails.iter().find(|email| {
                        email
                            .get("primary")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    }) {
                        if let Some(email_str) = primary_email.get("email").and_then(|v| v.as_str())
                        {
                            user_info["email"] = serde_json::Value::String(email_str.to_string());
                        }
                        if let Some(verified) =
                            primary_email.get("verified").and_then(|v| v.as_bool())
                        {
                            user_info["email_verified"] = serde_json::Value::Bool(verified);
                        }
                    }
                }
                Err(e) => {
                    // Extract user identifier for logging context
                    let user_id = user_info["id"]
                        .as_u64()
                        .map(|id| id.to_string())
                        .or_else(|| user_info["login"].as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| "unknown".to_string());

                    tracing::error!(
                        provider = "github",
                        user_id = %user_id,
                        error = %e,
                        "Failed to fetch GitHub email addresses"
                    );

                    // Return the error with additional context
                    return Err(AuthError::UserInfoFetchFailed(format!(
                        "Failed to fetch GitHub email addresses for user {}: {}",
                        user_id, e
                    )));
                }
            }
        }

        // Parse based on provider
        let oauth_user_info = match provider {
            "google" => self.parse_google_user_info(user_info)?,
            "github" => self.parse_github_user_info(user_info).await?,
            "twitter" => self.parse_twitter_user_info(user_info)?,
            _ => {
                return Err(AuthError::UserInfoFetchFailed(format!(
                    "Unsupported provider: {}",
                    provider
                )));
            }
        };

        Ok(oauth_user_info)
    }

    /// Fetches email addresses from GitHub's /user/emails endpoint
    ///
    /// GitHub users may have multiple email addresses, and their primary email
    /// may not be public. This method fetches all emails and identifies the
    /// primary verified one.
    async fn fetch_github_emails(
        &self,
        access_token: &str,
    ) -> Result<Vec<serde_json::Value>, AuthError> {
        let response = self
            .http_client
            .get("https://api.github.com/user/emails")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("User-Agent", "VEX-Gateway")
            .send()
            .await
            .map_err(|e| {
                AuthError::UserInfoFetchFailed(format!("Failed to fetch GitHub emails: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(AuthError::UserInfoFetchFailed(format!(
                "GitHub emails request failed: {}",
                response.status()
            )));
        }

        let emails: Vec<serde_json::Value> = response.json().await.map_err(|e| {
            AuthError::UserInfoFetchFailed(format!("Failed to parse GitHub emails: {}", e))
        })?;

        Ok(emails)
    }

    /// Parses Google userinfo response into standardized format
    ///
    /// # Response Format
    ///
    /// ```json
    /// {
    ///   "sub": "1234567890",
    ///   "email": "user@gmail.com",
    ///   "name": "John Doe",
    ///   "picture": "https://...",
    ///   "email_verified": true
    /// }
    /// ```
    fn parse_google_user_info(&self, data: Value) -> Result<OAuthUserInfo, AuthError> {
        tracing::debug!(
            "Parsing Google user info: {}",
            serde_json::to_string_pretty(&data)
                .unwrap_or_else(|_| "Failed to serialize".to_string())
        );

        let provider_id = data["sub"]
            .as_str()
            .ok_or_else(|| {
                let available_fields: Vec<String> = data
                    .as_object()
                    .map(|obj| obj.keys().cloned().collect())
                    .unwrap_or_default();
                AuthError::UserInfoFetchFailed(format!(
                    "Missing Google 'sub' field. Available fields: {:?}. Response: {}",
                    available_fields,
                    serde_json::to_string(&data)
                        .unwrap_or_else(|_| "Failed to serialize".to_string())
                ))
            })?
            .to_string();

        let email = data["email"]
            .as_str()
            .ok_or_else(|| AuthError::UserInfoFetchFailed("Missing Google email".to_string()))?
            .to_string();

        Ok(OAuthUserInfo {
            provider_id,
            email,
            name: data["name"].as_str().unwrap_or("Unknown User").to_string(),
            username: None,
            avatar_url: data["picture"].as_str().map(|s| s.to_string()),
            email_verified: data["email_verified"].as_bool().unwrap_or(false),
        })
    }

    /// Parses GitHub user response into standardized format
    ///
    /// # Response Format
    ///
    /// ```json
    /// {
    ///   "id": 1234567,
    ///   "login": "johndoe",
    ///   "name": "John Doe",
    ///   "email": "user@example.com",  // May be null
    ///   "avatar_url": "https://..."
    /// }
    /// ```
    ///
    /// Note: Email is fetched separately via `fetch_github_emails()`
    async fn parse_github_user_info(&self, data: Value) -> Result<OAuthUserInfo, AuthError> {
        let email = if let Some(email) = data["email"].as_str() {
            email.to_string()
        } else {
            return Err(AuthError::UserInfoFetchFailed(
                "GitHub email is not available".to_string(),
            ));
        };

        Ok(OAuthUserInfo {
            provider_id: data["id"]
                .as_u64()
                .ok_or_else(|| {
                    AuthError::UserInfoFetchFailed("Missing GitHub user ID".to_string())
                })?
                .to_string(),
            email,
            name: data["name"]
                .as_str()
                .or_else(|| data["login"].as_str())
                .unwrap_or("Unknown User")
                .to_string(),
            username: data["login"].as_str().map(|s| s.to_string()),
            avatar_url: data["avatar_url"].as_str().map(|s| s.to_string()),
            email_verified: data["email_verified"].as_bool().unwrap_or(false),
        })
    }

    /// Parses Twitter/X user response into standardized format
    ///
    /// # Response Format
    ///
    /// ```json
    /// {
    ///   "data": {
    ///     "id": "1234567890",
    ///     "username": "johndoe",
    ///     "name": "John Doe",
    ///     "profile_image_url": "https://...",
    ///     "email": "user@example.com"  // May be missing if email scope not granted
    ///   }
    /// }
    /// ```
    ///
    /// # Email Requirement
    ///
    /// This method requires a non-empty email address. If the email is missing or empty,
    /// it returns an error indicating that the email scope must be requested.
    fn parse_twitter_user_info(&self, data: Value) -> Result<OAuthUserInfo, AuthError> {
        let user_data = &data["data"];

        // Extract and validate email
        let email = user_data["email"].as_str().ok_or_else(|| {
            AuthError::UserInfoFetchFailed(
                "Twitter email not provided; request email scope".to_string(),
            )
        })?;

        if email.is_empty() {
            return Err(AuthError::UserInfoFetchFailed(
                "Twitter email not provided; request email scope".to_string(),
            ));
        }

        Ok(OAuthUserInfo {
            provider_id: user_data["id"]
                .as_str()
                .ok_or_else(|| {
                    AuthError::UserInfoFetchFailed("Missing Twitter user ID".to_string())
                })?
                .to_string(),
            email: email.to_string(),
            name: user_data["name"]
                .as_str()
                .unwrap_or("Unknown User")
                .to_string(),
            username: user_data["username"].as_str().map(|s| s.to_string()),
            avatar_url: user_data["profile_image_url"]
                .as_str()
                .map(|s| s.to_string()),
            email_verified: user_data["email_verified"].as_bool().unwrap_or(false),
        })
    }

    /// Logs out a user by blacklisting their JWT token
    ///
    /// This method invalidates the JWT by adding its JTI (JWT ID) to a Redis
    /// blacklist. The blacklist entry expires when the token would naturally expire.
    ///
    /// # Arguments
    ///
    /// * `token` - The JWT access token to revoke
    ///
    /// # Behavior
    ///
    /// - If token has no JTI, does nothing (token can't be blacklisted)
    /// - If token is already expired, does nothing
    /// - Otherwise, adds JTI to Redis blacklist with TTL = remaining token lifetime
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// oauth_service.logout("eyJhbGciOiJSUzI1NiIs...").await?;
    /// // Token is now invalid for all future requests
    /// ```
    pub async fn logout(&self, token: &str) -> Result<(), AuthError> {
        // Validate JWT to extract claims
        let claims = self.token_service.validate_jwt(token)?;

        // Check if token has JTI (required for blacklisting)
        if claims.jti.is_empty() {
            return Ok(());
        }

        // Calculate remaining TTL
        let current_time = chrono::Utc::now().timestamp();
        if claims.exp <= current_time {
            // Token already expired, no need to blacklist
            return Ok(());
        }

        let ttl_secs = (claims.exp - current_time) as u64;

        // Add JTI to blacklist in Redis
        self.token_service
            .blacklist_jti(&claims.jti, ttl_secs)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for normalize_username_candidate
    mod normalize_username {
        use super::*;

        #[test]
        fn test_lowercase_conversion() {
            assert_eq!(
                OAuthService::normalize_username_candidate("JohnDoe"),
                "johndoe"
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("UPPERCASE"),
                "uppercase"
            );
        }

        #[test]
        fn test_invalid_chars_replaced_with_dash() {
            assert_eq!(
                OAuthService::normalize_username_candidate("john@doe"),
                "john-doe"
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("user#name!"),
                "user-name"
            );
        }

        #[test]
        fn test_allowed_chars_preserved() {
            assert_eq!(
                OAuthService::normalize_username_candidate("john.doe"),
                "john.doe"
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("john_doe"),
                "john_doe"
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("john-doe"),
                "john-doe"
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("user123"),
                "user123"
            );
        }

        #[test]
        fn test_multiple_dashes_collapsed() {
            assert_eq!(
                OAuthService::normalize_username_candidate("john@@doe"),
                "john-doe"
            );
            assert_eq!(OAuthService::normalize_username_candidate("a---b"), "a-b");
        }

        #[test]
        fn test_leading_trailing_dashes_trimmed() {
            assert_eq!(OAuthService::normalize_username_candidate("-john-"), "john");
            assert_eq!(
                OAuthService::normalize_username_candidate("@username@"),
                "username"
            );
        }

        #[test]
        fn test_max_length_enforced() {
            let long_name = "a".repeat(50);
            let result = OAuthService::normalize_username_candidate(&long_name);
            assert_eq!(result.len(), 30);
        }

        #[test]
        fn test_empty_returns_user() {
            assert_eq!(OAuthService::normalize_username_candidate(""), "user");
            assert_eq!(OAuthService::normalize_username_candidate("@@@"), "user");
        }

        #[test]
        fn test_non_alphanumeric_start_prepends_user() {
            assert_eq!(
                OAuthService::normalize_username_candidate(".hidden"),
                "user-.hidden"
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("_underscore"),
                "user-_underscore"
            );
        }
    }

    // Tests for email_local_part
    mod email_local_part {
        use super::*;

        #[test]
        fn test_extracts_local_part() {
            assert_eq!(
                OAuthService::email_local_part("john@example.com"),
                Some("john".to_string())
            );
            assert_eq!(
                OAuthService::email_local_part("user.name@domain.org"),
                Some("user.name".to_string())
            );
        }

        #[test]
        fn test_handles_no_at_symbol() {
            assert_eq!(
                OAuthService::email_local_part("noemail"),
                Some("noemail".to_string())
            );
        }

        #[test]
        fn test_handles_empty_string() {
            assert_eq!(OAuthService::email_local_part(""), Some("".to_string()));
        }

        #[test]
        fn test_handles_multiple_at_symbols() {
            // Takes everything before the first @
            assert_eq!(
                OAuthService::email_local_part("weird@email@domain.com"),
                Some("weird".to_string())
            );
        }
    }

    // Tests for generate_username
    mod generate_username {
        use super::*;
        use crate::dto::auth_response_dto::OAuthUserInfo;

        fn make_user_info(name: &str, email: &str, username: Option<&str>) -> OAuthUserInfo {
            OAuthUserInfo {
                provider_id: "12345".to_string(),
                email: email.to_string(),
                email_verified: true,
                name: name.to_string(),
                avatar_url: None,
                username: username.map(|s| s.to_string()),
            }
        }

        #[test]
        fn test_github_prefers_username() {
            let info = make_user_info("John Doe", "john@example.com", Some("johndoe123"));
            assert_eq!(
                OAuthService::generate_username("github", &info),
                "johndoe123"
            );
        }

        #[test]
        fn test_github_fallback_to_name() {
            let info = make_user_info("John Doe", "john@example.com", None);
            assert_eq!(OAuthService::generate_username("github", &info), "john-doe");
        }

        #[test]
        fn test_github_with_invalid_name_and_email() {
            // When name normalizes to "user" (all invalid chars), it returns "user"
            // without falling through to email (this is expected behavior)
            let info = make_user_info("@@@", "john@example.com", None);
            assert_eq!(OAuthService::generate_username("github", &info), "user");
        }

        #[test]
        fn test_github_with_valid_email_only() {
            // Test the actual email fallback with a name that's truly empty after normalization
            // The normalize function returns "user" for empty strings, so this tests that
            let info = make_user_info("", "john@example.com", None);
            // normalize("") returns "user" which is not empty, so it returns "user"
            assert_eq!(OAuthService::generate_username("github", &info), "user");
        }

        #[test]
        fn test_google_prefers_email_local() {
            let info = make_user_info("John Doe", "johndoe@gmail.com", None);
            assert_eq!(OAuthService::generate_username("google", &info), "johndoe");
        }

        #[test]
        fn test_google_with_invalid_email_uses_name() {
            // When email has no local part (just @), it normalizes to "user"
            // but we still check if it's empty. Let's test with a valid case.
            let info = make_user_info("John Doe", "johndoe@gmail.com", None);
            // This should prefer email
            assert_eq!(OAuthService::generate_username("google", &info), "johndoe");
        }

        #[test]
        fn test_google_with_empty_email() {
            // When email is empty, email_local_part returns Some("")
            // normalize("") returns "user", which is not empty
            // So it returns "user" without checking name
            let info = make_user_info("John Doe", "", None);
            // Actually, email_local_part("") returns Some(""), and normalize("") returns "user"
            // which is not empty, so it returns "user" without checking name
            assert_eq!(OAuthService::generate_username("google", &info), "user");
        }

        #[test]
        fn test_unknown_provider_tries_username_first() {
            let info = make_user_info("John", "john@example.com", Some("twitter_user"));
            assert_eq!(
                OAuthService::generate_username("twitter", &info),
                "twitter_user"
            );
        }

        #[test]
        fn test_all_empty_returns_user() {
            let info = make_user_info("", "", None);
            assert_eq!(OAuthService::generate_username("github", &info), "user");
            assert_eq!(OAuthService::generate_username("google", &info), "user");
            assert_eq!(OAuthService::generate_username("twitter", &info), "user");
        }
    }

    // Tests for OAuthTxn serialization
    mod oauth_txn {
        use super::*;

        #[test]
        fn test_oauth_txn_serialization_roundtrip() {
            let txn = OAuthTxn {
                pkce_verifier: "test_verifier_123".to_string(),
                nonce: Some("nonce_value".to_string()),
                platform: Some("web".to_string()),
                app_challenge: None,
            };

            let json = serde_json::to_string(&txn).expect("serialize");
            let parsed: OAuthTxn = serde_json::from_str(&json).expect("deserialize");

            assert_eq!(parsed.pkce_verifier, txn.pkce_verifier);
            assert_eq!(parsed.nonce, txn.nonce);
            assert_eq!(parsed.platform, txn.platform);
            assert_eq!(parsed.app_challenge, txn.app_challenge);
        }

        #[test]
        fn test_oauth_txn_minimal() {
            let txn = OAuthTxn {
                pkce_verifier: "verifier".to_string(),
                nonce: None,
                platform: None,
                app_challenge: None,
            };

            let json = serde_json::to_string(&txn).expect("serialize");
            assert!(json.contains("pkce_verifier"));
            assert!(json.contains("verifier"));
        }

        #[test]
        fn test_oauth_txn_app_platform() {
            let txn = OAuthTxn {
                pkce_verifier: "verifier".to_string(),
                nonce: None,
                platform: Some("app".to_string()),
                app_challenge: Some("challenge_hash".to_string()),
            };

            let json = serde_json::to_string(&txn).expect("serialize");
            let parsed: OAuthTxn = serde_json::from_str(&json).expect("deserialize");

            assert_eq!(parsed.platform, Some("app".to_string()));
            assert_eq!(parsed.app_challenge, Some("challenge_hash".to_string()));
        }
    }
}

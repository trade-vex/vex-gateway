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

use crate::config::{OAuthConfig, OAuthProviderConfig};
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
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, ResponseTypes, Scope, TokenUrl,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreResponseType},
};
use secrecy::ExposeSecret;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::models::user::{Model, UserStatus};
use crate::repository::UserRepository;
use crate::utils::generate_random_string;

/// Longest username generated for an OAuth account, collision suffix included.
const MAX_USERNAME_LEN: usize = 30;

/// Length of the suffix that makes a taken username unique: '-' and 4 random characters.
const USERNAME_SUFFIX_LEN: usize = 5;

/// Suffixed usernames a new OAuth account tries after its own is taken, before sign-up fails.
const USERNAME_SUFFIX_ATTEMPTS: usize = 5;

/// A client built from provider metadata: the authorization endpoint is always present, while the
/// token and user info endpoints may be absent from the metadata.
type OAuthClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

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

    /// HTTP client for the OAuth/OIDC protocol requests (discovery, token exchange). It does not
    /// follow redirects, which would open the token exchange to SSRF.
    oidc_http_client: reqwest::Client,

    /// Pre-configured OAuth clients per provider (initialized at startup)
    oauth_clients: HashMap<String, OAuthClient>,

    /// JWT token service for minting application tokens
    token_service: Arc<TokenService>,

    /// User repository for database operations
    user_repo: Arc<dyn UserRepository>,

    /// Where a completed login lands: `auth.oauth.frontend_url_web`
    frontend_url_web: Option<String>,

    /// Where a completed mobile-app login lands: `auth.oauth.frontend_url_app`
    frontend_url_app: Option<String>,
}

impl OAuthService {
    /// A username built from `candidate`, or `None` when no usable character is left.
    fn normalize_username_candidate(candidate: &str) -> Option<String> {
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
        // Trim leading/trailing '-'; a candidate with no ASCII letter or digit left (such as
        // "___" or "...") is unusable
        let trimmed = collapsed.trim_matches('-');
        if !trimmed.chars().any(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        // Ensure starts with alphanumeric
        let mut username = if trimmed.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            trimmed.to_string()
        } else {
            format!("user-{trimmed}")
        };
        // Enforce max length; the username is ASCII, so any length is a char boundary. The cut
        // can land on a '-', which is trimmed like the others.
        username.truncate(MAX_USERNAME_LEN);
        username.truncate(username.trim_end_matches('-').len());
        Some(username)
    }

    fn email_local_part(email: &str) -> Option<String> {
        email.split('@').next().map(|s| s.to_string())
    }

    /// The username a new account starts from: the first usable candidate in the provider's
    /// order, and `"user"` only when none is usable.
    fn generate_username(provider: &str, info: &OAuthUserInfo) -> String {
        let handle = || {
            info.username
                .as_deref()
                .and_then(Self::normalize_username_candidate)
        };
        let name = || Self::normalize_username_candidate(&info.name);
        let email = || {
            Self::email_local_part(&info.email)
                .and_then(|local| Self::normalize_username_candidate(&local))
        };
        match provider {
            // Google: prefer email local-part; fallback to name
            "google" => email().or_else(name),
            // GitHub (`login`) and other providers: prefer the handle, then the display name,
            // then the email local-part
            _ => handle().or_else(name).or_else(email),
        }
        .unwrap_or_else(|| "user".to_string())
    }

    /// `base` with a random suffix, for when another account already holds `base`. `base` is
    /// shortened if needed to keep the result within `MAX_USERNAME_LEN`.
    fn suffixed_username(base: &str) -> String {
        // `base` is ASCII, so `keep` is a char boundary. Separators left at the cut are dropped,
        // so the result never reads "abc.-x1y2".
        let keep = base.len().min(MAX_USERNAME_LEN - USERNAME_SUFFIX_LEN);
        let stem = base[..keep].trim_end_matches(['-', '.', '_']);
        let suffix = generate_random_string(USERNAME_SUFFIX_LEN - 1).to_ascii_lowercase();
        format!("{stem}-{suffix}")
    }

    /// The account for an OAuth login: the one already linked to the provider identity, or a new
    /// one when the email is not registered yet.
    ///
    /// A new account's username must be unique (`ux_users_username`), and different people can
    /// map to the same one. When the insert reports the username taken, it is retried with a
    /// random suffix, `USERNAME_SUFFIX_ATTEMPTS` times at most, so the database constraint alone
    /// decides; a separate existence check would race with concurrent sign-ups.
    async fn find_or_create_user(
        user_repo: &dyn UserRepository,
        provider: &str,
        info: &OAuthUserInfo,
    ) -> Result<Model, AuthError> {
        if let Some(existing_user) = user_repo
            .find_by_provider_id(provider, &info.provider_id)
            .await?
        {
            // Update last login timestamp for existing user
            user_repo.update_last_login(existing_user.user_id).await?;
            return Ok(existing_user);
        }

        // Check if user exists with same email but different provider
        if user_repo.find_by_email(&info.email).await?.is_some() {
            return Err(AuthError::EmailAlreadyExists);
        }

        let now = chrono::Utc::now();
        let base_username = Self::generate_username(provider, info);
        let mut new_user = Model {
            user_id: Uuid::new_v4(),
            email: info.email.clone(),
            username: base_username.clone(),
            password_hash: None, // OAuth users don't have passwords
            auth_provider: provider.to_string(),
            provider_id: Some(info.provider_id.clone()),
            avatar_url: info.avatar_url.clone(),
            email_verified: info.email_verified,
            status: UserStatus::Active,
            created_at: now,
            updated_at: now,
            last_login: Some(now),
        };

        for _ in 0..USERNAME_SUFFIX_ATTEMPTS {
            match user_repo.create_user(&new_user).await {
                Err(AuthError::UsernameAlreadyExists) => {
                    new_user.username = Self::suffixed_username(&base_username);
                }
                result => return result,
            }
        }
        user_repo.create_user(&new_user).await
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
    /// * `oauth_config` - The `auth.oauth` section: the providers, the base URL for OAuth
    ///   callbacks (e.g., "https://api.example.com") and the frontend URLs a completed login
    ///   lands on
    /// * `token_service` - Service for minting JWT tokens
    /// * `user_repo` - Repository for user database operations
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
    ///     &config.auth.oauth,
    ///     token_service,
    ///     user_repo,
    /// ).await?;
    /// ```
    pub async fn new(
        conn: ConnectionManager,
        http_client: reqwest::Client,
        oauth_config: &OAuthConfig,
        token_service: Arc<TokenService>,
        user_repo: Arc<dyn UserRepository>,
    ) -> Result<Self, AuthError> {
        let oidc_http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AuthError::Config(format!("Failed to build OAuth HTTP client: {}", e)))?;
        let mut oauth_clients = HashMap::new();

        // Initialize OAuth clients for each configured provider
        for (provider, config) in &oauth_config.providers {
            let client = Self::create_oauth_client(
                provider,
                config,
                &oauth_config.redirect_base,
                &oidc_http_client,
            )
            .await?;
            oauth_clients.insert(provider.clone(), client);
        }

        Ok(Self {
            conn,
            http_client,
            oidc_http_client,
            oauth_clients,
            token_service,
            user_repo,
            frontend_url_web: oauth_config.frontend_url_web.clone(),
            frontend_url_app: oauth_config.frontend_url_app.clone(),
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
        http_client: &reqwest::Client,
    ) -> Result<OAuthClient, AuthError> {
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

            let provider_metadata = CoreProviderMetadata::discover_async(issuer, http_client)
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

    /// Where a completed OAuth login for `platform` lands: `auth.oauth.frontend_url_app` for the
    /// mobile app (`"app"`), `auth.oauth.frontend_url_web` otherwise. lib-config requires both
    /// whenever a provider is configured; there is no fallback URL.
    pub fn frontend_url(&self, platform: Option<&str>) -> Result<&str, AuthError> {
        landing_url(platform, &self.frontend_url_web, &self.frontend_url_app)
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
    /// - `UsernameAlreadyExists`: A new user's username and every suffixed retry were taken
    ///
    /// # Database Operations
    ///
    /// - For existing users: Updates `last_login` timestamp
    /// - For new users: Creates user record with OAuth provider info, under a unique username
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
            .map_err(|e| AuthError::Config(format!("Token endpoint not configured: {}", e)))?
            .set_pkce_verifier(PkceCodeVerifier::new(txn.pkce_verifier))
            .request_async(&self.oidc_http_client)
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
        let user = Self::find_or_create_user(self.user_repo.as_ref(), provider, &user_info).await?;

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
        client: &OAuthClient,
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

/// `app` for the mobile app (`platform == "app"`), `web` for every other platform.
fn landing_url<'a>(
    platform: Option<&str>,
    web: &'a Option<String>,
    app: &'a Option<String>,
) -> Result<&'a str, AuthError> {
    let (url, key) = match platform {
        Some("app") => (app, "frontend_url_app"),
        _ => (web, "frontend_url_web"),
    };
    url.as_deref()
        .ok_or_else(|| AuthError::Config(format!("auth.oauth.{key} is not configured")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_logins_land_on_the_app_url_and_every_other_platform_on_the_web_url() {
        let web = Some("https://app.example.com/oauth/done".to_string());
        let app = Some("vex://oauth".to_string());

        assert_eq!(landing_url(Some("app"), &web, &app).unwrap(), "vex://oauth");
        for platform in [None, Some("web"), Some("APP"), Some("")] {
            assert_eq!(
                landing_url(platform, &web, &app).unwrap(),
                "https://app.example.com/oauth/done"
            );
        }
        // No fallback from one to the other.
        assert!(matches!(
            landing_url(Some("app"), &web, &None),
            Err(AuthError::Config(_))
        ));
        assert!(matches!(
            landing_url(None, &None, &app),
            Err(AuthError::Config(_))
        ));
    }

    mod normalize_username {
        use super::*;

        #[test]
        fn test_lowercase_conversion() {
            assert_eq!(
                OAuthService::normalize_username_candidate("JohnDoe").as_deref(),
                Some("johndoe")
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("UPPERCASE").as_deref(),
                Some("uppercase")
            );
        }

        #[test]
        fn test_invalid_chars_replaced_with_dash() {
            assert_eq!(
                OAuthService::normalize_username_candidate("john@doe").as_deref(),
                Some("john-doe")
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("user#name!").as_deref(),
                Some("user-name")
            );
        }

        #[test]
        fn test_allowed_chars_preserved() {
            assert_eq!(
                OAuthService::normalize_username_candidate("john.doe").as_deref(),
                Some("john.doe")
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("john_doe").as_deref(),
                Some("john_doe")
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("john-doe").as_deref(),
                Some("john-doe")
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("user123").as_deref(),
                Some("user123")
            );
        }

        #[test]
        fn test_multiple_dashes_collapsed() {
            assert_eq!(
                OAuthService::normalize_username_candidate("john@@doe").as_deref(),
                Some("john-doe")
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("a---b").as_deref(),
                Some("a-b")
            );
        }

        #[test]
        fn test_leading_trailing_dashes_trimmed() {
            assert_eq!(
                OAuthService::normalize_username_candidate("-john-").as_deref(),
                Some("john")
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("@username@").as_deref(),
                Some("username")
            );
        }

        #[test]
        fn test_max_length_enforced() {
            let long_name = "a".repeat(50);
            let result = OAuthService::normalize_username_candidate(&long_name).unwrap();
            assert_eq!(result.len(), 30);

            // The cap includes the "user-" prefix.
            let long_name = format!("_{}", "a".repeat(50));
            let result = OAuthService::normalize_username_candidate(&long_name).unwrap();
            assert_eq!(result, format!("user-_{}", "a".repeat(24)));
        }

        #[test]
        fn test_unusable_returns_none() {
            assert_eq!(OAuthService::normalize_username_candidate(""), None);
            assert_eq!(OAuthService::normalize_username_candidate("@@@"), None);
            // A name with no ASCII letter or digit, such as one in a non-Latin script.
            assert_eq!(OAuthService::normalize_username_candidate("名前"), None);
            // Only separators.
            assert_eq!(OAuthService::normalize_username_candidate("___"), None);
            assert_eq!(OAuthService::normalize_username_candidate("..."), None);
        }

        #[test]
        fn test_cut_at_a_dash_is_trimmed() {
            let long_name = format!("{} {}", "a".repeat(29), "b".repeat(10));
            assert_eq!(
                OAuthService::normalize_username_candidate(&long_name),
                Some("a".repeat(29))
            );
        }

        #[test]
        fn test_non_alphanumeric_start_prepends_user() {
            assert_eq!(
                OAuthService::normalize_username_candidate(".hidden").as_deref(),
                Some("user-.hidden")
            );
            assert_eq!(
                OAuthService::normalize_username_candidate("_underscore").as_deref(),
                Some("user-_underscore")
            );
        }
    }

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
            // Everything before the first '@'.
            assert_eq!(
                OAuthService::email_local_part("weird@email@domain.com"),
                Some("weird".to_string())
            );
        }
    }

    mod generate_username {
        use super::*;

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
            // An unusable name falls through to the email local-part.
            let info = make_user_info("@@@", "john@example.com", None);
            assert_eq!(OAuthService::generate_username("github", &info), "john");
        }

        #[test]
        fn test_github_with_valid_email_only() {
            let info = make_user_info("", "john@example.com", None);
            assert_eq!(OAuthService::generate_username("github", &info), "john");
        }

        #[test]
        fn test_google_prefers_email_local() {
            let info = make_user_info("John Doe", "johndoe@gmail.com", None);
            assert_eq!(OAuthService::generate_username("google", &info), "johndoe");
        }

        #[test]
        fn test_google_with_invalid_email_uses_name() {
            // The email local-part has no usable character, so the name is used.
            let info = make_user_info("John Doe", "+++@gmail.com", None);
            assert_eq!(OAuthService::generate_username("google", &info), "john-doe");
        }

        #[test]
        fn test_google_with_empty_email() {
            let info = make_user_info("John Doe", "", None);
            assert_eq!(OAuthService::generate_username("google", &info), "john-doe");
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
        fn test_unknown_provider_falls_back_to_email() {
            // Unusable handle and name.
            let info = make_user_info("@@@", "john@example.com", Some("!!!"));
            assert_eq!(OAuthService::generate_username("twitter", &info), "john");
        }

        #[test]
        fn test_all_empty_returns_user() {
            let info = make_user_info("", "", None);
            assert_eq!(OAuthService::generate_username("github", &info), "user");
            assert_eq!(OAuthService::generate_username("google", &info), "user");
            assert_eq!(OAuthService::generate_username("twitter", &info), "user");
        }
    }

    mod find_or_create_user {
        use super::*;
        use async_trait::async_trait;
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Keeps users in memory and, like the users table, refuses a duplicate username or email.
        #[derive(Default)]
        struct MemoryUserRepository {
            users: Mutex<Vec<Model>>,
            create_calls: AtomicUsize,
            /// Refuse every username, as if each one tried were taken.
            all_usernames_taken: bool,
        }

        #[async_trait]
        impl UserRepository for MemoryUserRepository {
            async fn find_by_id(&self, _user_id: Uuid) -> Result<Option<Model>, AuthError> {
                unreachable!()
            }

            async fn find_by_email(&self, email: &str) -> Result<Option<Model>, AuthError> {
                let users = self.users.lock().unwrap();
                Ok(users.iter().find(|user| user.email == email).cloned())
            }

            async fn find_by_provider_id(
                &self,
                provider: &str,
                provider_id: &str,
            ) -> Result<Option<Model>, AuthError> {
                let users = self.users.lock().unwrap();
                Ok(users
                    .iter()
                    .find(|user| {
                        user.auth_provider == provider
                            && user.provider_id.as_deref() == Some(provider_id)
                    })
                    .cloned())
            }

            async fn create_user(&self, new_user: &Model) -> Result<Model, AuthError> {
                self.create_calls.fetch_add(1, Ordering::SeqCst);
                let mut users = self.users.lock().unwrap();
                if self.all_usernames_taken
                    || users.iter().any(|user| user.username == new_user.username)
                {
                    return Err(AuthError::UsernameAlreadyExists);
                }
                if users.iter().any(|user| user.email == new_user.email) {
                    return Err(AuthError::EmailAlreadyExists);
                }
                users.push(new_user.clone());
                Ok(new_user.clone())
            }

            async fn update_last_login(&self, _user_id: Uuid) -> Result<(), AuthError> {
                Ok(())
            }

            async fn verify_email(&self, _user_id: Uuid) -> Result<(), AuthError> {
                unreachable!()
            }

            async fn update_password(
                &self,
                _user_id: Uuid,
                _password_hash: &str,
            ) -> Result<(), AuthError> {
                unreachable!()
            }
        }

        fn oauth_user(provider_id: &str, email: &str, handle: Option<&str>) -> OAuthUserInfo {
            OAuthUserInfo {
                provider_id: provider_id.to_string(),
                email: email.to_string(),
                email_verified: true,
                name: "John Doe".to_string(),
                avatar_url: None,
                username: handle.map(|s| s.to_string()),
            }
        }

        fn assert_suffixed(username: &str, base: &str) {
            let suffix = username
                .strip_prefix(base)
                .and_then(|rest| rest.strip_prefix('-'))
                .unwrap_or_else(|| panic!("{username:?} is not {base:?} with a suffix"));
            assert_eq!(suffix.len(), USERNAME_SUFFIX_LEN - 1);
            assert!(
                suffix
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            );
        }

        #[test]
        fn test_suffixed_username_stays_within_the_length_cap() {
            assert_suffixed(&OAuthService::suffixed_username("john"), "john");

            let base = "a".repeat(MAX_USERNAME_LEN);
            let suffixed = OAuthService::suffixed_username(&base);
            assert_eq!(suffixed.len(), MAX_USERNAME_LEN);
            assert_suffixed(&suffixed, &base[..MAX_USERNAME_LEN - USERNAME_SUFFIX_LEN]);

            // A separator left at the cut is dropped before the suffix.
            let keep = MAX_USERNAME_LEN - USERNAME_SUFFIX_LEN;
            let base = format!("{}.{}", "a".repeat(keep - 1), "b".repeat(10));
            assert_suffixed(
                &OAuthService::suffixed_username(&base),
                &"a".repeat(keep - 1),
            );
        }

        #[tokio::test]
        async fn test_taken_username_is_retried_with_a_suffix() {
            let repo = MemoryUserRepository::default();
            // A GitHub user already holds "john".
            let holder = oauth_user("1", "jd@example.com", Some("john"));
            OAuthService::find_or_create_user(&repo, "github", &holder)
                .await
                .unwrap();
            repo.create_calls.store(0, Ordering::SeqCst);

            let info = oauth_user("2", "john@acme.com", None);
            let user = OAuthService::find_or_create_user(&repo, "google", &info)
                .await
                .unwrap();

            assert_suffixed(&user.username, "john");
            assert_eq!(repo.create_calls.load(Ordering::SeqCst), 2);
        }

        #[tokio::test]
        async fn test_two_users_with_the_same_base_username_both_sign_up() {
            let repo = MemoryUserRepository::default();
            let first_info = oauth_user("1", "john@gmail.com", None);
            let second_info = oauth_user("2", "john@acme.com", None);

            let first = OAuthService::find_or_create_user(&repo, "google", &first_info)
                .await
                .unwrap();
            let second = OAuthService::find_or_create_user(&repo, "google", &second_info)
                .await
                .unwrap();

            assert_eq!(first.username, "john");
            assert_suffixed(&second.username, "john");
            assert_ne!(first.user_id, second.user_id);
            assert_eq!(repo.users.lock().unwrap().len(), 2);

            // Each one's next login finds their own account.
            for (info, user) in [(&first_info, &first), (&second_info, &second)] {
                let again = OAuthService::find_or_create_user(&repo, "google", info)
                    .await
                    .unwrap();
                assert_eq!(again.user_id, user.user_id);
            }
            assert_eq!(repo.users.lock().unwrap().len(), 2);
        }

        #[tokio::test]
        async fn test_sign_up_fails_once_the_suffix_retries_run_out() {
            let repo = MemoryUserRepository {
                all_usernames_taken: true,
                ..Default::default()
            };

            let info = oauth_user("1", "john@acme.com", None);
            let result = OAuthService::find_or_create_user(&repo, "google", &info).await;

            assert!(matches!(result, Err(AuthError::UsernameAlreadyExists)));
            assert_eq!(
                repo.create_calls.load(Ordering::SeqCst),
                1 + USERNAME_SUFFIX_ATTEMPTS
            );
        }
    }

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
            assert!(json.contains(r#""pkce_verifier":"verifier""#));
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

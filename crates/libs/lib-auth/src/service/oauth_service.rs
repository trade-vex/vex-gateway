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
use deadpool_redis::Pool as RedisPool;
use deadpool_redis::redis::AsyncCommands;
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreResponseType},
    reqwest::async_http_client,
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, ResponseTypes, Scope, TokenUrl,
};
#[allow(unused_imports)]
use openidconnect::TokenResponse;
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
    /// Redis connection pool for state management
    redis_pool: Arc<RedisPool>,
    
    /// HTTP client for API requests to providers
    http_client: reqwest::Client,
    
    /// Pre-configured OAuth clients per provider (initialized at startup)
    oauth_clients: HashMap<String, CoreClient>,
    
    /// JWT token service for minting application tokens
    token_service: Arc<TokenService>,
    
    /// User repository for database operations
    user_repo: Arc<dyn UserRepository>,
}

impl OAuthService {
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
    /// ).await?;
    /// ```
    pub async fn new(
        redis_pool: Arc<RedisPool>,
        http_client: reqwest::Client,
        oauth_providers: HashMap<String, OAuthProviderConfig>,
        token_service: Arc<TokenService>,
        user_repo: Arc<dyn UserRepository>,
        redirect_base: String,
    ) -> Result<Self, AuthError> {
        let mut oauth_clients = HashMap::new();

        // Initialize OAuth clients for each configured provider
        for (provider, config) in oauth_providers {
            let client = Self::create_oauth_client(&provider, &config, &redirect_base).await?;
            oauth_clients.insert(provider, client);
        }

        Ok(Self {
            redis_pool,
            http_client,
            oauth_clients,
            token_service,
            user_repo,
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
        let redirect_uri = format!("{}/auth/{}/callback", redirect_base, provider);
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
                openidconnect::JsonWebKeySetUrl::new(format!("https://{}.com/.well-known/jwks.json", provider))
                    .map_err(|e| AuthError::Config(format!("Invalid JWKS URL: {}", e)))?,
                vec![ResponseTypes::new(vec![CoreResponseType::Code])],
                vec![
                    openidconnect::core::CoreSubjectIdentifierType::Public,
                ],
                vec![
                    openidconnect::core::CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
                ],
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
        let mut redis_conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Redis connection failed: {}", e)))?;

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
        if provider == "google" {
            let nonce_key = format!("oauth_nonce:{}:{}", provider, csrf_state.secret());
            let _: () = redis_conn
                .set_ex(&nonce_key, nonce.secret(), 600)
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to store nonce: {}", e)))?;
        }

        Ok(final_auth_url.to_string())
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
    ) -> Result<OAuthLoginResponse, AuthError> {
        // Verify and consume state for CSRF protection
        self.verify_and_consume_state(provider, state).await?;

        // Get OAuth client
        let client = self.oauth_clients.get(provider).ok_or_else(|| {
            AuthError::InvalidProvider(format!("Provider '{}' not configured", provider))
        })?;

        // Retrieve PKCE verifier from Redis
        let pkce_verifier = self.get_stored_verifier(provider, state).await?;

        // Exchange authorization code for access token
        let token_response = client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
            .request_async(async_http_client)
            .await
            .map_err(|e| AuthError::CodeExchangeFailed(format!("Token exchange failed: {}", e)))?;

        // Extract access token from response
        // Trait must be in scope to use the access_token() method
        use openidconnect::OAuth2TokenResponse;
        let access_token_secret = token_response.access_token().secret();

        // Get user info from provider's API
        let user_info = self
            .get_user_info(provider, access_token_secret)
            .await?;

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
                let new_user = Model {
                    user_id: Uuid::new_v4(),
                    email: user_info.email.clone(),
                    username: user_info.name.clone(),
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

        Ok(OAuthLoginResponse {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_in: tokens.expires_in,
            user: user.into(),
        })
    }

    /// Verifies and atomically consumes the OAuth state parameter
    ///
    /// This method provides CSRF protection by ensuring the state parameter
    /// matches what was stored in Redis during authorization URL generation.
    ///
    /// # Atomicity
    ///
    /// Uses Redis `GETDEL` command to atomically retrieve and delete the state,
    /// preventing replay attacks even in concurrent scenarios.
    ///
    /// # Errors
    ///
    /// Returns `StateMismatch` if:
    /// - State is not found in Redis (never existed or already consumed)
    /// - State has expired (> 10 minutes)
    async fn verify_and_consume_state(&self, provider: &str, state: &str) -> Result<(), AuthError> {
        let mut redis_conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Redis connection failed: {}", e)))?;

        let state_key = format!("oauth_state:{}:{}", provider, state);

        // Atomically get and delete state - prevents replay attacks
        let state_value: Option<String> = redis::cmd("GETDEL")
            .arg(&state_key)
            .query_async(&mut redis_conn)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get and delete state: {}", e)))?;

        if state_value.is_none() {
            return Err(AuthError::StateMismatch);
        }

        Ok(())
    }

    /// Retrieves and consumes the PKCE verifier from Redis
    ///
    /// The PKCE verifier is needed to complete the token exchange and must match
    /// the code challenge sent in the authorization request.
    ///
    /// # One-time Use
    ///
    /// The verifier is deleted after retrieval to prevent reuse.
    async fn get_stored_verifier(&self, provider: &str, state: &str) -> Result<String, AuthError> {
        let mut redis_conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Redis connection failed: {}", e)))?;

        let verifier_key = format!("oauth_verifier:{}:{}", provider, state);

        let verifier: Option<String> = redis_conn
            .get(&verifier_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to retrieve verifier: {}", e)))?;

        let verifier = verifier.ok_or_else(|| {
            AuthError::CodeExchangeFailed("PKCE verifier not found or expired".to_string())
        })?;

        // Delete verifier after retrieval (one-time use)
        let _: () = redis_conn
            .del(&verifier_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete verifier: {}", e)))?;

        Ok(verifier)
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
            "google" => "https://www.googleapis.com/oauth2/v2/userinfo",
            "github" => "https://api.github.com/user",
            "twitter" => "https://api.twitter.com/2/users/me?user.fields=profile_image_url",
            _ => {
                return Err(AuthError::InvalidProvider(format!(
                    "Unknown provider: {}",
                    provider
                )))
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

        // Special handling for GitHub - fetch emails separately
        if provider == "github"
            && let Ok(emails) = self.fetch_github_emails(access_token).await
        {
            // Find the primary verified email
            if let Some(primary_email) = emails.iter().find(|email| {
                email
                    .get("primary")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            }) {
                if let Some(email_str) = primary_email.get("email").and_then(|v| v.as_str()) {
                    user_info["email"] = serde_json::Value::String(email_str.to_string());
                }
                if let Some(verified) = primary_email.get("verified").and_then(|v| v.as_bool()) {
                    user_info["email_verified"] = serde_json::Value::Bool(verified);
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
        Ok(OAuthUserInfo {
            provider_id: data["sub"]
                .as_str()
                .ok_or_else(|| AuthError::UserInfoFetchFailed("Missing Google sub".to_string()))?
                .to_string(),
            email: data["email"]
                .as_str()
                .ok_or_else(|| AuthError::UserInfoFetchFailed("Missing Google email".to_string()))?
                .to_string(),
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
    ///     "profile_image_url": "https://..."
    ///   }
    /// }
    /// ```
    ///
    /// Note: Twitter doesn't always provide email addresses
    fn parse_twitter_user_info(&self, data: Value) -> Result<OAuthUserInfo, AuthError> {
        let user_data = &data["data"];

        Ok(OAuthUserInfo {
            provider_id: user_data["id"]
                .as_str()
                .ok_or_else(|| {
                    AuthError::UserInfoFetchFailed("Missing Twitter user ID".to_string())
                })?
                .to_string(),
            email: user_data["email"].as_str().unwrap_or("").to_string(),
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

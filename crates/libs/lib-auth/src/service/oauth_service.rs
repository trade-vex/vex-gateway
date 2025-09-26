use crate::config::OAuthProviderConfig;
use crate::dto::auth_response_dto::{OAuthLoginResponse, OAuthTokenResponse, OAuthUserInfo};
use crate::error::auth_error::AuthError;
use crate::service::token_service::TokenService;
use crate::utils::rand::generate_random_string;
use deadpool_redis::Pool as RedisPool;
use deadpool_redis::redis::AsyncCommands;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

use crate::models::user::{Model, UserStatus};
use crate::repository::UserRepository;

pub struct OAuthService {
    redis_pool: Arc<RedisPool>,
    http_client: reqwest::Client,
    oauth_providers: HashMap<String, OAuthProviderConfig>,
    token_service: Arc<TokenService>,
    user_repo: Arc<dyn UserRepository>,
}

impl OAuthService {
    pub fn new(
        redis_pool: Arc<RedisPool>,
        http_client: reqwest::Client,
        oauth_providers: HashMap<String, OAuthProviderConfig>,
        token_service: Arc<TokenService>,
        user_repo: Arc<dyn UserRepository>,
    ) -> Self {
        Self {
            redis_pool,
            http_client,
            oauth_providers,
            token_service,
            user_repo,
        }
    }

    pub async fn get_available_providers(&self) -> Vec<String> {
        self.oauth_providers.keys().cloned().collect()
    }

    pub async fn get_auth_url(&self, provider: &str) -> Result<String, AuthError> {
        if provider.is_empty() {
            return Err(AuthError::InvalidProvider(
                "Provider name is empty".to_string(),
            ));
        }

        let provider_config = self.oauth_providers.get(provider).ok_or_else(|| {
            AuthError::InvalidProvider(format!("Provider '{}' not configured", provider))
        })?;

        // Generate state for CSRF protection
        let state = generate_random_string(32);

        // Store state in Redis with 10-minute expiry
        let mut redis_conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Redis connection failed: {}", e)))?;

        let state_key = format!("oauth_state:{}:{}", provider, state);
        let _: () = redis_conn
            .set_ex(&state_key, "valid", 600)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store state: {}", e)))?;

        // Build redirect URI
        let redirect_uri = format!(
            "{}/auth/{}/callback",
            "http://localhost:8081",
            provider // TODO: Get from config
        );

        // Build authorization URL
        let mut auth_url = Url::parse(&provider_config.auth_url)
            .map_err(|e| AuthError::Config(format!("Invalid auth URL for {}: {}", provider, e)))?;

        {
            let mut query_pairs = auth_url.query_pairs_mut();
            query_pairs.append_pair("client_id", &provider_config.client_id);
            query_pairs.append_pair("response_type", &provider_config.response_type);
            query_pairs.append_pair("redirect_uri", &redirect_uri);
            query_pairs.append_pair("scope", &provider_config.scopes.join(" "));
            query_pairs.append_pair("state", &state);

            // Provider-specific parameters
            if provider == "twitter" {
                let (code_verifier, code_challenge) = self.generate_pkce_pair();

                // Store verifier in Redis with state
                let verifier_key = format!("oauth_verifier:{}:{}", provider, state);
                let _: () = redis_conn
                    .set_ex(&verifier_key, &code_verifier, 600)
                    .await
                    .map_err(|e| AuthError::Redis(format!("Failed to store verifier: {}", e)))?;

                query_pairs.append_pair("code_challenge", &code_challenge);
                query_pairs.append_pair("code_challenge_method", "S256");
            }
        }

        Ok(auth_url.to_string())
    }

    pub async fn handle_callback(
        &self,
        provider: &str,
        code: &str,
        state: &str,
    ) -> Result<OAuthLoginResponse, AuthError> {
        // Verify and consume state
        self.verify_and_consume_state(provider, state).await?;

        // Get provider config
        let provider_config = self.oauth_providers.get(provider).ok_or_else(|| {
            AuthError::InvalidProvider(format!("Provider '{}' not configured", provider))
        })?;

        // Exchange code for access token - pass state parameter
        let token_response = self
            .exchange_code_for_token(provider, provider_config, code, state)
            .await?;

        // Get user info from provider
        let user_info = self
            .get_user_info(provider, &token_response.access_token, provider_config)
            .await?;

        // Find or create user in our database
        let user = match self
            .user_repo
            .find_by_provider_id(provider, &user_info.provider_id)
            .await?
        {
            Some(existing_user) => {
                // Update last login
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

                // The service is now responsible for creating the User entity, including the UUID.
                // This separates the business logic of entity creation from the persistence logic of the repository.
                let now = chrono::Utc::now();
                let new_user = Model {
                    user_id: Uuid::new_v4(),
                    email: user_info.email.clone(),
                    username: user_info.name.clone(),
                    password_hash: None,
                    auth_provider: provider.to_string(),
                    provider_id: Some(user_info.provider_id.clone()),
                    avatar_url: user_info.avatar_url.clone(),
                    email_verified: true, // OAuth emails are generally considered verified
                    status: UserStatus::Active,
                    created_at: now,
                    updated_at: now,
                    last_login: Some(now),
                };

                // Create new user by passing the full User object.
                // NOTE: This requires the `UserRepository` trait to have a `create_user` method,
                // for example: `async fn create_user(&self, user: &User) -> Result<User, AuthError>;`
                self.user_repo.create_user(&new_user).await?
            }
        };

        // Generate JWT tokens for our app
        let tokens = self
            .token_service
            .mint_tokens(user.user_id)
            .await
            .map_err(|e| AuthError::CodeExchangeFailed(format!("Token minting failed: {}", e)))?;

        Ok(OAuthLoginResponse {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_in: tokens.expires_in as u64,
            user: user.into(),
        })
    }

    async fn verify_and_consume_state(&self, provider: &str, state: &str) -> Result<(), AuthError> {
        let mut redis_conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Redis connection failed: {}", e)))?;

        let state_key = format!("oauth_state:{}:{}", provider, state);

        // Check if state exists
        let state_valid: bool = redis_conn
            .exists(&state_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to check state: {}", e)))?;

        if !state_valid {
            return Err(AuthError::StateMismatch);
        }

        // Delete the state (one-time use)
        let _: () = redis_conn
            .del(&state_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete state: {}", e)))?;

        Ok(())
    }

    async fn exchange_code_for_token(
        &self,
        provider: &str,
        config: &OAuthProviderConfig,
        code: &str,
        state: &str, // Add state parameter
    ) -> Result<OAuthTokenResponse, AuthError> {
        let redirect_uri = format!(
            "{}/auth/{}/callback",
            "http://localhost:8081",
            provider // TODO: Get from config
        );

        let mut params = vec![
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ];
        // Add code_verifier for Twitter
        let mut code_verifier_owned = None;
        if provider == "twitter" {
            let code_verifier = self.get_stored_verifier(provider, state).await?;
            code_verifier_owned = Some(code_verifier);
        }
        if let Some(ref code_verifier) = code_verifier_owned {
            params.push(("code_verifier", code_verifier.as_str()));
        }

        let response = self
            .http_client
            .post(&config.token_url)
            .header("Accept", "application/json")
            .header("User-Agent", "vex-gateway-auth/1.0")
            .form(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(AuthError::CodeExchangeFailed(format!(
                "HTTP {}: {}",
                status, error_text
            )));
        }

        let token_response: OAuthTokenResponse = response
            .json()
            .await
            .map_err(|e| AuthError::CodeExchangeFailed(format!("JSON parsing failed: {}", e)))?;

        Ok(token_response)
    }

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

        // Delete the verifier (one-time use)
        let _: () = redis_conn
            .del(&verifier_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete verifier: {}", e)))?;

        Ok(verifier)
    }

    pub async fn get_user_info(
        &self,
        provider: &str,
        access_token: &str,
        config: &OAuthProviderConfig,
    ) -> Result<OAuthUserInfo, AuthError> {
        let response = self
            .http_client
            .get(&config.user_info_url)
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
            // Find the primary email
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

        // Parse the user_info Value into OAuthUserInfo based on provider
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

    fn parse_google_user_info(&self, data: Value) -> Result<OAuthUserInfo, AuthError> {
        Ok(OAuthUserInfo {
            provider_id: data["id"]
                .as_str()
                .ok_or_else(|| {
                    AuthError::UserInfoFetchFailed("Missing Google user ID".to_string())
                })?
                .to_string(),
            email: data["email"]
                .as_str()
                .ok_or_else(|| AuthError::UserInfoFetchFailed("Missing Google email".to_string()))?
                .to_string(),
            name: data["name"].as_str().unwrap_or("Unknown User").to_string(),
            username: None,
            avatar_url: data["picture"].as_str().map(|s| s.to_string()),
        })
    }

    async fn parse_github_user_info(&self, data: Value) -> Result<OAuthUserInfo, AuthError> {
        // Get user's primary email if not public
        let email = if let Some(email) = data["email"].as_str() {
            email.to_string()
        } else {
            // This case should not happen since fetch_github_emails is called in get_user_info
            // before this method is called, but handle it gracefully
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
        })
    }

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
        })
    }

    // Update the PKCE generation to return both verifier and challenge
    fn generate_pkce_pair(&self) -> (String, String) {
        use base64::Engine;
        use sha2::{Digest, Sha256};

        let verifier = generate_random_string(128);
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = hasher.finalize();
        let challenge_encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(challenge);

        (verifier, challenge_encoded)
    }

    pub async fn logout(&self, token: &str) -> Result<(), AuthError> {
        let claims = self.token_service.validate_jwt(token)?;

        // Calculate time until expiration
        let current_time: i64 = chrono::Utc::now().timestamp();
        let time_remaining = if claims.exp > current_time {
            claims.exp - current_time
        } else {
            return Ok(()); // Token already expired
        };

        // Add to Redis blacklist
        let mut redis_conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Redis connection failed: {}", e)))?;

        let _: () = redis_conn
            .set_ex(
                format!("blacklisted_token:{}", token),
                "1",
                time_remaining as u64,
            )
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to blacklist token: {}", e)))?;

        Ok(())
    }
}

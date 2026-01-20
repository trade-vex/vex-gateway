use crate::error::auth_error::AuthError;
use deadpool_redis::Pool as RedisPool;
use deadpool_redis::redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Session data stored in Redis - only tokens and user_id
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub user_id: Uuid,
    pub access_token: String,
    pub refresh_token: String,
}

/// One-time login code data for mobile OAuth exchange (short TTL)
/// Used for PKCE-like "second hop" verification in mobile deep-link flow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginCodeData {
    /// The session_id to return after successful verification
    pub session_id: String,
    /// Serialized user info JSON
    pub user_info_json: String,
    /// App challenge (SHA256 hash) that must match S256(verifier)
    pub app_challenge: String,
}

/// Service for managing user sessions in Redis
#[derive(Clone)]
pub struct SessionService {
    redis_pool: Arc<RedisPool>,
    session_ttl_seconds: u64,
}

impl SessionService {
    pub fn new(redis_pool: Arc<RedisPool>, session_ttl_seconds: i64) -> Self {
        if session_ttl_seconds <= 0 {
            panic!(
                "session_ttl_seconds must be greater than 0, got: {}",
                session_ttl_seconds
            );
        }
        Self {
            redis_pool,
            session_ttl_seconds: session_ttl_seconds as u64,
        }
    }

    /// Create a new session for a user
    pub async fn create_session(
        &self,
        user_id: Uuid,
        access_token: String,
        refresh_token: String,
    ) -> Result<String, AuthError> {
        let session_id = Uuid::new_v4().to_string();
        let session_key = format!("session:{}", session_id);

        let session_data = SessionData {
            user_id,
            access_token,
            refresh_token,
        };

        let session_json = serde_json::to_string(&session_data)
            .map_err(|e| AuthError::Redis(format!("Failed to serialize session: {}", e)))?;

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let _: () = conn
            .set_ex(&session_key, session_json, self.session_ttl_seconds)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store session: {}", e)))?;

        Ok(session_id)
    }

    /// Get session data by session ID
    pub async fn get_session(&self, session_id: &str) -> Result<SessionData, AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let session_json: Option<String> = conn
            .get(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get session: {}", e)))?;

        let session_json = session_json.ok_or(AuthError::InvalidRequest)?;

        let session_data: SessionData = serde_json::from_str(&session_json)
            .map_err(|e| AuthError::Redis(format!("Failed to deserialize session: {}", e)))?;

        Ok(session_data)
    }

    /// Delete a session
    pub async fn delete_session(&self, session_id: &str) -> Result<(), AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let _: () = conn
            .del(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;

        Ok(())
    }

    /// Refresh/extend a session's TTL
    pub async fn refresh_session(&self, session_id: &str) -> Result<(), AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        // Check if session exists
        let exists: bool = conn
            .exists(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to check session: {}", e)))?;

        if !exists {
            return Err(AuthError::InvalidRequest);
        }

        // Extend the TTL
        let _: () = conn
            .expire(&session_key, self.session_ttl_seconds as i64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to refresh session: {}", e)))?;

        Ok(())
    }

    /// Update tokens in an existing session
    /// Uses a single Redis connection for read-modify-write to minimize race conditions
    pub async fn update_session_tokens(
        &self,
        session_id: &str,
        access_token: String,
        refresh_token: String,
    ) -> Result<(), AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        // Get existing session to preserve user_id (using same connection)
        let session_json: Option<String> = conn
            .get(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get session: {}", e)))?;

        let session_json = session_json.ok_or(AuthError::InvalidRequest)?;

        let session_data: SessionData = serde_json::from_str(&session_json)
            .map_err(|e| AuthError::Redis(format!("Failed to deserialize session: {}", e)))?;

        // Create updated session data with new tokens
        let updated_session = SessionData {
            user_id: session_data.user_id,
            access_token,
            refresh_token,
        };

        let updated_json = serde_json::to_string(&updated_session)
            .map_err(|e| AuthError::Redis(format!("Failed to serialize session: {}", e)))?;

        // Update the session with new tokens and reset TTL atomically using SETEX
        let _: () = conn
            .set_ex(&session_key, updated_json, self.session_ttl_seconds)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to update session: {}", e)))?;

        Ok(())
    }

    /// Revoke all sessions for a user except the specified session
    pub async fn revoke_user_sessions_except(
        &self,
        user_id: Uuid,
        keep_session_id: &str,
    ) -> Result<usize, AuthError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        // Find all sessions
        let pattern = "session:*".to_string();
        let keys: Vec<String> = conn
            .keys(pattern)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to find sessions: {}", e)))?;

        let mut revoked_count = 0;

        // Check each session and delete if it belongs to this user but isn't the current session
        for key in keys {
            if let Ok(Some(session_json)) = conn.get::<String, Option<String>>(key.clone()).await
                && let Ok(session_data) = serde_json::from_str::<SessionData>(&session_json)
                && session_data.user_id == user_id
            {
                // Extract session_id from key (format: "session:{session_id}")
                if let Some(session_id) = key.strip_prefix("session:") {
                    // Delete if it's not the session we want to keep
                    if session_id != keep_session_id {
                        let _: () = conn.del(key).await.map_err(|e| {
                            AuthError::Redis(format!("Failed to delete session: {}", e))
                        })?;
                        revoked_count += 1;
                    }
                }
            }
        }

        Ok(revoked_count)
    }

    /// Revoke all sessions for a user
    pub async fn revoke_all_user_sessions(&self, user_id: Uuid) -> Result<usize, AuthError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        // Find all sessions
        let pattern = "session:*".to_string();
        let keys: Vec<String> = conn
            .keys(pattern)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to find sessions: {}", e)))?;

        let mut revoked_count = 0;

        // Check each session and delete if it belongs to this user
        for key in keys {
            if let Ok(Some(session_json)) = conn.get::<String, Option<String>>(key.clone()).await
                && let Ok(session_data) = serde_json::from_str::<SessionData>(&session_json)
                && session_data.user_id == user_id
            {
                let _: () = conn
                    .del(key)
                    .await
                    .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;
                revoked_count += 1;
            }
        }

        Ok(revoked_count)
    }

    // ============== Mobile OAuth Login Code Methods ==============

    /// Create a short-lived, one-time login_code for mobile deep-link return
    ///
    /// This implements the "second hop" PKCE-like flow for mobile OAuth:
    /// - Instead of putting session_id in the deep link (interceptable),
    /// - We return a login_code that requires the app's verifier to redeem
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to return after verification
    /// * `user_info_json` - Serialized user info to return
    /// * `app_challenge` - SHA256 hash of the app's verifier
    ///
    /// # Returns
    ///
    /// A one-time login_code (UUID) that expires in 120 seconds
    pub async fn create_login_code(
        &self,
        session_id: String,
        user_info_json: String,
        app_challenge: String,
    ) -> Result<String, AuthError> {
        let login_code = Uuid::new_v4().to_string();
        let key = format!("oauth_login_code:{}", login_code);

        let data = LoginCodeData {
            session_id,
            user_info_json,
            app_challenge,
        };

        let json = serde_json::to_string(&data)
            .map_err(|e| AuthError::Redis(format!("Failed to serialize login_code data: {}", e)))?;

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        // Very short TTL (120 seconds) - just enough for the app to exchange
        let _: () = conn
            .set_ex(&key, json, 120)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store login_code: {}", e)))?;

        Ok(login_code)
    }

    /// Retrieve login_code data (does NOT delete - use delete_login_code after verification)
    ///
    /// # Arguments
    ///
    /// * `login_code` - The one-time code from the deep link
    ///
    /// # Returns
    ///
    /// The LoginCodeData if found, or InvalidRequest if expired/not found
    pub async fn get_login_code(&self, login_code: &str) -> Result<LoginCodeData, AuthError> {
        let key = format!("oauth_login_code:{}", login_code);

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let json: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get login_code: {}", e)))?;

        let json = json.ok_or(AuthError::InvalidRequest)?;
        let data: LoginCodeData = serde_json::from_str(&json)
            .map_err(|e| AuthError::Redis(format!("Failed to deserialize login_code: {}", e)))?;

        Ok(data)
    }

    /// Delete a login_code after successful verification (one-time use)
    ///
    /// # Arguments
    ///
    /// * `login_code` - The code to delete
    pub async fn delete_login_code(&self, login_code: &str) -> Result<(), AuthError> {
        let key = format!("oauth_login_code:{}", login_code);

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let _: () = conn
            .del(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete login_code: {}", e)))?;

        Ok(())
    }
}

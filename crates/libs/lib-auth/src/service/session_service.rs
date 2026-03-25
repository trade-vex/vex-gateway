use crate::error::auth_error::AuthError;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
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
    /// Rotating refresh token for seamless mobile session renewal
    pub refresh_token: String,
    /// Serialized user info JSON
    pub user_info_json: String,
    /// App challenge (SHA256 hash) that must match S256(verifier)
    pub app_challenge: String,
}

/// Service for managing user sessions in Redis
#[derive(Clone)]
pub struct SessionService {
    conn: ConnectionManager,
    session_ttl_seconds: u64,
}

impl SessionService {
    pub fn new(conn: ConnectionManager, session_ttl_seconds: i64) -> Self {
        if session_ttl_seconds <= 0 {
            panic!(
                "session_ttl_seconds must be greater than 0, got: {}",
                session_ttl_seconds
            );
        }
        Self {
            conn,
            session_ttl_seconds: session_ttl_seconds as u64,
        }
    }

    /// Get the session TTL in seconds (for cookie max_age alignment)
    pub fn ttl_seconds(&self) -> u64 {
        self.session_ttl_seconds
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

        let mut conn = self.conn.clone();

        let _: () = conn
            .set_ex(&session_key, session_json, self.session_ttl_seconds)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store session: {}", e)))?;

        Ok(session_id)
    }

    /// Get session data by session ID
    pub async fn get_session(&self, session_id: &str) -> Result<SessionData, AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self.conn.clone();

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

        let mut conn = self.conn.clone();

        let _: () = conn
            .del(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;

        Ok(())
    }

    /// Refresh/extend a session's TTL
    pub async fn refresh_session(&self, session_id: &str) -> Result<(), AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self.conn.clone();

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

        // Get existing session to preserve user_id
        let session_data = self.get_session(session_id).await?;

        // Create updated session data with new tokens
        let updated_session = SessionData {
            user_id: session_data.user_id,
            access_token,
            refresh_token,
        };

        let updated_json = serde_json::to_string(&updated_session)
            .map_err(|e| AuthError::Redis(format!("Failed to serialize session: {}", e)))?;

        let mut conn = self.conn.clone();

        // Update the session with new tokens
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
        let mut conn = self.conn.clone();

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
        let mut conn = self.conn.clone();

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
        refresh_token: String,
        user_info_json: String,
        app_challenge: String,
    ) -> Result<String, AuthError> {
        let login_code = Uuid::new_v4().to_string();
        let key = format!("oauth_login_code:{}", login_code);

        let data = LoginCodeData {
            session_id,
            refresh_token,
            user_info_json,
            app_challenge,
        };

        let json = serde_json::to_string(&data)
            .map_err(|e| AuthError::Redis(format!("Failed to serialize login_code data: {}", e)))?;

        let mut conn = self.conn.clone();

        // Very short TTL (120 seconds) - just enough for the app to exchange
        let _: () = conn
            .set_ex(&key, json, 120)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store login_code: {}", e)))?;

        Ok(login_code)
    }

    /// Atomically retrieve and delete login_code data (prevents race conditions)
    ///
    /// Uses Redis GETDEL command to ensure one-time use semantics and prevent replay attacks.
    /// This is the preferred method for login_code exchange as it eliminates the race condition
    /// where two concurrent requests could both retrieve the same code before deletion.
    ///
    /// # Arguments
    ///
    /// * `login_code` - The one-time code from the deep link
    ///
    /// # Returns
    ///
    /// The LoginCodeData if found, or InvalidRequest if expired/not found
    ///
    /// # Security
    ///
    /// - Atomic operation: Get and delete happen in a single Redis command
    /// - Prevents replay attacks: Code can only be retrieved once
    /// - Eliminates race conditions: Concurrent requests cannot both succeed
    pub async fn get_and_delete_login_code(
        &self,
        login_code: &str,
    ) -> Result<LoginCodeData, AuthError> {
        let key = format!("oauth_login_code:{}", login_code);

        let mut conn = self.conn.clone();

        // Atomic: Get and delete in one operation (prevents race conditions)
        let json: Option<String> = redis::cmd("GETDEL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to GETDEL login_code: {}", e)))?;

        let json = json.ok_or(AuthError::InvalidRequest)?;
        let data: LoginCodeData = serde_json::from_str(&json)
            .map_err(|e| AuthError::Redis(format!("Failed to deserialize login_code: {}", e)))?;

        Ok(data)
    }
}

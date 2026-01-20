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

    /// Delete a session (without ownership check)
    ///
    /// **Security Note:** This method does NOT verify session ownership.
    /// Use `delete_session_for_user` for user-facing endpoints.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self.conn.clone();

        let _: () = conn
            .del(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;

        Ok(())
    }

    /// Delete a session with ownership verification
    ///
    /// This method verifies that the session belongs to the specified user
    /// before deleting it, preventing users from deleting other users' sessions.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to delete
    /// * `user_id` - The user ID that must own the session
    ///
    /// # Errors
    ///
    /// * `AuthError::InvalidRequest` - Session not found
    /// * `AuthError::Unauthorized` - Session does not belong to the user
    /// * `AuthError::Redis` - Redis operation failed
    pub async fn delete_session_for_user(
        &self,
        session_id: &str,
        user_id: Uuid,
    ) -> Result<(), AuthError> {
        // First, verify the session exists and belongs to the user
        let session_data = self.get_session(session_id).await?;

        if session_data.user_id != user_id {
            tracing::warn!(
                session_id = session_id,
                claimed_user_id = %user_id,
                actual_user_id = %session_data.user_id,
                "Session ownership verification failed"
            );
            return Err(AuthError::Unauthorized);
        }

        // Session belongs to user, safe to delete
        self.delete_session(session_id).await
    }

    /// Refresh/extend a session's TTL (without ownership check)
    ///
    /// **Security Note:** This method does NOT verify session ownership.
    /// Use `refresh_session_for_user` for user-facing endpoints.
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

    /// Refresh/extend a session's TTL with ownership verification
    ///
    /// This method verifies that the session belongs to the specified user
    /// before refreshing it, preventing users from refreshing other users' sessions.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to refresh
    /// * `user_id` - The user ID that must own the session
    ///
    /// # Errors
    ///
    /// * `AuthError::InvalidRequest` - Session not found
    /// * `AuthError::Unauthorized` - Session does not belong to the user
    /// * `AuthError::Redis` - Redis operation failed
    pub async fn refresh_session_for_user(
        &self,
        session_id: &str,
        user_id: Uuid,
    ) -> Result<(), AuthError> {
        // First, verify the session exists and belongs to the user
        let session_data = self.get_session(session_id).await?;

        if session_data.user_id != user_id {
            tracing::warn!(
                session_id = session_id,
                claimed_user_id = %user_id,
                actual_user_id = %session_data.user_id,
                "Session ownership verification failed during refresh"
            );
            return Err(AuthError::Unauthorized);
        }

        // Session belongs to user, safe to refresh
        self.refresh_session(session_id).await
    }

    /// Update tokens in an existing session (without ownership check)
    ///
    /// **Security Note:** This method does NOT verify session ownership.
    /// Use `update_session_tokens_for_user` for user-facing endpoints.
    ///
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

    /// Update tokens in an existing session with ownership verification
    ///
    /// This method verifies that the session belongs to the specified user
    /// before updating tokens, preventing users from hijacking other users' sessions.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session ID to update
    /// * `user_id` - The user ID that must own the session
    /// * `access_token` - New access token
    /// * `refresh_token` - New refresh token
    ///
    /// # Errors
    ///
    /// * `AuthError::InvalidRequest` - Session not found
    /// * `AuthError::Unauthorized` - Session does not belong to the user
    /// * `AuthError::Redis` - Redis operation failed
    pub async fn update_session_tokens_for_user(
        &self,
        session_id: &str,
        user_id: Uuid,
        access_token: String,
        refresh_token: String,
    ) -> Result<(), AuthError> {
        let session_key = format!("session:{}", session_id);

        // Get existing session and verify ownership
        let session_data = self.get_session(session_id).await?;

        if session_data.user_id != user_id {
            tracing::warn!(
                session_id = session_id,
                claimed_user_id = %user_id,
                actual_user_id = %session_data.user_id,
                "Session ownership verification failed during token update"
            );
            return Err(AuthError::Unauthorized);
        }

        // Session belongs to user, safe to update
        let updated_session = SessionData {
            user_id,
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
    ///
    /// Uses Redis SCAN instead of KEYS to avoid blocking the Redis server.
    /// SCAN is non-blocking and iterates through keys incrementally.
    pub async fn revoke_user_sessions_except(
        &self,
        user_id: Uuid,
        keep_session_id: &str,
    ) -> Result<usize, AuthError> {
        let mut conn = self.conn.clone();

        // Use SCAN instead of KEYS to avoid blocking Redis
        // SCAN iterates through keys incrementally without blocking
        // Collect keys first to avoid borrow issues with the connection
        let keys: Vec<String> = {
            let mut iter: redis::AsyncIter<String> = conn
                .scan_match("session:*")
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to scan sessions: {}", e)))?;

            let mut keys = Vec::new();
            while let Some(key_result) = iter.next_item().await {
                match key_result {
                    Ok(k) => keys.push(k),
                    Err(e) => {
                        tracing::warn!(error = %e, "Error during session scan, skipping key");
                    }
                }
            }
            keys
        };

        let mut revoked_count = 0;

        // Process collected keys
        for key in keys {
            if let Ok(Some(session_json)) = conn.get::<&str, Option<String>>(&key).await
                && let Ok(session_data) = serde_json::from_str::<SessionData>(&session_json)
                && session_data.user_id == user_id
            {
                // Extract session_id from key (format: "session:{session_id}")
                if let Some(session_id) = key.strip_prefix("session:") {
                    // Delete if it's not the session we want to keep
                    if session_id != keep_session_id {
                        let _: () = conn.del(&key).await.map_err(|e| {
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
    ///
    /// Uses Redis SCAN instead of KEYS to avoid blocking the Redis server.
    /// SCAN is non-blocking and iterates through keys incrementally.
    pub async fn revoke_all_user_sessions(&self, user_id: Uuid) -> Result<usize, AuthError> {
        let mut conn = self.conn.clone();

        // Use SCAN instead of KEYS to avoid blocking Redis
        // SCAN iterates through keys incrementally without blocking
        // Collect keys first to avoid borrow issues with the connection
        let keys: Vec<String> = {
            let mut iter: redis::AsyncIter<String> = conn
                .scan_match("session:*")
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to scan sessions: {}", e)))?;

            let mut keys = Vec::new();
            while let Some(key_result) = iter.next_item().await {
                match key_result {
                    Ok(k) => keys.push(k),
                    Err(e) => {
                        tracing::warn!(error = %e, "Error during session scan, skipping key");
                    }
                }
            }
            keys
        };

        let mut revoked_count = 0;

        // Process collected keys
        for key in keys {
            if let Ok(Some(session_json)) = conn.get::<&str, Option<String>>(&key).await
                && let Ok(session_data) = serde_json::from_str::<SessionData>(&session_json)
                && session_data.user_id == user_id
            {
                let _: () = conn
                    .del(&key)
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

        let mut conn = self.conn.clone();

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

        let mut conn = self.conn.clone();

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

        let mut conn = self.conn.clone();

        let _: () = conn
            .del(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete login_code: {}", e)))?;

        Ok(())
    }
}

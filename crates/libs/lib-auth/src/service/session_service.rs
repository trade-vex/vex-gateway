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

/// Service for managing user sessions in Redis
#[derive(Clone)]
pub struct SessionService {
    redis_pool: Arc<RedisPool>,
    session_ttl_seconds: i64,
}

impl SessionService {
    pub fn new(redis_pool: Arc<RedisPool>, session_ttl_seconds: i64) -> Self {
        Self {
            redis_pool,
            session_ttl_seconds,
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

        let ttl = self.session_ttl_seconds as u64;
        let _: () = conn
            .set_ex(&session_key, session_json, ttl)
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
            .expire(&session_key, self.session_ttl_seconds)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to refresh session: {}", e)))?;

        Ok(())
    }

    /// Update tokens in an existing session
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

        // Get existing session to preserve user_id
        let session_data = self.get_session(session_id).await?;

        // Create updated session data with new tokens
        let updated_session = SessionData {
            user_id: session_data.user_id,
            access_token,
            refresh_token,
        };

        let session_json = serde_json::to_string(&updated_session)
            .map_err(|e| AuthError::Redis(format!("Failed to serialize session: {}", e)))?;

        // Update the session with new tokens, preserving existing TTL
        let _: () = conn
            .set(&session_key, session_json)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to update session: {}", e)))?;

        // Extend TTL to the configured session TTL
        let _: () = conn
            .expire(&session_key, self.session_ttl_seconds)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to extend session TTL: {}", e)))?;

        Ok(())
    }

    /// Get the session ID from a cookie name
    pub async fn get_session_id_from_cookie(&self, cookie_value: &str) -> Option<String> {
        if cookie_value.is_empty() {
            return None;
        }
        Some(cookie_value.to_string())
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
                let _: () = conn.del(key).await.map_err(|e| {
                    AuthError::Redis(format!("Failed to delete session: {}", e))
                })?;
                revoked_count += 1;
            }
        }

        Ok(revoked_count)
    }
}

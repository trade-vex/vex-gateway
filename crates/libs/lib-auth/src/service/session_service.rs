//! Session Management Service
//!
//! This service manages user sessions in Redis by storing session data (access token,
//! refresh token, user info) keyed by session ID. The session ID is sent to clients
//! via HTTP-only cookies for security.

use crate::dto::auth_response_dto::UserInfo;
use crate::dto::session_dto::SessionData;
use crate::error::auth_error::AuthError;
use deadpool_redis::Pool;
use deadpool_redis::redis::AsyncCommands;
use serde_json;
use std::sync::Arc;
use uuid::Uuid;

/// Session service for managing user sessions in Redis
#[derive(Clone)]
pub struct SessionService {
    redis_pool: Arc<Pool>,
    session_ttl_secs: u64,
}

impl SessionService {
    pub fn new(redis_pool: Arc<Pool>, session_ttl_days: i64) -> Self {
        let session_ttl_secs = (session_ttl_days * 24 * 3600) as u64;
        Self {
            redis_pool,
            session_ttl_secs,
        }
    }

    /// Create a new session with the provided data
    /// Returns the session ID
    pub async fn create_session(
        &self,
        user_info: UserInfo,
        access_token: String,
        refresh_token: String,
    ) -> Result<String, AuthError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        // Generate a unique session ID
        let session_id = format!("session:{}", Uuid::new_v4().to_string());

        let session_data = SessionData {
            user: user_info,
            access_token,
            refresh_token,
        };

        // Serialize session data
        let session_json = serde_json::to_string(&session_data).map_err(|e| {
            AuthError::Validation(format!("Failed to serialize session data: {}", e))
        })?;

        // Store in Redis with expiration
        let _: () = conn
            .set_ex(&session_id, session_json, self.session_ttl_secs)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store session: {}", e)))?;

        Ok(session_id.clone())
    }

    /// Get session data by session ID
    pub async fn get_session(&self, session_id: &str) -> Result<SessionData, AuthError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let session_json: Option<String> = conn
            .get(session_id)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get session: {}", e)))?;

        match session_json {
            Some(json) => {
                let session_data: SessionData = serde_json::from_str(&json).map_err(|e| {
                    AuthError::Validation(format!("Failed to deserialize session data: {}", e))
                })?;
                Ok(session_data)
            }
            None => Err(AuthError::Validation("Session not found or expired".to_string())),
        }
    }

    /// Update session data (e.g., when refreshing tokens)
    pub async fn update_session(
        &self,
        session_id: &str,
        access_token: String,
        refresh_token: String,
    ) -> Result<(), AuthError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        // Get existing session to preserve user info
        let existing_session = self.get_session(session_id).await?;

        let session_data = SessionData {
            user: existing_session.user,
            access_token,
            refresh_token,
        };

        let session_json = serde_json::to_string(&session_data).map_err(|e| {
            AuthError::Validation(format!("Failed to serialize session data: {}", e))
        })?;

        // Get TTL for existing session to maintain it
        let ttl: i64 = conn
            .ttl(session_id)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get session TTL: {}", e)))?;

        let ttl_to_use = if ttl > 0 {
            ttl as u64
        } else {
            self.session_ttl_secs
        };

        let _: () = conn
            .set_ex(session_id, session_json, ttl_to_use)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to update session: {}", e)))?;

        Ok(())
    }

    /// Delete a session
    pub async fn destroy_session(&self, session_id: &str) -> Result<(), AuthError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let _: () = conn.del(session_id)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;

        Ok(())
    }

    /// Check if a session exists
    pub async fn session_exists(&self, session_id: &str) -> Result<bool, AuthError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let exists: bool = conn
            .exists(session_id)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to check session: {}", e)))?;

        Ok(exists)
    }

    /// Revoke all sessions for a user (used for password change or account lockdown)
    pub async fn revoke_user_sessions(&self, user_id: Uuid) -> Result<(), AuthError> {
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        // Find all session keys matching the pattern
        let pattern = "session:*".to_string();
        let keys: Vec<String> = conn
            .keys(&pattern)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to find sessions: {}", e)))?;

        // Check each session and delete if it belongs to this user
        for key in keys {
            if let Ok(Some(session_json)) = conn.get::<String, Option<String>>(key.clone()).await {
                if let Ok(session_data) = serde_json::from_str::<SessionData>(&session_json) {
                    if session_data.user.user_id == user_id {
                        let _: () = conn.del(key).await.map_err(|e| {
                            AuthError::Redis(format!("Failed to delete session: {}", e))
                        })?;
                    }
                }
            }
        }

        Ok(())
    }
}


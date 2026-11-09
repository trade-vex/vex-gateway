use crate::error::auth_error::AuthError;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    fn session_index_key(user_id: Uuid) -> String {
        format!("user:{}:sessions", user_id)
    }

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
        let session_index_key = Self::session_index_key(user_id);

        let session_data = SessionData {
            user_id,
            access_token,
            refresh_token,
        };

        let session_json = serde_json::to_string(&session_data)
            .map_err(|e| AuthError::Redis(format!("Failed to serialize session: {}", e)))?;

        let mut conn = self.conn.clone();

        let _: () = redis::pipe()
            .atomic()
            .cmd("SET")
            .arg(&session_key)
            .arg(session_json)
            .arg("EX")
            .arg(self.session_ttl_seconds)
            .ignore()
            .cmd("SADD")
            .arg(&session_index_key)
            .arg(&session_id)
            .ignore()
            .cmd("EXPIRE")
            .arg(&session_index_key)
            .arg(self.session_ttl_seconds)
            .ignore()
            .query_async(&mut conn)
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
        let session_json: Option<String> = conn
            .get(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get session: {}", e)))?;
        let user_id = session_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<SessionData>(json).ok())
            .map(|session| session.user_id);

        let mut pipe = redis::pipe();
        pipe.atomic().cmd("DEL").arg(&session_key).ignore();
        if let Some(user_id) = user_id {
            pipe.cmd("SREM")
                .arg(Self::session_index_key(user_id))
                .arg(session_id)
                .ignore();
        }
        let _: () = pipe
            .query_async(&mut conn)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;

        Ok(())
    }

    /// Refresh/extend a session's TTL
    pub async fn refresh_session(&self, session_id: &str) -> Result<(), AuthError> {
        let session_key = format!("session:{}", session_id);
        let session_data = self.get_session(session_id).await?;
        let session_index_key = Self::session_index_key(session_data.user_id);
        let mut conn = self.conn.clone();

        // Extend the TTL
        let _: () = redis::pipe()
            .atomic()
            .cmd("EXPIRE")
            .arg(&session_key)
            .arg(self.session_ttl_seconds)
            .ignore()
            .cmd("SADD")
            .arg(&session_index_key)
            .arg(session_id)
            .ignore()
            .cmd("EXPIRE")
            .arg(&session_index_key)
            .arg(self.session_ttl_seconds)
            .ignore()
            .query_async(&mut conn)
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
        let session_index_key = Self::session_index_key(session_data.user_id);

        // Update the session with new tokens
        let _: () = redis::pipe()
            .atomic()
            .cmd("SET")
            .arg(&session_key)
            .arg(updated_json)
            .arg("EX")
            .arg(self.session_ttl_seconds)
            .ignore()
            .cmd("SADD")
            .arg(&session_index_key)
            .arg(session_id)
            .ignore()
            .cmd("EXPIRE")
            .arg(&session_index_key)
            .arg(self.session_ttl_seconds)
            .ignore()
            .query_async(&mut conn)
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
        let session_ids = self.user_session_ids(user_id).await?;
        let session_index_key = Self::session_index_key(user_id);
        let keep_session_exists = session_ids.contains(keep_session_id);

        let mut revoked_count = 0;

        for session_id in session_ids {
            if session_id != keep_session_id {
                let (deleted,): (usize,) = redis::pipe()
                    .atomic()
                    .cmd("DEL")
                    .arg(format!("session:{}", session_id))
                    .cmd("SREM")
                    .arg(&session_index_key)
                    .arg(&session_id)
                    .ignore()
                    .query_async(&mut conn)
                    .await
                    .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;
                revoked_count += deleted;
            }
        }

        if keep_session_exists {
            let _: () = redis::pipe()
                .atomic()
                .cmd("SADD")
                .arg(&session_index_key)
                .arg(keep_session_id)
                .ignore()
                .cmd("EXPIRE")
                .arg(&session_index_key)
                .arg(self.session_ttl_seconds)
                .ignore()
                .query_async(&mut conn)
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to keep session indexed: {}", e)))?;
        }

        Ok(revoked_count)
    }

    /// Revoke all sessions for a user
    pub async fn revoke_all_user_sessions(&self, user_id: Uuid) -> Result<usize, AuthError> {
        let mut conn = self.conn.clone();
        let session_ids = self.user_session_ids(user_id).await?;
        let session_index_key = Self::session_index_key(user_id);

        let mut revoked_count = 0;

        for session_id in session_ids {
            let (deleted,): (usize,) = redis::pipe()
                .atomic()
                .cmd("DEL")
                .arg(format!("session:{}", session_id))
                .cmd("SREM")
                .arg(&session_index_key)
                .arg(&session_id)
                .ignore()
                .query_async(&mut conn)
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;
            revoked_count += deleted;
        }

        Ok(revoked_count)
    }

    async fn user_session_ids(&self, user_id: Uuid) -> Result<HashSet<String>, AuthError> {
        let mut conn = self.conn.clone();
        conn.smembers(Self::session_index_key(user_id))
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get user sessions: {}", e)))
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_connection() -> ConnectionManager {
        let client = redis::Client::open("redis://127.0.0.1:6379/13").unwrap();
        ConnectionManager::new(client).await.unwrap()
    }

    #[tokio::test]
    async fn revocation_uses_user_index_and_preserves_other_users_sessions() {
        let conn = test_connection().await;
        let service = SessionService::new(conn.clone(), 60);
        let user_id = Uuid::new_v4();
        let other_user_id = Uuid::new_v4();
        let keep_session = service
            .create_session(user_id, "access-keep".into(), "refresh-keep".into())
            .await
            .unwrap();
        let revoke_session = service
            .create_session(user_id, "access-revoke".into(), "refresh-revoke".into())
            .await
            .unwrap();
        let other_session = service
            .create_session(other_user_id, "access-other".into(), "refresh-other".into())
            .await
            .unwrap();

        let revoked = service
            .revoke_user_sessions_except(user_id, &keep_session)
            .await
            .unwrap();

        assert_eq!(revoked, 1);
        assert!(service.get_session(&keep_session).await.is_ok());
        assert!(service.get_session(&revoke_session).await.is_err());
        assert_eq!(
            service.get_session(&other_session).await.unwrap().user_id,
            other_user_id
        );

        let revoked = service.revoke_all_user_sessions(user_id).await.unwrap();
        assert_eq!(revoked, 1);
        assert!(service.get_session(&keep_session).await.is_err());
        assert!(service.get_session(&other_session).await.is_ok());

        service.delete_session(&other_session).await.unwrap();
    }
}

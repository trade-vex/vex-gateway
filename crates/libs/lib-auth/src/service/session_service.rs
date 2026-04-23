use crate::error::auth_error::AuthError;
use crate::redis_pool::RedisPool;
use dashmap::DashMap;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use uuid::Uuid;

/// Soft cap on the refresh-throttle cache. Enforced probabilistically on
/// insert by sweeping entries that haven't been refreshed within the session
/// TTL (those sessions are either gone from Redis or we haven't seen traffic
/// for them long enough that one extra EXPIRE RTT is acceptable).
const REFRESH_CACHE_SOFT_CAP: usize = 100_000;

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
    pool: RedisPool,
    session_ttl_seconds: u64,
    /// In-proc throttle: session_id -> last-refresh unix timestamp. `refresh_session`
    /// is a no-op while the last refresh is recent enough, which removes one Redis
    /// RTT from every authenticated request in the steady state.
    refresh_cache: Arc<DashMap<String, AtomicI64>>,
    /// Minimum seconds between Redis EXPIRE calls for the same session.
    refresh_throttle_seconds: i64,
}

impl SessionService {
    pub fn new(pool: RedisPool, session_ttl_seconds: i64) -> Self {
        if session_ttl_seconds <= 0 {
            panic!(
                "session_ttl_seconds must be greater than 0, got: {}",
                session_ttl_seconds
            );
        }
        // Refresh at most once every ttl/4, bounded to [30s, 300s]. For a 1800s
        // TTL this works out to 300s.
        let throttle = ((session_ttl_seconds / 4).max(30)).min(300);
        Self {
            pool,
            session_ttl_seconds: session_ttl_seconds as u64,
            refresh_cache: Arc::new(DashMap::new()),
            refresh_throttle_seconds: throttle,
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

        let mut conn = self.pool.get();

        let _: () = conn
            .set_ex(&session_key, session_json, self.session_ttl_seconds)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store session: {}", e)))?;

        // Seed so the first authenticated request doesn't immediately re-EXPIRE
        // a key we just wrote with the full TTL.
        self.refresh_cache
            .insert(session_id.clone(), AtomicI64::new(chrono::Utc::now().timestamp()));

        Ok(session_id)
    }

    /// Get session data by session ID
    pub async fn get_session(&self, session_id: &str) -> Result<SessionData, AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self.pool.get();

        let session_json: Option<String> = conn
            .get(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get session: {}", e)))?;

        let session_json = session_json.ok_or(AuthError::InvalidRequest)?;

        let session_data: SessionData = serde_json::from_str(&session_json)
            .map_err(|e| AuthError::Redis(format!("Failed to deserialize session: {}", e)))?;

        Ok(session_data)
    }

    /// Get session data and extend inactivity TTL in one Redis round-trip.
    pub async fn get_session_and_refresh(
        &self,
        session_id: &str,
    ) -> Result<SessionData, AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self.pool.get();

        let session_json: Option<String> = conn
            .get_ex(&session_key, redis::Expiry::EX(self.session_ttl_seconds))
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get session: {}", e)))?;

        let session_json = session_json.ok_or(AuthError::InvalidRequest)?;

        let session_data: SessionData = serde_json::from_str(&session_json)
            .map_err(|e| AuthError::Redis(format!("Failed to deserialize session: {}", e)))?;

        // GETEX also refreshed the TTL — record so the throttle gate skips.
        self.refresh_cache
            .entry(session_id.to_string())
            .or_insert_with(|| AtomicI64::new(0))
            .store(chrono::Utc::now().timestamp(), Ordering::Relaxed);

        Ok(session_data)
    }

    /// Delete a session
    pub async fn delete_session(&self, session_id: &str) -> Result<(), AuthError> {
        let session_key = format!("session:{}", session_id);

        let mut conn = self.pool.get();

        let _: () = conn
            .del(&session_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;

        self.refresh_cache.remove(session_id);

        Ok(())
    }

    /// Refresh/extend a session's TTL in a single Redis round-trip.
    ///
    /// Throttled: if the same session was refreshed within
    /// `refresh_throttle_seconds`, this returns Ok(()) without touching Redis.
    /// Under sustained load this cuts the auth hot path from 2 Redis RTTs
    /// (blacklist EXISTS + session EXPIRE) down to 1 in the steady state.
    ///
    /// EXPIRE returns 1 if the key existed, 0 if it did not — we return
    /// `AuthError::InvalidRequest` on 0 so callers treat it as idle-expired.
    pub async fn refresh_session(&self, session_id: &str) -> Result<(), AuthError> {
        let now = chrono::Utc::now().timestamp();

        if let Some(entry) = self.refresh_cache.get(session_id) {
            let last = entry.load(Ordering::Relaxed);
            if now - last < self.refresh_throttle_seconds {
                return Ok(());
            }
        }

        let session_key = format!("session:{}", session_id);

        let mut conn = self.pool.get();

        let updated: i64 = conn
            .expire(&session_key, self.session_ttl_seconds as i64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to refresh session: {}", e)))?;

        if updated == 0 {
            self.refresh_cache.remove(session_id);
            return Err(AuthError::InvalidRequest);
        }

        // Before inserting a new tracking entry, cap memory by sweeping
        // entries that are definitely stale (older than the Redis session TTL
        // — those sessions have either idle-expired or been explicitly
        // deleted; in either case Redis is authoritative and dropping our
        // local timestamp just costs one extra EXPIRE on the next request).
        if self.refresh_cache.len() >= REFRESH_CACHE_SOFT_CAP {
            let cutoff = now - self.session_ttl_seconds as i64;
            self.refresh_cache
                .retain(|_, ts| ts.load(Ordering::Relaxed) > cutoff);
        }

        self.refresh_cache
            .entry(session_id.to_string())
            .or_insert_with(|| AtomicI64::new(0))
            .store(now, Ordering::Relaxed);

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

        let mut conn = self.pool.get();

        // Update the session with new tokens
        let _: () = conn
            .set_ex(&session_key, updated_json, self.session_ttl_seconds)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to update session: {}", e)))?;

        self.refresh_cache
            .entry(session_id.to_string())
            .or_insert_with(|| AtomicI64::new(0))
            .store(chrono::Utc::now().timestamp(), Ordering::Relaxed);

        Ok(())
    }

    /// Revoke all sessions for a user except the specified session
    pub async fn revoke_user_sessions_except(
        &self,
        user_id: Uuid,
        keep_session_id: &str,
    ) -> Result<usize, AuthError> {
        let mut conn = self.pool.get();

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
                        let _: () = conn.del(&key).await.map_err(|e| {
                            AuthError::Redis(format!("Failed to delete session: {}", e))
                        })?;
                        self.refresh_cache.remove(session_id);
                        revoked_count += 1;
                    }
                }
            }
        }

        Ok(revoked_count)
    }

    /// Revoke all sessions for a user
    pub async fn revoke_all_user_sessions(&self, user_id: Uuid) -> Result<usize, AuthError> {
        let mut conn = self.pool.get();

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
                    .del(&key)
                    .await
                    .map_err(|e| AuthError::Redis(format!("Failed to delete session: {}", e)))?;
                if let Some(session_id) = key.strip_prefix("session:") {
                    self.refresh_cache.remove(session_id);
                }
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

        let mut conn = self.pool.get();

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

        let mut conn = self.pool.get();

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

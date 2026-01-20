use crate::config::JwtConfig;
use crate::dto::jwt_response_dto::{AppClaims, JwtTokenResponse};
use crate::error::auth_error::AuthError;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use jsonwebtoken::{DecodingKey, Validation, decode};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use std::sync::Arc;
use uuid::Uuid;
#[derive(Clone)]
pub struct TokenService {
    jwt_config: Arc<JwtConfig>,
    conn: ConnectionManager,
    encoding_key: Arc<EncodingKey>,
    decoding_key: Arc<DecodingKey>,
}

impl TokenService {
    pub fn new(
        jwt_config: Arc<JwtConfig>,
        conn: ConnectionManager,
        encoding_key: Arc<EncodingKey>,
        decoding_key: Arc<DecodingKey>,
    ) -> Self {
        Self {
            jwt_config,
            conn,
            encoding_key,
            decoding_key,
        }
    }

    pub async fn mint_tokens(&self, user_id: Uuid) -> Result<JwtTokenResponse, AuthError> {
        // Fix the Redis connection error handling
        let mut conn = self.conn.clone();

        let jti = Uuid::new_v4().to_string();
        let now = Utc::now();

        // Access Token
        let access_exp = now + Duration::minutes(self.jwt_config.access_token_ttl_minutes);
        let access_claims = AppClaims {
            sub: user_id.to_string(),
            jti: jti.clone(),
            iat: now.timestamp(),
            exp: access_exp.timestamp(),
            iss: self.jwt_config.issuer.clone(),
            aud: self.jwt_config.audience.clone(),
            pwd: now.timestamp(), // Use current time as password timestamp
        };

        let access_token = encode(
            &Header::new(Algorithm::RS256),
            &access_claims,
            &self.encoding_key,
        )
        .map_err(|e| AuthError::Validation(format!("JWT validation failed: {}", e)))?;

        // Refresh Token
        let refresh_token = Uuid::new_v4().to_string();
        let refresh_ttl_secs = self.jwt_config.refresh_token_ttl_days * 24 * 3600;
        let redis_key = format!("refresh:{}", refresh_token);
        let redis_val = format!("{}:{}", user_id, jti);

        // Convert to u64 using fallible conversion to handle overflow
        let refresh_ttl_secs_u64 = refresh_ttl_secs
            .try_into()
            .map_err(|_| AuthError::Validation("Refresh token TTL is too large".to_string()))?;

        let _: () = conn
            .set_ex(redis_key, redis_val, refresh_ttl_secs_u64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store refresh token: {}", e)))?;

        let expires_in = (self.jwt_config.access_token_ttl_minutes * 60)
            .try_into()
            .map_err(|_| AuthError::Validation("Access token TTL is too large".to_string()))?;

        Ok(JwtTokenResponse {
            access_token,
            refresh_token,
            expires_in,
        })
    }

    // Fixed refresh_tokens method
    pub async fn refresh_tokens(
        &self,
        old_refresh_token: &str,
    ) -> Result<JwtTokenResponse, AuthError> {
        let mut conn = self.conn.clone();

        let redis_key = format!("refresh:{}", old_refresh_token);

        // Get and delete the refresh token in one operation
        let entry: Option<String> = conn
            .get_del(&redis_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get refresh token: {}", e)))?;

        if let Some(val) = entry {
            let parts: Vec<&str> = val.split(':').collect();
            if parts.len() != 2 {
                return Err(AuthError::InvalidResetToken);
            }

            let user_id = Uuid::parse_str(parts[0]).map_err(|_| AuthError::InvalidResetToken)?;
            let old_jti = parts[1];

            // Blacklist the old access token's JTI
            let blacklist_key = format!("blacklist:{}", old_jti);
            let blacklist_ttl = self.jwt_config.access_token_ttl_minutes * 60;
            let blacklist_ttl_u64 = blacklist_ttl
                .try_into()
                .map_err(|_| AuthError::Validation("Blacklist TTL is too large".to_string()))?;
            let _: () = conn
                .set_ex(blacklist_key, "1", blacklist_ttl_u64)
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to blacklist token: {}", e)))?;

            // Mint a fresh pair of tokens
            self.mint_tokens(user_id).await
        } else {
            Err(AuthError::InvalidResetToken)
        }
    }

    // Validate JWT token
    pub fn validate_jwt(&self, token: &str) -> Result<AppClaims, AuthError> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.jwt_config.issuer]);
        validation.set_audience(&[&self.jwt_config.audience]);

        decode::<AppClaims>(token, &self.decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|e| AuthError::Validation(format!("JWT validation failed: {}", e)))
    }

    // Check if a token is blacklisted
    pub async fn is_token_blacklisted(&self, jti: &str) -> Result<bool, AuthError> {
        let mut conn = self.conn.clone();

        let blacklist_key = format!("blacklist:{}", jti);
        let exists: bool = conn
            .exists(&blacklist_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to check blacklist: {}", e)))?;

        Ok(exists)
    }

    // Blacklist a token (for logout)
    pub async fn blacklist_token(&self, token: &str) -> Result<(), AuthError> {
        // First validate the token to get the JTI and expiration
        let claims = self.validate_jwt(token)?;

        let mut conn = self.conn.clone();

        // Calculate TTL until token expires
        let current_time = Utc::now().timestamp();
        let ttl = if claims.exp > current_time {
            claims.exp - current_time
        } else {
            return Ok(()); // Token already expired, no need to blacklist
        };

        let blacklist_key = format!("blacklist:{}", claims.jti);
        let ttl_u64 = ttl
            .try_into()
            .map_err(|_| AuthError::Validation("Token TTL is too large".to_string()))?;
        let _: () = conn
            .set_ex(blacklist_key, "1", ttl_u64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to blacklist token: {}", e)))?;

        Ok(())
    }

    // Blacklist a JTI with specific TTL
    pub async fn blacklist_jti(&self, jti: &str, ttl_secs: u64) -> Result<(), AuthError> {
        let mut conn = self.conn.clone();

        let blacklist_key = format!("blacklist:{}", jti);
        let _: () = conn
            .set_ex(blacklist_key, "1", ttl_secs)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to blacklist JTI: {}", e)))?;

        Ok(())
    }

    // Mint tokens with specific password timestamp (for session invalidation)
    pub async fn mint_tokens_with_password_time(
        &self,
        user_id: Uuid,
        password_timestamp: i64,
    ) -> Result<JwtTokenResponse, AuthError> {
        let mut conn = self.conn.clone();

        let jti = Uuid::new_v4().to_string();
        let now = Utc::now();

        // Access Token
        let access_exp = now + Duration::minutes(self.jwt_config.access_token_ttl_minutes);
        let access_claims = AppClaims {
            sub: user_id.to_string(),
            jti: jti.clone(),
            iat: now.timestamp(),
            exp: access_exp.timestamp(),
            iss: self.jwt_config.issuer.clone(),
            aud: self.jwt_config.audience.clone(),
            pwd: password_timestamp,
        };

        let access_token = encode(
            &Header::new(Algorithm::RS256),
            &access_claims,
            &self.encoding_key,
        )
        .map_err(|e| AuthError::Validation(format!("JWT validation failed: {}", e)))?;

        // Refresh Token
        let refresh_token = Uuid::new_v4().to_string();
        let refresh_ttl_secs = self.jwt_config.refresh_token_ttl_days * 24 * 3600;
        let redis_key = format!("refresh:{}", refresh_token);
        let redis_val = format!("{}:{}", user_id, jti);

        // Convert to u64 using fallible conversion to handle overflow
        let refresh_ttl_secs_u64 = refresh_ttl_secs
            .try_into()
            .map_err(|_| AuthError::Validation("Refresh token TTL is too large".to_string()))?;

        let _: () = conn
            .set_ex(redis_key, redis_val, refresh_ttl_secs_u64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store refresh token: {}", e)))?;

        let expires_in = (self.jwt_config.access_token_ttl_minutes * 60)
            .try_into()
            .map_err(|_| AuthError::Validation("Access token TTL is too large".to_string()))?;

        Ok(JwtTokenResponse {
            access_token,
            refresh_token,
            expires_in,
        })
    }

    // Revoke all sessions for a user by blacklisting all refresh tokens
    pub async fn revoke_user_sessions(&self, user_id: Uuid) -> Result<(), AuthError> {
        let mut conn = self.conn.clone();

        // Find all refresh tokens for this user
        let pattern = "refresh:*".to_string();
        let keys: Vec<String> = conn
            .keys(pattern)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to find refresh tokens: {}", e)))?;

        // Check each refresh token and delete if it belongs to this user
        for key in keys {
            if let Ok(Some(val)) = conn.get::<String, Option<String>>(key.clone()).await {
                let parts: Vec<&str> = val.split(':').collect();
                if parts.len() == 2 && parts[0] == user_id.to_string() {
                    // Delete the refresh token
                    let _: () = conn.del(key).await.map_err(|e| {
                        AuthError::Redis(format!("Failed to delete refresh token: {}", e))
                    })?;

                    // Blacklist the associated JTI
                    let jti = parts[1];
                    let blacklist_key = format!("blacklist:{}", jti);
                    let blacklist_ttl = self.jwt_config.access_token_ttl_minutes * 60;
                    let blacklist_ttl_u64 = blacklist_ttl.try_into().map_err(|_| {
                        AuthError::Validation("Blacklist TTL is too large".to_string())
                    })?;
                    let _: () = conn
                        .set_ex(blacklist_key, "1", blacklist_ttl_u64)
                        .await
                        .map_err(|e| AuthError::Redis(format!("Failed to blacklist JTI: {}", e)))?;
                }
            }
        }

        Ok(())
    }
}

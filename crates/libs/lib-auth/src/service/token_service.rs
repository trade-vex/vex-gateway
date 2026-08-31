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

pub(crate) fn token_version_key(user_id: Uuid) -> String {
    format!("token_version:{}", user_id)
}

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
        let token_version = self.get_token_version(user_id).await?;
        self.mint_tokens_at_version(user_id, token_version).await
    }

    pub async fn mint_tokens_at_version(
        &self,
        user_id: Uuid,
        token_version: u64,
    ) -> Result<JwtTokenResponse, AuthError> {
        if self.get_token_version(user_id).await? != token_version {
            return Err(AuthError::TokenRevoked);
        }

        self.mint_tokens_at_version_with_password_time(
            user_id,
            Utc::now().timestamp(),
            token_version,
        )
        .await
    }

    async fn mint_tokens_at_version_with_password_time(
        &self,
        user_id: Uuid,
        password_timestamp: i64,
        token_version: u64,
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
            token_version,
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
        let redis_val = format!("{}:{}:{}", user_id, jti, token_version);

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
            let [user_id, old_jti, token_version] = parts[..] else {
                return Err(AuthError::InvalidResetToken);
            };

            let user_id = Uuid::parse_str(user_id).map_err(|_| AuthError::InvalidResetToken)?;
            let token_version = token_version
                .parse::<u64>()
                .map_err(|_| AuthError::InvalidResetToken)?;

            if token_version < self.get_token_version(user_id).await? {
                return Err(AuthError::TokenRevoked);
            }

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
            self.mint_tokens_at_version(user_id, token_version).await
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

    pub async fn get_token_version(&self, user_id: Uuid) -> Result<u64, AuthError> {
        let mut conn = self.conn.clone();
        let version: Option<u64> = conn
            .get(token_version_key(user_id))
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get token version: {}", e)))?;

        Ok(version.unwrap_or_default())
    }

    pub async fn bump_token_version(&self, user_id: Uuid) -> Result<u64, AuthError> {
        let mut conn = self.conn.clone();
        conn.incr(token_version_key(user_id), 1_u64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to bump token version: {}", e)))
    }

    // Mint tokens with specific password timestamp (for session invalidation)
    pub async fn mint_tokens_with_password_time(
        &self,
        user_id: Uuid,
        password_timestamp: i64,
    ) -> Result<JwtTokenResponse, AuthError> {
        let token_version = self.get_token_version(user_id).await?;
        self.mint_tokens_at_version_with_password_time(user_id, password_timestamp, token_version)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_redis_conn() -> ConnectionManager {
        let client = redis::Client::open("redis://127.0.0.1:6379/")
            .expect("Failed to create test Redis client");
        ConnectionManager::new(client)
            .await
            .expect("Failed to create test Redis connection manager")
    }

    fn test_service(conn: ConnectionManager) -> TokenService {
        TokenService::new(
            Arc::new(JwtConfig {
                private_key_path: String::new(),
                public_key_path: String::new(),
                access_token_ttl_minutes: 60,
                refresh_token_ttl_days: 30,
                issuer: "vex-gateway".to_string(),
                audience: "vex-api".to_string(),
            }),
            conn,
            Arc::new(EncodingKey::from_secret(b"unused")),
            Arc::new(DecodingKey::from_secret(b"unused")),
        )
    }

    #[tokio::test]
    async fn minting_at_captured_login_version_fails_after_revocation() {
        let user_id = Uuid::new_v4();
        let service = test_service(create_test_redis_conn().await);

        let captured_version = service.get_token_version(user_id).await.unwrap();
        service.bump_token_version(user_id).await.unwrap();

        let result = service
            .mint_tokens_at_version(user_id, captured_version)
            .await;
        assert!(matches!(result, Err(AuthError::TokenRevoked)));
    }

    #[tokio::test]
    async fn refresh_record_without_a_version_is_rejected() {
        let mut conn = create_test_redis_conn().await;
        let service = test_service(conn.clone());
        let refresh_token = Uuid::new_v4().to_string();
        let jti = Uuid::new_v4().to_string();
        let _: () = conn
            .set_ex(
                format!("refresh:{}", refresh_token),
                format!("{}:{}", Uuid::new_v4(), jti),
                60_u64,
            )
            .await
            .unwrap();

        let result = service.refresh_tokens(&refresh_token).await;

        assert!(matches!(result, Err(AuthError::InvalidResetToken)));
        let blacklisted: bool = conn.exists(format!("blacklist:{}", jti)).await.unwrap();
        assert!(!blacklisted, "a rejected record must not be acted on");
    }
}

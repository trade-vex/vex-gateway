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

pub(crate) fn blacklist_key(jti: &str) -> String {
    format!("blacklist:{}", jti)
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

        let now = Utc::now();

        // Access Token
        let access_exp = now + Duration::minutes(self.jwt_config.access_token_ttl_minutes);
        let access_claims = AppClaims {
            sub: user_id.to_string(),
            jti: Uuid::new_v4().to_string(),
            iat: now.timestamp(),
            exp: access_exp.timestamp(),
            iss: self.jwt_config.issuer.clone(),
            aud: self.jwt_config.audience.clone(),
            token_version,
        };

        let access_token = encode(
            &Header::new(Algorithm::RS256),
            &access_claims,
            &self.encoding_key,
        )
        .map_err(|e| AuthError::InternalError(format!("JWT encoding failed: {}", e)))?;

        let expires_in = (self.jwt_config.access_token_ttl_minutes * 60)
            .try_into()
            .map_err(|_| AuthError::Validation("Access token TTL is too large".to_string()))?;

        Ok(JwtTokenResponse {
            access_token,
            expires_in,
        })
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

        let ttl_u64 = ttl
            .try_into()
            .map_err(|_| AuthError::Validation("Token TTL is too large".to_string()))?;
        let _: () = conn
            .set_ex(blacklist_key(&claims.jti), "1", ttl_u64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to blacklist token: {}", e)))?;

        Ok(())
    }

    // Blacklist a JTI with specific TTL
    pub async fn blacklist_jti(&self, jti: &str, ttl_secs: u64) -> Result<(), AuthError> {
        let mut conn = self.conn.clone();

        let _: () = conn
            .set_ex(blacklist_key(jti), "1", ttl_secs)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_service(conn: ConnectionManager) -> TokenService {
        TokenService::new(
            Arc::new(JwtConfig {
                private_key_path: String::new(),
                public_key_path: String::new(),
                access_token_ttl_minutes: 60,
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
        let service = test_service(crate::test_redis().await);

        let captured_version = service.get_token_version(user_id).await.unwrap();
        service.bump_token_version(user_id).await.unwrap();

        let result = service
            .mint_tokens_at_version(user_id, captured_version)
            .await;
        assert!(matches!(result, Err(AuthError::TokenRevoked)));
    }
}

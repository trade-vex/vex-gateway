use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct LoginRequest {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8))]
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub user: UserInfo,
}

#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub user_id: Uuid,
    pub email: String,
    pub username: String,
    pub auth_provider: String,
    pub email_verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct RefreshTokenRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
pub struct RefreshTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Debug, Serialize)]
pub struct UserProfile {
    pub user_id: Uuid,
    pub email: String,
    pub username: String,
    pub avatar_url: Option<String>,
    pub auth_provider: String,
    pub email_verified: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_login: Option<chrono::DateTime<chrono::Utc>>,
}

/// The response sent to the client after a successful login or token refresh.
#[derive(Serialize, Deserialize, Debug)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64, // TTL in seconds for the access token
}

/// The claims embedded within our application's JWT.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppClaims {
    // Standard claims
    pub sub: String, // Subject (our internal user ID)
    pub exp: i64,    // Expiration time
    pub iat: i64,    // Issued at
    pub iss: String, // Issuer (our app)
    pub aud: String, // Audience (the intended recipient, e.g., "api")

    // Custom claims
    pub jti: String, // JWT ID (for revocation)
}

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub user: UserInfo,
}

/// The response sent to the client after a successful login or token refresh.
#[derive(Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64, // TTL in seconds for the access token
}

#[derive(Serialize)]
pub struct RefreshTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Serialize)]
pub struct SignupResponse {
    pub user: UserInfo,
    pub message: String,
}

#[derive(Serialize)]
pub struct OAuthLoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub user: UserInfo,
}

#[derive(Serialize, Deserialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<u64>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
}

#[derive(Serialize)]
pub struct UserInfo {
    pub user_id: Uuid,
    pub email: String,
    pub username: String,
    pub avatar_url: Option<String>,
    pub auth_provider: String,
    pub email_verified: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
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

#[derive(Clone)]
pub struct OAuthUserInfo {
    pub provider_id: String,
    pub email: String,
    pub name: String,
    pub username: Option<String>,
    pub avatar_url: Option<String>,
}

// Conversion from User model to UserInfo DTO
impl From<crate::models::user::Model> for UserInfo {
    fn from(user: crate::models::user::Model) -> Self {
        Self {
            user_id: user.user_id,
            email: user.email,
            username: user.username,
            avatar_url: user.avatar_url,
            auth_provider: user.auth_provider,
            email_verified: user.email_verified,
            created_at: user.created_at,
        }
    }
}

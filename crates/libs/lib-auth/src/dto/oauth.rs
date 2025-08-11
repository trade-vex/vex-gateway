use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: String,
    pub state: String,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<u64>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OAuthUserInfo {
    pub provider_id: String,
    pub email: String,
    pub name: String,
    pub username: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OAuthLoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub user: UserInfo,
}

#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub user_id: Uuid,
    pub email: String,
    pub username: String,
    pub avatar_url: Option<String>,
    pub auth_provider: String,
    pub email_verified: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// Traditional auth DTOs
#[derive(Debug, Deserialize)]
pub struct SignupRequest {
    pub email: String,
    pub username: String,
    pub password: String,
    pub invite_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub user: UserInfo,
}

#[derive(Debug, Deserialize)]
pub struct EmailVerificationRequest {
    pub email: String,
    pub verification_code: String,
}

#[derive(Debug, Deserialize)]
pub struct PasswordResetRequest {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct SetNewPasswordRequest {
    pub reset_token: String,
    pub new_password: String,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

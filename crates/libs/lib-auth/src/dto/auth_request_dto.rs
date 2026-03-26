use serde::Deserialize;
use validator::Validate;

#[derive(Deserialize, Validate)]
pub struct LoginRequest {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8))]
    pub password: String,
}

#[derive(Deserialize)]
pub struct RefreshTokenRequest {
    pub refresh_token: String,
}

#[derive(Deserialize, Validate)]
pub struct SignupRequest {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 3, max = 100))]
    pub username: String,
    #[validate(length(min = 8))]
    pub password: String,
}

#[derive(Deserialize, Validate)]
pub struct ChangePasswordRequest {
    #[validate(length(min = 8))]
    pub old_password: String,
    #[validate(length(min = 8))]
    pub new_password: String,
}

#[derive(Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Deserialize, Validate)]
pub struct VerifyEmailRequest {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 6, max = 6))]
    pub otp: String,
}

#[derive(Deserialize, Validate)]
pub struct CheckEmailRequest {
    #[validate(email)]
    pub email: String,
}

#[derive(Deserialize, Validate)]
pub struct ForgotPasswordRequest {
    #[validate(email)]
    pub email: String,
}

#[derive(Deserialize, Validate)]
pub struct VerifyResetOtpRequest {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 6, max = 6))]
    pub otp: String,
}

#[derive(Deserialize, Validate)]
pub struct ResetPasswordRequest {
    pub reset_token: String,
    #[validate(length(min = 8))]
    pub new_password: String,
}

/// Query parameters for OAuth initiation
#[derive(Debug, Clone, Deserialize)]
pub struct OAuthInitQuery {
    /// Platform identifier: "web" or "app"
    pub platform: Option<String>,
    /// Required when platform="app" - SHA256 hash of the app's verifier (PKCE-like)
    pub app_challenge: Option<String>,
}

/// Request body for mobile OAuth login code exchange
#[derive(Debug, Clone, Deserialize)]
pub struct OAuthExchangeRequest {
    /// One-time login code received via deep link
    pub login_code: String,
    /// The original verifier that was hashed to create app_challenge
    pub verifier: String,
}

/// Request body for session renewal using a rotated refresh token
#[derive(Debug, Clone, Deserialize)]
pub struct SessionRenewRequest {
    /// Rotating refresh token issued during login, verification, or renewal.
    /// Optional so web clients can rely on the refresh_token cookie instead.
    pub refresh_token: Option<String>,
}

use actix_web::{HttpResponse, ResponseError};
use serde_json::json;

#[derive(Debug, thiserror::Error)]

pub enum AuthError {
    // Existing errors
    #[error("Token is revoked")]
    TokenRevoked,

    #[error("Token has expired")]
    TokenExpired,

    #[error("Missing authorization header")]
    MissingAuthHeader,

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("User not found")]
    UserNotFound,

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    InternalError(String),

    // New OAuth-specific errors
    #[error("Invalid OAuth provider: {0}")]
    InvalidProvider(String),

    #[error("OAuth state mismatch")]
    StateMismatch,

    #[error("OAuth code exchange failed: {0}")]
    CodeExchangeFailed(String),

    #[error("Failed to fetch user info: {0}")]
    UserInfoFetchFailed(String),

    #[error("Invalid OAuth callback")]
    InvalidCallback,

    #[error("OAuth provider error: {0}")]
    ProviderError(String),

    // Traditional auth errors
    #[error("Email already exists")]
    EmailAlreadyExists,

    #[error("Invalid password")]
    InvalidPassword,

    #[error("Email not verified")]
    EmailNotVerified,

    #[error("Invalid verification code")]
    InvalidVerificationCode,

    #[error("Verification code expired")]
    VerificationCodeExpired,

    #[error("Password reset token invalid")]
    InvalidResetToken,

    #[error("Password reset token expired")]
    ResetTokenExpired,

    // Database errors
    #[error("Database error: {0}")]
    Database(String),

    // Redis errors
    #[error("Redis error: {0}")]
    Redis(String),

    // Validation errors
    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Invalid request")]
    InvalidRequest,
}

impl ResponseError for AuthError {
    fn error_response(&self) -> HttpResponse {
        let (status_code, error_code, message) = match self {
            AuthError::TokenRevoked | AuthError::TokenExpired => {
                (401, "TOKEN_INVALID", self.to_string())
            }
            AuthError::MissingAuthHeader => (
                401,
                "MISSING_AUTH_HEADER",
                "Authorization header is required".to_string(),
            ),
            AuthError::InvalidCredentials => (
                401,
                "INVALID_CREDENTIALS",
                "Invalid email or password".to_string(),
            ),
            AuthError::UserNotFound => (404, "USER_NOT_FOUND", "User not found".to_string()),
            AuthError::InvalidProvider(_) => (400, "INVALID_PROVIDER", self.to_string()),
            AuthError::StateMismatch => (
                400,
                "STATE_MISMATCH",
                "OAuth state parameter mismatch".to_string(),
            ),
            AuthError::CodeExchangeFailed(_) => (
                400,
                "CODE_EXCHANGE_FAILED",
                "Failed to exchange OAuth code for token".to_string(),
            ),
            AuthError::UserInfoFetchFailed(_) => (
                400,
                "USER_INFO_FAILED",
                "Failed to fetch user information from provider".to_string(),
            ),
            AuthError::InvalidCallback => (
                400,
                "INVALID_CALLBACK",
                "Invalid OAuth callback parameters".to_string(),
            ),
            AuthError::ProviderError(_) => (
                400,
                "PROVIDER_ERROR",
                "Upstream provider error".to_string()
            ),
            AuthError::EmailAlreadyExists => (
                409,
                "EMAIL_EXISTS",
                "Email address is already registered".to_string(),
            ),
            AuthError::InvalidPassword => (
                400,
                "INVALID_PASSWORD",
                "Password does not meet requirements".to_string(),
            ),
            AuthError::EmailNotVerified => (
                403,
                "EMAIL_NOT_VERIFIED",
                "Email address not verified".to_string(),
            ),
            AuthError::InvalidVerificationCode => (
                400,
                "INVALID_VERIFICATION_CODE",
                "Invalid verification code".to_string(),
            ),
            AuthError::VerificationCodeExpired => (
                400,
                "VERIFICATION_CODE_EXPIRED",
                "Verification code has expired".to_string(),
            ),
            AuthError::InvalidResetToken => (
                400,
                "INVALID_RESET_TOKEN",
                "Invalid password reset token".to_string(),
            ),
            AuthError::ResetTokenExpired => (
                400,
                "RESET_TOKEN_EXPIRED",
                "Password reset token has expired".to_string(),
            ),
            AuthError::Database(_) => (
                500,
                "DATABASE_ERROR",
                "Database operation failed".to_string(),
            ),
            AuthError::Redis(_) => (500, "REDIS_ERROR", "Redis operation failed".to_string()),
            AuthError::Validation(_) => (400, "VALIDATION_ERROR", self.to_string()),
            AuthError::InvalidRequest => {
                (400, "INVALID_REQUEST", "Invalid request format".to_string())
            }
            AuthError::Config(_) | AuthError::InternalError(_) => {
                (500, "INTERNAL_ERROR", "Internal server error".to_string())
            }
        };

        HttpResponse::build(actix_web::http::StatusCode::from_u16(status_code).unwrap()).json(
            json!({
                "error": {
                    "code": error_code,
                    "message": message
                }
            }),
        )
    }
}

// Conversion implementations
impl From<sea_orm::DbErr> for AuthError {
    fn from(error: sea_orm::DbErr) -> Self {
        AuthError::Database(error.to_string())
    }
}

impl From<deadpool_redis::redis::RedisError> for AuthError {
    fn from(error: deadpool_redis::redis::RedisError) -> Self {
        AuthError::Redis(error.to_string())
    }
}

impl From<reqwest::Error> for AuthError {
    fn from(error: reqwest::Error) -> Self {
        AuthError::InternalError(format!("HTTP request failed: {}", error))
    }
}

impl From<serde_json::Error> for AuthError {
    fn from(error: serde_json::Error) -> Self {
        AuthError::InternalError(format!("JSON parsing failed: {}", error))
    }
}

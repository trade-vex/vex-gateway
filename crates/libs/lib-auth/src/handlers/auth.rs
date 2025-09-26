use actix_web::{HttpRequest, HttpResponse, web};
use serde_json::json;

use crate::dto::auth_request_dto::{LoginRequest, RefreshTokenRequest};
use crate::dto::auth_response_dto::{RefreshTokenResponse, UserProfile};
use crate::error::auth_error::AuthError;
use crate::middleware::jwt_auth::AuthenticatedUser;
use crate::service::token_service::TokenService;
use crate::state::AppState;
use std::sync::Arc;

// Traditional login (returns error since we only support OAuth)
pub async fn login(_req: web::Json<LoginRequest>) -> Result<HttpResponse, AuthError> {
    Ok(HttpResponse::BadRequest().json(json!({
        "error": {
            "code": "not_supported",
            "message": "Traditional login not supported. Please use OAuth with Google or GitHub.",
            "status": 400,
            "oauth_providers": [
                {
                    "name": "google",
                    "url": "/auth/google"
                },
                {
                    "name": "github",
                    "url": "/auth/github"
                }
            ]
        }
    })))
}

// Logout - blacklist the JWT token
pub async fn logout(
    req: HttpRequest,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    // Extract JWT token from Authorization header
    let token = match req.headers().get("Authorization") {
        Some(auth_header) => {
            let auth_str = auth_header
                .to_str()
                .map_err(|_| AuthError::InvalidRequest)?;
            if let Some(stripped) = auth_str.strip_prefix("Bearer ") {
                stripped.to_string()
            } else {
                return Err(AuthError::InvalidRequest);
            }
        }
        None => return Err(AuthError::InvalidRequest),
    };

    // Blacklist the token
    token_service.blacklist_token(&token).await?;

    Ok(HttpResponse::Ok().json(json!({
        "message": "Logged out successfully",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

pub async fn get_profile(
    user: AuthenticatedUser,
    app_state: web::Data<Arc<AppState>>,
) -> Result<HttpResponse, AuthError> {
    tracing::info!("Get profile request for user: {}", user.user_id);

    // This will now work with the new find_by_id method
    let user_record = app_state
        .user_repo
        .find_by_id(user.user_id) // Method now exists
        .await?
        .ok_or(AuthError::UserNotFound)?;

    let profile = UserProfile {
        user_id: user_record.user_id,
        email: user_record.email,
        username: user_record.username,
        avatar_url: user_record.avatar_url,
        auth_provider: user_record.auth_provider,
        email_verified: user_record.email_verified,
        created_at: user_record.created_at,
        last_login: user_record.last_login,
    };

    Ok(HttpResponse::Ok().json(profile))
}

pub async fn refresh_token(
    req: web::Json<RefreshTokenRequest>,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    tracing::info!("Token refresh request");

    let new_tokens = token_service.refresh_tokens(&req.refresh_token).await?;

    let response = RefreshTokenResponse {
        access_token: new_tokens.access_token,
        token_type: "Bearer".to_string(),
        expires_in: new_tokens.expires_in,
    };

    Ok(HttpResponse::Ok().json(response))
}

// Add this function at the end of the file
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/login", web::post().to(login))
        .route("/logout", web::post().to(logout))
        .route("/refresh", web::post().to(refresh_token))
        .route("/profile", web::get().to(get_profile));
}

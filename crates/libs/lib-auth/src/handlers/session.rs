use crate::error::auth_error::AuthError;
use crate::service::session_service::SessionService;
use crate::service::token_service::TokenService;
use actix_web::{HttpRequest, HttpResponse, web};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct SessionInfo {
    pub user_id: uuid::Uuid,
    pub access_token: String,
    pub refresh_token: String,
    pub session_expires_in: u64,
    pub refresh_expires_in: u64,
}

/// Get session info (tokens and user_id)
/// GET /api/session/info
pub async fn get_session_info(
    req: HttpRequest,
    session_service: web::Data<Arc<SessionService>>,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    // Extract session ID from cookie
    let session_id = req
        .cookie("session_id")
        .ok_or(AuthError::InvalidRequest)?
        .value()
        .to_string();

    // Get session data from Redis
    let session_data = session_service.get_session(&session_id).await?;

    // Return tokens and user_id
    let response = SessionInfo {
        user_id: session_data.user_id,
        access_token: session_data.access_token,
        refresh_token: session_data.refresh_token,
        session_expires_in: session_service.ttl_seconds(),
        refresh_expires_in: token_service.refresh_token_ttl_seconds()?,
    };

    Ok(HttpResponse::Ok().json(response))
}

/// Refresh session TTL
/// POST /api/session/refresh
pub async fn refresh_session(
    req: HttpRequest,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    // Extract session ID from cookie
    let session_id = req
        .cookie("session_id")
        .ok_or(AuthError::InvalidRequest)?
        .value()
        .to_string();

    // Refresh the session TTL
    session_service.refresh_session(&session_id).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "message": "Session refreshed successfully"
    })))
}

/// Refresh tokens and update session
/// POST /api/session/refresh-tokens
pub async fn refresh_session_tokens(
    req: HttpRequest,
    session_service: web::Data<Arc<SessionService>>,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    let session_id = req
        .cookie("session_id")
        .ok_or(AuthError::InvalidRequest)?
        .value()
        .to_string();

    let session_data = session_service.get_session(&session_id).await?;

    let new_tokens = token_service
        .refresh_tokens(&session_data.refresh_token)
        .await?;

    session_service
        .update_session_tokens(
            &session_id,
            new_tokens.access_token.clone(),
            new_tokens.refresh_token.clone(),
        )
        .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "message": "Token refreshed successfully"
    })))
}

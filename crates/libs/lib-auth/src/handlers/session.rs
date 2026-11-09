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
}

/// Get session info (access token and non-sensitive metadata)
/// GET /api/session/info
pub async fn get_session_info(
    req: HttpRequest,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    // Extract session ID from cookie
    let session_id = req
        .cookie("session_id")
        .ok_or(AuthError::InvalidRequest)?
        .value()
        .to_string();

    // Get session data from Redis
    let session_data = session_service.get_session(&session_id).await?;

    // Return the access token handoff and non-sensitive session metadata
    let response = SessionInfo {
        user_id: session_data.user_id,
        access_token: session_data.access_token,
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

#[cfg(test)]
mod tests {
    use super::SessionInfo;

    #[test]
    fn session_info_omits_refresh_token() {
        let response = SessionInfo {
            user_id: uuid::Uuid::nil(),
            access_token: "access-token".to_string(),
        };

        let json = serde_json::to_value(response).expect("SessionInfo should serialize");

        assert_eq!(json["access_token"], "access-token");
        assert!(json.get("refresh_token").is_none());
    }
}

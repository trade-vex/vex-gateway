use crate::error::auth_error::AuthError;
use crate::service::session_service::SessionService;
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

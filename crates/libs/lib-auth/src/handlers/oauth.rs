use crate::dto::auth_request_dto::OAuthCallbackQuery;
use crate::error::auth_error::AuthError;
use crate::service::{oauth_service::OAuthService, session_service::SessionService};

use actix_web::{
    HttpResponse,
    cookie::{Cookie, SameSite, time::Duration},
    web,
};
use serde_json;
use std::sync::Arc;
use tracing::error;

/// Initiate OAuth flow
/// GET /oauth/{provider}
pub async fn initiate_oauth(
    provider: web::Path<String>,
    oauth_service: web::Data<Arc<OAuthService>>,
) -> Result<HttpResponse, AuthError> {
    match oauth_service.get_auth_url(&provider).await {
        Ok(url) => Ok(HttpResponse::Found()
            .append_header(("Location", url))
            .finish()),
        Err(e) => {
            error!("Failed to initiate OAuth: {}", e);
            Err(e)
        }
    }
}

/// Handle OAuth callback
/// GET /oauth/{provider}/callback?code=...&state=...
pub async fn handle_oauth_callback(
    provider: web::Path<String>,
    query: web::Query<OAuthCallbackQuery>,
    oauth_service: web::Data<Arc<OAuthService>>,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate that the OAuth code is present and non-empty
    let code = query
        .code
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or(AuthError::InvalidRequest)?;

    match oauth_service
        .handle_callback(&provider, code, &query.state)
        .await
    {
        Ok(response) => {
            let is_secure = oauth_service.get_frontend_url().starts_with("https://");

            // Get user_id and user_info from the response
            let user_id = response.user.user_id;
            let user_info = response.user.clone();

            // Create session with user_id and tokens
            let session_id = session_service
                .create_session(
                    user_id,
                    response.access_token.clone(),
                    response.refresh_token.clone(),
                )
                .await
                .map_err(|e| {
                    AuthError::CodeExchangeFailed(format!("Failed to create session: {}", e))
                })?;

            // Create session cookie
            let session_cookie = Cookie::build("session_id", session_id)
                .path("/")
                .http_only(true)
                .secure(is_secure)
                .same_site(SameSite::Strict)
                .max_age(Duration::seconds(response.expires_in as i64))
                .finish();

            // Create user_info cookie (JSON format)
            let user_info_json = serde_json::to_string(&user_info).map_err(|e| {
                AuthError::Validation(format!("Failed to serialize user info: {}", e))
            })?;

            let user_info_cookie = Cookie::build("user_info", user_info_json)
                .path("/")
                .http_only(false) // Allow client-side JavaScript access
                .secure(is_secure)
                .same_site(SameSite::Strict)
                .max_age(Duration::seconds(response.expires_in as i64))
                .finish();

            Ok(HttpResponse::Found()
                .cookie(session_cookie)
                .cookie(user_info_cookie)
                .append_header(("Location", oauth_service.get_frontend_url()))
                .finish())
        }
        Err(e) => {
            error!("OAuth callback error: {}", e);
            Err(e)
        }
    }
}

/// Get list of available OAuth providers
/// GET /oauth/providers
pub async fn get_oauth_providers(
    oauth_service: web::Data<Arc<OAuthService>>,
) -> Result<HttpResponse, AuthError> {
    let providers: Vec<String> = oauth_service.get_available_providers().await;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "providers": providers
    })))
}

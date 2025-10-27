use crate::dto::auth_request_dto::OAuthCallbackQuery;
use crate::error::auth_error::AuthError;
use crate::service::{oauth_service::OAuthService, session_service::SessionService};

use actix_web::{HttpResponse, web, cookie::{Cookie, SameSite, time::Duration}};
use std::sync::Arc;
use tracing::error;
use serde_json;

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
            // Create session with the OAuth response data
            let session_id = session_service
                .create_session(response.user.clone(), response.access_token, response.refresh_token)
                .await?;

            let is_secure = oauth_service.get_frontend_url().starts_with("https://");
            
            // Create session cookie
            let session_cookie = Cookie::build("session_id", session_id)
                .path("/")
                .http_only(true)
                .secure(is_secure)
                .same_site(SameSite::Strict)
                .max_age(Duration::days(30))
                .finish();

            // Redirect to the portfolio page with session cookie
            let redirect_url = format!("{}/portfolio", oauth_service.get_frontend_url());
            Ok(HttpResponse::Found()
                .cookie(session_cookie)
                .append_header(("Location", redirect_url))
                .finish())
        },
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

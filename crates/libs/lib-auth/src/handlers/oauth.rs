use crate::dto::auth_request_dto::OAuthCallbackQuery;
use crate::error::auth_error::AuthError;
use crate::service::oauth_service::OAuthService;

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

            // Create cookies for the authentication data
            let access_token_cookie = Cookie::build("access_token", &response.access_token)
                .path("/")
                .http_only(true)
                .secure(is_secure) // Only secure for HTTPS
                .same_site(SameSite::Strict)
                .max_age(Duration::seconds(response.expires_in as i64))
                .finish();

            let refresh_token_cookie = Cookie::build("refresh_token", &response.refresh_token)
                .path("/")
                .http_only(true)
                .secure(is_secure) // Only secure for HTTPS
                .same_site(SameSite::Strict)
                .max_age(Duration::days(30)) // Refresh token expires in 30 days
                .finish();

            // Serialize user info to JSON for the cookie
            let user_info_json = serde_json::to_string(&response.user).map_err(|e| {
                AuthError::CodeExchangeFailed(format!("Failed to serialize user info: {}", e))
            })?;

            let user_info_cookie = Cookie::build("user_info", user_info_json)
                .path("/")
                .http_only(true)
                .secure(is_secure) // Only secure for HTTPS
                .same_site(SameSite::Strict)
                .max_age(Duration::seconds(response.expires_in as i64))
                .finish();

            // Redirect to the portfolio page with cookies
            let redirect_url = format!("{}/portfolio", oauth_service.get_frontend_url());
            Ok(HttpResponse::Found()
                .cookie(access_token_cookie)
                .cookie(refresh_token_cookie)
                .cookie(user_info_cookie)
                .append_header(("Location", redirect_url))
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

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
use url::Url;

/// Initiate OAuth flow
/// GET /oauth/{provider}?platform=app|web
pub async fn initiate_oauth(
    provider: web::Path<String>,
    query: web::Query<std::collections::HashMap<String, String>>,
    oauth_service: web::Data<Arc<OAuthService>>,
) -> Result<HttpResponse, AuthError> {
    let platform = query.get("platform").map(|s| s.as_str());
    match oauth_service.get_auth_url_with_platform(&provider, platform).await {
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
        Ok((response, platform)) => {
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

            // Get the frontend URL based on platform
            let frontend_url = match platform.as_deref() {
                Some("app") => {
                    std::env::var("FRONTEND_URL_APP")
                        .or_else(|_| std::env::var("OAUTH_FRONTEND_URL_APP"))
                        .unwrap_or_else(|_| oauth_service.get_frontend_url().to_string())
                }
                Some("web") | _ => {
                    std::env::var("FRONTEND_URL_WEB")
                        .or_else(|_| std::env::var("OAUTH_FRONTEND_URL_WEB"))
                        .unwrap_or_else(|_| oauth_service.get_frontend_url().to_string())
                }
            };
            let is_secure = frontend_url.starts_with("https://");

            // Serialize user_info to JSON (needed for both platforms)
            let user_info_json = serde_json::to_string(&user_info).map_err(|e| {
                AuthError::Validation(format!("Failed to serialize user info: {}", e))
            })?;

            // Determine redirect URL and cookies based on platform
            match platform.as_deref() {
                Some("app") => {
                    // For app: redirect with query parameters (no cookies)
                    // Use encodeURIComponent-style encoding (form_urlencoded)
                    use url::form_urlencoded;
                    
                    // Encode values similar to JavaScript's encodeURIComponent
                    // form_urlencoded::byte_serialize provides encodeURIComponent-style encoding
                    let encoded_session: String = form_urlencoded::byte_serialize(session_id.as_bytes()).collect();
                    let encoded_user_info: String = form_urlencoded::byte_serialize(user_info_json.as_bytes()).collect();
                    
                    // Build URL with query parameters (works for both standard URLs and custom schemes like vex://)
                    let redirect_url = if frontend_url.contains("://") && !frontend_url.starts_with("http://") && !frontend_url.starts_with("https://") {
                        // Custom scheme (e.g., vex://) - manually append query params
                        format!("{}?session_id={}&user_info={}", frontend_url, encoded_session, encoded_user_info)
                    } else {
                        // Standard URL - use URL parsing for proper encoding
                        match Url::parse(&frontend_url) {
                            Ok(mut url) => {
                                url.query_pairs_mut()
                                    .append_pair("session_id", &session_id)
                                    .append_pair("user_info", &user_info_json);
                                url.to_string()
                            }
                            Err(_) => {
                                // Fallback: manually append if parsing fails
                                format!("{}?session_id={}&user_info={}", frontend_url, encoded_session, encoded_user_info)
                            }
                        }
                    };

                    Ok(HttpResponse::Found()
                        .append_header(("Location", redirect_url))
                        .finish())
                }
                Some("web") | _ => {
                    // For web: redirect to frontend_url with cookies (no query parameters)
                    // Create session cookie
                    let session_cookie = Cookie::build("session_id", session_id.clone())
                        .path("/")
                        .http_only(true)
                        .secure(is_secure)
                        .same_site(SameSite::Strict)
                        .max_age(Duration::seconds(response.expires_in as i64))
                        .finish();

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
                        .append_header(("Location", frontend_url))
                        .finish())
                }
            }
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

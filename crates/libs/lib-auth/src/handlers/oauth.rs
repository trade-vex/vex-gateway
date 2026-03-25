use crate::dto::auth_request_dto::{
    OAuthCallbackQuery, OAuthExchangeRequest, OAuthInitQuery, SessionRenewRequest,
};
use crate::dto::auth_response_dto::{OAuthExchangeResponse, SessionRenewResponse};
use crate::error::auth_error::AuthError;
use crate::service::{
    oauth_service::OAuthService, session_service::SessionService, token_service::TokenService,
};

use actix_web::{
    HttpRequest, HttpResponse,
    cookie::{Cookie, SameSite, time::Duration},
    web,
};
use serde_json;
use std::sync::Arc;
use tracing::error;
use url::Url;

fn build_session_cookie(session_id: String, is_secure: bool, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build("session_id", session_id)
        .path("/")
        .http_only(true)
        .secure(is_secure)
        .same_site(SameSite::Strict)
        .max_age(Duration::seconds(max_age_secs))
        .finish()
}

/// Initiate OAuth flow
/// GET /oauth/{provider}?platform=app|web&app_challenge=...
///
/// For mobile apps (platform=app), app_challenge is required. This is the SHA256
/// hash of a verifier that the app will use to redeem the login_code.
///
/// For web (platform=web or omitted), a state cookie is set to bind the OAuth
/// state to this browser session (prevents login CSRF / session swapping).
pub async fn initiate_oauth(
    req: HttpRequest,
    provider: web::Path<String>,
    query: web::Query<OAuthInitQuery>,
    oauth_service: web::Data<Arc<OAuthService>>,
) -> Result<HttpResponse, AuthError> {
    let platform = query.platform.as_deref();
    let app_challenge = query.app_challenge.as_deref();

    // Require app_challenge for mobile app flow
    if platform == Some("app") && app_challenge.is_none() {
        return Err(AuthError::InvalidRequest);
    }

    match oauth_service
        .get_auth_url_with_platform(&provider, platform, app_challenge)
        .await
    {
        Ok((url, state)) => {
            let mut response = HttpResponse::Found();

            // For web platform: bind state to user-agent session via cookie
            // This prevents login CSRF / session swapping attacks
            // Mobile apps don't use cookies, they use app_challenge instead
            if platform != Some("app") {
                let is_secure = req.connection_info().scheme() == "https";
                let cookie_name = format!("oauth_state_{}", provider.as_str());
                let state_cookie = Cookie::build(cookie_name, state)
                    .path(format!("/api/oauth/{}/callback", provider.as_str()))
                    .http_only(true)
                    .secure(is_secure)
                    .same_site(SameSite::Lax) // Must be Lax for OAuth redirect
                    .max_age(Duration::seconds(600))
                    .finish();
                response.cookie(state_cookie);
            }

            Ok(response.append_header(("Location", url)).finish())
        }
        Err(e) => {
            error!("Failed to initiate OAuth: {}", e);
            Err(e)
        }
    }
}

/// Handle OAuth callback
/// GET /oauth/{provider}/callback?code=...&state=...
///
/// For web: validates state cookie binding before processing.
/// For mobile: creates a one-time login_code instead of returning session_id directly.
pub async fn handle_oauth_callback(
    req: HttpRequest,
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

    // Check for state cookie (present for web, absent for mobile)
    let cookie_name = format!("oauth_state_{}", provider.as_str());
    let cookie_state = req.cookie(&cookie_name).map(|c| c.value().to_string());
    let has_state_cookie = cookie_state.is_some();

    // For web flow: enforce state cookie binding (prevents login CSRF / session swapping)
    // If cookie exists, it must match the query state
    if let Some(ref state_from_cookie) = cookie_state
        && state_from_cookie != &query.state
    {
        return Err(AuthError::StateMismatch);
    }

    match oauth_service
        .handle_callback(&provider, code, &query.state)
        .await
    {
        Ok((response, meta)) => {
            // For web flows (non-app), require the state cookie to prevent CSRF
            // Mobile apps don't have cookies, they use app_challenge verification instead
            if meta.app_challenge.is_none() && !has_state_cookie {
                return Err(AuthError::StateMismatch);
            }

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
            let frontend_url = match meta.platform.as_deref() {
                Some("app") => std::env::var("FRONTEND_URL_APP")
                    .or_else(|_| std::env::var("OAUTH_FRONTEND_URL_APP"))
                    .unwrap_or_else(|_| oauth_service.get_frontend_url().to_string()),
                _ => std::env::var("FRONTEND_URL_WEB")
                    .or_else(|_| std::env::var("OAUTH_FRONTEND_URL_WEB"))
                    .unwrap_or_else(|_| oauth_service.get_frontend_url().to_string()),
            };
            let is_secure = req.connection_info().scheme() == "https";

            // Serialize user_info to JSON
            let user_info_json = serde_json::to_string(&user_info).map_err(|e| {
                AuthError::Validation(format!("Failed to serialize user info: {}", e))
            })?;

            // Determine redirect URL and cookies based on platform
            match meta.platform.as_deref() {
                Some("app") => {
                    // Mobile: NEVER put session_id in the deep link (custom schemes can be intercepted)
                    // Instead, create a short-lived one-time login_code bound to the app_challenge
                    let app_challenge = meta.app_challenge.ok_or(AuthError::InvalidRequest)?;
                    let login_code = session_service
                        .create_login_code(
                            session_id,
                            response.refresh_token.clone(),
                            user_info_json,
                            app_challenge,
                        )
                        .await?;

                    // Build redirect URL with only login_code
                    let redirect_url = if frontend_url.contains("://")
                        && !frontend_url.starts_with("http://")
                        && !frontend_url.starts_with("https://")
                    {
                        // Custom scheme (e.g., vex://) - UUIDs are URL-safe, no encoding needed
                        format!("{}?login_code={}", frontend_url, login_code)
                    } else {
                        // Standard URL - use URL parsing for proper encoding
                        match Url::parse(&frontend_url) {
                            Ok(mut url) => {
                                // query_pairs_mut() automatically handles encoding if needed
                                url.query_pairs_mut().append_pair("login_code", &login_code);
                                url.to_string()
                            }
                            Err(_) => {
                                // Fallback - UUIDs are URL-safe
                                format!("{}?login_code={}", frontend_url, login_code)
                            }
                        }
                    };

                    Ok(HttpResponse::Found()
                        .insert_header(("Cache-Control", "no-store"))
                        .append_header(("Location", redirect_url))
                        .finish())
                }
                _ => {
                    // For web: session cookie only; frontend can call /profile to fetch user info
                    // Removed user_info cookie - it was XSS-sensitive (not HttpOnly)
                    // Use SessionService TTL for cookie max_age to align client/server state
                    let session_cookie = Cookie::build("session_id", session_id.clone())
                        .path("/")
                        .http_only(true)
                        .secure(is_secure)
                        .same_site(SameSite::Strict)
                        .max_age(Duration::seconds(session_service.ttl_seconds() as i64))
                        .finish();

                    // Clear the oauth_state cookie on success
                    let clear_state_cookie = Cookie::build(cookie_name, "")
                        .path(format!("/api/oauth/{}/callback", provider.as_str()))
                        .http_only(true)
                        .secure(is_secure)
                        .same_site(SameSite::Lax)
                        .max_age(Duration::seconds(0))
                        .finish();

                    Ok(HttpResponse::Found()
                        .cookie(session_cookie)
                        .cookie(clear_state_cookie)
                        .insert_header(("Cache-Control", "no-store"))
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

/// Exchange a one-time login_code for session credentials (mobile app only)
/// POST /oauth/exchange
///
/// This is the "second hop" of the mobile OAuth flow:
/// 1. App initiated OAuth with app_challenge (SHA256 hash of verifier)
/// 2. Callback returned a login_code in the deep link
/// 3. App calls this endpoint with login_code + verifier
/// 4. Server verifies SHA256(verifier) == app_challenge, returns session_id
///
/// This prevents interception attacks: even if a malicious app intercepts the
/// deep link, they don't have the verifier and cannot exchange the login_code.
pub async fn exchange_oauth_login_code(
    body: web::Json<OAuthExchangeRequest>,
    session_service: web::Data<Arc<SessionService>>,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    let login_code = body.login_code.trim();
    let verifier = body.verifier.trim();

    if login_code.is_empty() || verifier.is_empty() {
        return Err(AuthError::InvalidRequest);
    }

    // Atomically retrieve and delete login_code data
    let data = session_service
        .get_and_delete_login_code(login_code)
        .await?;

    // Verify PKCE-like S256 challenge: SHA256(verifier) must equal app_challenge
    let computed = crate::utils::pkce::s256_challenge(verifier)?;
    if computed != data.app_challenge {
        // Don't reveal whether the code was valid or challenge failed
        return Err(AuthError::InvalidRequest);
    }

    // Parse user_info JSON
    let user_info_val: serde_json::Value =
        serde_json::from_str(&data.user_info_json).map_err(|_| AuthError::InvalidRequest)?;

    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .json(OAuthExchangeResponse {
            session_id: data.session_id,
            refresh_token: data.refresh_token,
            session_expires_in: session_service.ttl_seconds(),
            refresh_expires_in: token_service.refresh_token_ttl_seconds()?,
            user_info: user_info_val,
        }))
}

/// Renew a session using a rotated refresh token
/// POST /session/renew
///
/// This endpoint is shared by web and mobile clients after their short-lived
/// `session_id` has expired. The client presents the last issued refresh token,
/// the server rotates it, creates a fresh Redis-backed session, and returns the
/// new `session_id` plus the rotated refresh token. A fresh `session_id` cookie
/// is also set for browser-based clients.
pub async fn renew_session(
    req: HttpRequest,
    body: web::Json<SessionRenewRequest>,
    session_service: web::Data<Arc<SessionService>>,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    let refresh_token = body.refresh_token.trim();

    if refresh_token.is_empty() {
        return Err(AuthError::InvalidRequest);
    }

    let new_tokens = token_service.refresh_tokens(refresh_token).await?;
    let claims = token_service.validate_jwt(&new_tokens.access_token)?;
    let user_id =
        uuid::Uuid::parse_str(&claims.sub).map_err(|_| AuthError::InternalError(
            "Minted access token contains invalid subject".to_string(),
        ))?;

    let session_id = session_service
        .create_session(
            user_id,
            new_tokens.access_token,
            new_tokens.refresh_token.clone(),
        )
        .await?;

    let session_cookie = build_session_cookie(
        session_id.clone(),
        req.connection_info().scheme() == "https",
        session_service.ttl_seconds() as i64,
    );

    Ok(HttpResponse::Ok()
        .cookie(session_cookie)
        .insert_header(("Cache-Control", "no-store"))
        .json(SessionRenewResponse {
            session_id,
            refresh_token: new_tokens.refresh_token,
            session_expires_in: session_service.ttl_seconds(),
            refresh_expires_in: token_service.refresh_token_ttl_seconds()?,
        }))
}

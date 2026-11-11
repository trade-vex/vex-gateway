use crate::dto::auth_request_dto::{OAuthCallbackQuery, OAuthExchangeRequest, OAuthInitQuery};
use crate::dto::auth_response_dto::OAuthExchangeResponse;
use crate::error::auth_error::AuthError;
use crate::handlers::auth::session_cookie;
use crate::service::{
    email_service::EmailService,
    oauth_service::{OAuthCallbackMeta, OAuthService},
    session_service::SessionService,
};

use actix_web::{
    HttpRequest, HttpResponse,
    cookie::{Cookie, SameSite, time::Duration},
    web,
};
use lib_middleware::client_ip::{client_ip_key, is_https};
use serde_json;
use std::sync::Arc;
use tracing::error;
use url::Url;

pub(crate) fn validate_callback_state_cookie(
    meta: &OAuthCallbackMeta,
    has_state_cookie: bool,
) -> Result<(), AuthError> {
    if meta.platform.as_deref() != Some("app") && !has_state_cookie {
        return Err(AuthError::StateMismatch);
    }

    Ok(())
}

/// `base` with `key=value` appended to its query, whatever its scheme.
fn login_redirect(base: &str, key: &str, value: &str) -> Result<String, AuthError> {
    let mut url = Url::parse(base)
        .map_err(|e| AuthError::Config(format!("frontend URL {base:?} is not a URL: {e}")))?;
    url.query_pairs_mut().append_pair(key, value);
    Ok(url.to_string())
}

const OAUTH_INIT_RATE_LIMIT_SCOPE: &str = "oauth_initiate";
const OAUTH_CALLBACK_RATE_LIMIT_SCOPE: &str = "oauth_callback";
// `client_ip_key` honours X-Forwarded-For only from configured trusted proxies
// (`auth.server.trusted_proxies`), and keys an IPv6 client by its /64.
// This loose limit is a coarse flood guard.
const OAUTH_IP_MAX_REQUESTS: u64 = 300;
const OAUTH_IP_WINDOW_SECONDS: u64 = 5 * 60;

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
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    email_service
        .enforce_rate_limit(
            OAUTH_INIT_RATE_LIMIT_SCOPE,
            &client_ip_key(&req),
            OAUTH_IP_MAX_REQUESTS,
            OAUTH_IP_WINDOW_SECONDS,
        )
        .await?;

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
                let is_secure = is_https(&req);
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
/// Without a code (the user cancelled, or the provider answered `error`), redirects to the
/// front end with `?error=oauth_cancelled`.
/// For web: validates state cookie binding before processing.
/// For mobile: creates a one-time login_code instead of returning session_id directly.
pub async fn handle_oauth_callback(
    req: HttpRequest,
    provider: web::Path<String>,
    query: web::Query<OAuthCallbackQuery>,
    oauth_service: web::Data<Arc<OAuthService>>,
    session_service: web::Data<Arc<SessionService>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    email_service
        .enforce_rate_limit(
            OAUTH_CALLBACK_RATE_LIMIT_SCOPE,
            &client_ip_key(&req),
            OAUTH_IP_MAX_REQUESTS,
            OAUTH_IP_WINDOW_SECONDS,
        )
        .await?;

    // Check for state cookie (present for web, absent for mobile)
    let cookie_name = format!("oauth_state_{}", provider.as_str());
    let cookie_state = req.cookie(&cookie_name).map(|c| c.value().to_string());
    let has_state_cookie = cookie_state.is_some();
    let clear_state_cookie = Cookie::build(cookie_name, "")
        .path(format!("/api/oauth/{}/callback", provider.as_str()))
        .http_only(true)
        .secure(is_https(&req))
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(0))
        .finish();

    // The user cancelled or the provider refused: land on the front end, not on this API. Only a
    // browser login has the state cookie.
    let code = match query.code.as_deref().filter(|code| !code.is_empty()) {
        Some(code) if query.error.is_none() => code,
        _ => {
            let platform = if has_state_cookie { None } else { Some("app") };
            let landing = login_redirect(
                oauth_service.frontend_url(platform)?,
                "error",
                "oauth_cancelled",
            )?;
            return Ok(HttpResponse::Found()
                .cookie(clear_state_cookie)
                .insert_header(("Cache-Control", "no-store"))
                .append_header(("Location", landing))
                .finish());
        }
    };

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
            validate_callback_state_cookie(&meta, has_state_cookie)?;

            // Get the frontend URL based on platform, before a session exists to orphan
            let frontend_url = oauth_service
                .frontend_url(meta.platform.as_deref())?
                .to_string();

            // Get user_id and user_info from the response
            let user_id = response.user.user_id;
            let user_info = response.user.clone();

            // Create session with user_id and its access token
            let session_id = session_service
                .create_session(user_id, response.access_token.clone())
                .await
                .map_err(|e| {
                    AuthError::CodeExchangeFailed(format!("Failed to create session: {}", e))
                })?;

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
                        .create_login_code(session_id, user_info_json, app_challenge)
                        .await?;

                    let redirect_url = login_redirect(&frontend_url, "login_code", &login_code)?;

                    Ok(HttpResponse::Found()
                        .insert_header(("Cache-Control", "no-store"))
                        .append_header(("Location", redirect_url))
                        .finish())
                }
                _ => {
                    // For web: session cookie only; frontend can call /profile to fetch user info
                    // Removed user_info cookie - it was XSS-sensitive (not HttpOnly)
                    // Clear the oauth_state cookie on success
                    Ok(HttpResponse::Found()
                        .cookie(session_cookie(&req, session_id))
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
            user_info: user_info_val,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OAuthConfig, OAuthProviderConfig};
    use crate::handlers::auth::tests::test_services;
    use crate::service::oauth_service::tests::test_service;
    use actix_web::{
        App,
        dev::ServiceResponse,
        http::{StatusCode, header::LOCATION},
        test::{self, TestRequest},
    };
    use secrecy::SecretString;
    use std::collections::HashMap;

    /// GitHub as the one provider; its token endpoint refuses connections.
    fn oauth_config() -> OAuthConfig {
        OAuthConfig {
            providers: HashMap::from([(
                "github".to_string(),
                OAuthProviderConfig {
                    client_id: "id".to_string(),
                    client_secret: SecretString::new("secret".to_string()),
                    auth_url: "https://github.com/login/oauth/authorize".to_string(),
                    token_url: "http://127.0.0.1:9/token".to_string(),
                    issuer_url: None,
                },
            )]),
            redirect_base: "http://localhost".to_string(),
            frontend_url_web: Some("https://app.example.com/oauth/done".to_string()),
            frontend_url_app: Some("vex://oauth".to_string()),
        }
    }

    /// Sends `req` to the callback route, whose services use the test Redis.
    async fn callback(req: TestRequest) -> ServiceResponse {
        let (email_service, _, session_service) = test_services().await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Arc::new(
                    test_service(&oauth_config()).await,
                )))
                .app_data(web::Data::new(session_service))
                .app_data(web::Data::new(email_service))
                .route(
                    "/oauth/{provider}/callback",
                    web::get().to(handle_oauth_callback),
                ),
        )
        .await;
        let req = req
            .peer_addr("198.51.100.7:1".parse().unwrap())
            .to_request();
        test::call_service(&app, req).await
    }

    #[actix_web::test]
    async fn cancelled_oauth_login_redirects_to_the_front_end_with_an_error() {
        // A browser login has the state cookie; the app's has none.
        for (state_cookie, landing) in [
            (
                true,
                "https://app.example.com/oauth/done?error=oauth_cancelled",
            ),
            (false, "vex://oauth?error=oauth_cancelled"),
        ] {
            let mut req =
                TestRequest::get().uri("/oauth/github/callback?error=access_denied&state=s");
            if state_cookie {
                req = req.cookie(Cookie::new("oauth_state_github", "s"));
            }

            let response = callback(req).await;

            assert_eq!(response.status(), StatusCode::FOUND);
            assert_eq!(response.headers().get(LOCATION).unwrap(), landing);
            let cleared = response
                .response()
                .cookies()
                .find(|cookie| cookie.name() == "oauth_state_github")
                .unwrap();
            assert_eq!(cleared.max_age(), Some(Duration::ZERO));
        }
    }

    #[test]
    fn the_mobile_redirect_keeps_an_existing_query_on_the_app_url() {
        for (base, redirect) in [
            ("vex://oauth", "vex://oauth?login_code=abc"),
            (
                "vex://oauth?src=login",
                "vex://oauth?src=login&login_code=abc",
            ),
            (
                "https://app.example.com/done",
                "https://app.example.com/done?login_code=abc",
            ),
        ] {
            assert_eq!(login_redirect(base, "login_code", "abc").unwrap(), redirect);
        }
    }
}

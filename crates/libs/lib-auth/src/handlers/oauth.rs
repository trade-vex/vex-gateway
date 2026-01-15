use crate::dto::auth_request_dto::{OAuthCallbackQuery, OAuthVerifyRequest};
use crate::error::auth_error::AuthError;
use crate::service::{oauth_service::OAuthService, session_service::SessionService};

use actix_web::{
    HttpResponse,
    cookie::{Cookie, SameSite, time::Duration},
    web,
};
use serde_json;
use std::sync::Arc;
use tracing::{error, warn};
use url::Url;

/// Initiate OAuth flow
/// GET /oauth/{provider}?platform=app|web&code_challenge=<SHA256_HASH>
/// 
/// # Parameters
/// 
/// - `platform`: "app" or "web" (optional)
/// - `code_challenge`: SHA256 hash of code_verifier (optional, but required for app platform security)
pub async fn initiate_oauth(
    provider: web::Path<String>,
    query: web::Query<std::collections::HashMap<String, String>>,
    oauth_service: web::Data<Arc<OAuthService>>,
) -> Result<HttpResponse, AuthError> {
    let platform = query.get("platform").map(|s| s.as_str());
    let code_challenge = query.get("code_challenge").map(|s| s.as_str());
    
    // For app platform, code_challenge is recommended for security
    if platform == Some("app") && code_challenge.is_none() {
        warn!("OAuth initiation for app platform without code_challenge - verification code will not be PKCE-protected");
    }
    
    match oauth_service.get_auth_url_with_platform(&provider, platform, code_challenge).await {
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
                    // SECURITY: For app platform, use PKCE-protected verification code
                    // Retrieve code_challenge that was stored during OAuth initiation
                    let code_challenge = oauth_service
                        .get_stored_code_challenge(&provider, &query.state)
                        .await?
                        .unwrap_or_default(); // Fallback to empty if not provided
                    
                    // Store verification code WITH code_challenge (PKCE protection)
                    let verification_code = session_service
                        .store_oauth_verification_code(
                            session_id,
                            user_info_json,
                            code_challenge, // PKCE challenge for verification
                        )
                        .await
                        .map_err(|e| {
                            AuthError::CodeExchangeFailed(format!(
                                "Failed to store verification code: {}",
                                e
                            ))
                        })?;

                    // Build URL with ONLY verification code (not session_id or user_info)
                    let redirect_url = if frontend_url.contains("://")
                        && !frontend_url.starts_with("http://")
                        && !frontend_url.starts_with("https://")
                    {
                        // Custom scheme (e.g., vex://) - manually append query param
                        format!("{}?code={}", frontend_url, verification_code)
                    } else {
                        // Standard URL - use URL parsing for proper encoding
                        match Url::parse(&frontend_url) {
                            Ok(mut url) => {
                                url.query_pairs_mut()
                                    .clear()
                                    .append_pair("code", &verification_code);
                                url.to_string()
                            }
                            Err(_) => {
                                // Fallback: manually append if parsing fails
                                format!("{}?code={}", frontend_url, verification_code)
                            }
                        }
                    };

                    Ok(HttpResponse::Found()
                        .append_header(("Location", redirect_url))
                        .finish())
                }
                Some("web") | _ => {
                    // For web: redirect to frontend_url with cookies (no query parameters)
                    // Cookies are more secure than URL parameters for web
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

/// Verify OAuth callback code and return session securely
/// POST /api/oauth/verify
/// 
/// # Request Body
/// 
/// ```json
/// {
///   "code": "verification_code_from_callback",
///   "code_verifier": "frontend_generated_verifier"
/// }
/// ```
/// 
/// # Security
/// 
/// - Verifies PKCE code_challenge matches SHA256(code_verifier)
/// - Single-use verification code (deleted after use)
/// - 5-minute TTL on verification codes
/// - Prevents interception attacks
pub async fn verify_oauth_callback(
    req: web::Json<OAuthVerifyRequest>,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    use sha2::{Sha256, Digest};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    // Consume the verification code (single-use)
    let verification_data = session_service
        .consume_oauth_verification_code(&req.code)
        .await?;

    // PKCE VERIFICATION: Compute challenge from verifier and compare
    let mut hasher = Sha256::new();
    hasher.update(req.code_verifier.as_bytes());
    let computed_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    // CRITICAL SECURITY CHECK: Challenge must match stored challenge
    if computed_challenge != verification_data.code_challenge {
        warn!(
            "PKCE verification failed: stored_challenge={}, computed_challenge={}",
            verification_data.code_challenge,
            computed_challenge
        );
        return Err(AuthError::InvalidVerificationCode);
    }

    // Parse user info from stored JSON
    let user_info: crate::dto::auth_response_dto::UserInfo =
        serde_json::from_str(&verification_data.user_info)
            .map_err(|e| AuthError::Validation(format!("Failed to parse user info: {}", e)))?;

    // Verify the session exists and is valid
    let session_data = session_service
        .get_session(&verification_data.session_id)
        .await?;

    // Return session_id and user_info securely via JSON response
    // Frontend should store these securely (not from URL parameters)
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "session_id": verification_data.session_id,
        "user": user_info,
        "access_token": session_data.access_token,
        "refresh_token": session_data.refresh_token,
    })))
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

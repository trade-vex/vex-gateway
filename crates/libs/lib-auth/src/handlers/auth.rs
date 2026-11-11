use actix_web::{HttpRequest, HttpResponse, web};
use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
use validator::Validate;

use crate::dto::auth_request_dto::{
    ChangePasswordRequest, CheckEmailRequest, ForgotPasswordRequest, LoginRequest,
    RefreshTokenRequest, ResetPasswordRequest, SignupRequest, VerifyEmailRequest,
    VerifyResetOtpRequest,
};
use crate::dto::auth_response_dto::{RefreshTokenResponse, SignupResponse, UserInfo, UserProfile};
use crate::error::auth_error::AuthError;
use crate::middleware::jwt_auth::AuthenticatedUser;
use crate::models::user::{Model, UserStatus};
use crate::repository::UserRepository;
use crate::service::{EmailService, SessionService, TokenService};
use lib_middleware::client_ip::{client_ip_key, is_https};
use lib_middleware::metrics::{self, AuthMethod};
use std::sync::Arc;

const LOGIN_RATE_LIMIT_SCOPE: &str = "login";
// The IP limits key on `client_ip_key`: the TCP peer unless the peer is a configured trusted
// proxy (`auth.server.trusted_proxies`), and an IPv6 client by its /64,
// so behind an unconfigured load balancer every client shares one IP bucket.
// These loose limits are coarse flood guards.
const LOGIN_IP_MAX_REQUESTS: u64 = 300;
const LOGIN_IP_WINDOW_SECONDS: u64 = 60;
const LOGIN_PRINCIPAL_MAX_REQUESTS: u64 = 5;
const LOGIN_PRINCIPAL_WINDOW_SECONDS: u64 = 5 * 60;

const SIGNUP_RATE_LIMIT_SCOPE: &str = "signup";
const SIGNUP_IP_MAX_REQUESTS: u64 = 300;
const SIGNUP_IP_WINDOW_SECONDS: u64 = 10 * 60;
const SIGNUP_PRINCIPAL_MAX_REQUESTS: u64 = 5;
const SIGNUP_PRINCIPAL_WINDOW_SECONDS: u64 = 15 * 60;

const VERIFY_EMAIL_RATE_LIMIT_SCOPE: &str = "verify_email";
const VERIFY_EMAIL_IP_MAX_REQUESTS: u64 = 600;
const VERIFY_EMAIL_IP_WINDOW_SECONDS: u64 = 15 * 60;
const VERIFY_EMAIL_PRINCIPAL_MAX_REQUESTS: u64 = 10;
const VERIFY_EMAIL_PRINCIPAL_WINDOW_SECONDS: u64 = 15 * 60;

fn normalized_principal(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// A login refused for its email and password, counted as a login authentication failure.
fn refused_login() -> AuthError {
    metrics::auth_failures(AuthMethod::Login).inc();
    AuthError::InvalidCredentials
}

/// Session cookie attributes for `req`. Returns (is_secure, cookie_ttl) where:
/// - is_secure: the request used HTTPS: a TLS connection, or `X-Forwarded-Proto: https` from a
///   trusted proxy (`is_https`), the same rule the OAuth handlers apply to their cookies
/// - cookie_ttl: cookie time-to-live in seconds (24 hours)
fn get_cookie_config(req: &HttpRequest) -> (bool, i64) {
    let is_secure = is_https(req);
    let cookie_ttl = 24 * 3600;
    (is_secure, cookie_ttl)
}

// Signup handler
pub async fn signup(
    http_req: HttpRequest,
    req: web::Json<SignupRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    email_service
        .enforce_rate_limit(
            SIGNUP_RATE_LIMIT_SCOPE,
            &client_ip_key(&http_req),
            SIGNUP_IP_MAX_REQUESTS,
            SIGNUP_IP_WINDOW_SECONDS,
        )
        .await?;

    // Validate the request before using its principals in Redis keys.
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    email_service
        .enforce_rate_limit(
            SIGNUP_RATE_LIMIT_SCOPE,
            &format!("email:{}", normalized_principal(&req.email)),
            SIGNUP_PRINCIPAL_MAX_REQUESTS,
            SIGNUP_PRINCIPAL_WINDOW_SECONDS,
        )
        .await?;
    email_service
        .enforce_rate_limit(
            SIGNUP_RATE_LIMIT_SCOPE,
            &format!("username:{}", normalized_principal(&req.username)),
            SIGNUP_PRINCIPAL_MAX_REQUESTS,
            SIGNUP_PRINCIPAL_WINDOW_SECONDS,
        )
        .await?;

    tracing::debug!("Signup request");

    // Check if user already exists
    if let Some(_existing_user) = user_repo.find_by_email(&req.email).await? {
        return Err(AuthError::EmailAlreadyExists);
    }

    // Hash the password
    let password_hash = tokio::task::spawn_blocking({
        let pwd = req.password.clone();
        move || hash(&pwd, DEFAULT_COST)
    })
    .await
    .map_err(|e| AuthError::Validation(format!("Join error: {}", e)))?
    .map_err(|e| AuthError::Validation(format!("Password hashing failed: {}", e)))?;

    // Create new user
    let now = Utc::now();
    let new_user = Model {
        user_id: Uuid::new_v4(),
        email: req.email.clone(),
        username: req.username.clone(),
        password_hash: Some(password_hash),
        auth_provider: "email".to_string(),
        provider_id: None,
        avatar_url: None,
        email_verified: false,
        status: UserStatus::Pending,
        created_at: now,
        updated_at: now,
        last_login: None,
    };

    // Save user to database
    let created_user = user_repo.create_user(&new_user).await?;

    // Send verification email (skip in development if SMTP is not available)
    if let Err(e) = email_service
        .send_verification_email(&created_user.email, &created_user.username)
        .await
    {
        tracing::warn!(
            "Failed to send verification email (this is expected in development): {}",
            e
        );
    }

    let user_info = UserInfo::from(created_user);
    let response = SignupResponse {
        user: user_info,
        message: "User created successfully. Please check your email for the verification code."
            .to_string(),
    };

    Ok(HttpResponse::Created().json(response))
}

// Login handler
pub async fn login(
    http_req: HttpRequest,
    req: web::Json<LoginRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    token_service: web::Data<Arc<TokenService>>,
    session_service: web::Data<Arc<SessionService>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    // Key the failure budget on (peer IP, email), not the email alone. A principal-only bucket
    // lets an unauthenticated attacker exhaust a victim's budget from anywhere and lock them out
    // of their own account -- the success path that would clear it is unreachable while locked
    // (finding G3-2). The separate per-IP flood limit above still bounds a single source.
    let login_principal = format!(
        "email:{}|{}",
        normalized_principal(&req.email),
        client_ip_key(&http_req)
    );
    email_service
        .enforce_rate_limit(
            LOGIN_RATE_LIMIT_SCOPE,
            &client_ip_key(&http_req),
            LOGIN_IP_MAX_REQUESTS,
            LOGIN_IP_WINDOW_SECONDS,
        )
        .await?;
    email_service
        .check_login_failure_rate_limit(
            &login_principal,
            LOGIN_PRINCIPAL_MAX_REQUESTS,
            LOGIN_PRINCIPAL_WINDOW_SECONDS,
        )
        .await?;

    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    tracing::debug!("Login request");

    // Find user by email
    let user = match user_repo.find_by_email(&req.email).await? {
        Some(user) => user,
        None => {
            email_service
                .record_login_authentication(
                    &login_principal,
                    false,
                    LOGIN_PRINCIPAL_WINDOW_SECONDS,
                )
                .await?;
            return Err(refused_login());
        }
    };
    // PR #85: capture the token version immediately after the lookup and
    // BEFORE the bcrypt verification below, so a racing password change cannot
    // have a token minted at a version that outlives its revocation.
    let token_version = token_service.get_token_version(user.user_id).await?;

    // ...and re-read the user AFTER capturing that version, so the password hash we verify is
    // never OLDER than the version we will mint at. Without this, a password reset committing
    // between the two reads leaves the old hash verifying while the new version is minted, and
    // the resulting token survives the revocation that reset was supposed to perform (G3-1).
    // If a reset lands after this re-read instead, it bumps the version past the captured one and
    // `mint_tokens_at_version` fails closed. The two together close the window.
    let user = match user_repo.find_by_id(user.user_id).await? {
        Some(user) => user,
        None => return Err(refused_login()),
    };

    // Check if user can login
    if !user.can_login() {
        return Err(AuthError::EmailNotVerified);
    }

    // Verify password
    let password_hash = match user.password_hash.clone() {
        Some(password_hash) => password_hash,
        None => {
            email_service
                .record_login_authentication(
                    &login_principal,
                    false,
                    LOGIN_PRINCIPAL_WINDOW_SECONDS,
                )
                .await?;
            return Err(refused_login());
        }
    };

    let login_lock_token = email_service
        .acquire_login_inflight_lock(&login_principal)
        .await?;
    let authenticated = tokio::task::spawn_blocking({
        let p = req.password.clone();
        let h = password_hash.clone();
        move || verify(&p, &h)
    })
    .await
    .map_err(|e| AuthError::Validation(format!("Join error: {}", e)))?
    .map_err(|e| AuthError::Validation(format!("Password verification failed: {}", e)));

    let authenticated = match authenticated {
        Ok(authenticated) => authenticated,
        Err(error) => {
            email_service
                .release_login_inflight_lock(&login_principal, &login_lock_token)
                .await?;
            return Err(error);
        }
    };

    let record_result = email_service
        .record_login_authentication(
            &login_principal,
            authenticated,
            LOGIN_PRINCIPAL_WINDOW_SECONDS,
        )
        .await;
    let release_result = email_service
        .release_login_inflight_lock(&login_principal, &login_lock_token)
        .await;
    record_result?;
    release_result?;

    if !authenticated {
        return Err(refused_login());
    }

    let user_id = user.user_id;

    // Generate tokens
    let tokens = token_service
        .mint_tokens_at_version(user_id, token_version)
        .await?;

    // Update last login
    user_repo.update_last_login(user_id).await?;

    // Create session (store user_id and tokens in Redis)
    let session_id = session_service
        .create_session(
            user_id,
            tokens.access_token.clone(),
            tokens.refresh_token.clone(),
        )
        .await?;

    // Create session cookie
    let (is_secure, cookie_ttl) = get_cookie_config(&http_req);

    let session_cookie = actix_web::cookie::Cookie::build("session_id", session_id)
        .path("/")
        .http_only(true)
        .secure(is_secure)
        .same_site(actix_web::cookie::SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(cookie_ttl))
        .finish();

    Ok(HttpResponse::Ok().cookie(session_cookie).json(json!({
        "message": "Logged in successfully"
    })))
}

// Logout handler
pub async fn logout(
    req: HttpRequest,
    user: AuthenticatedUser,
    token_service: web::Data<Arc<TokenService>>,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    token_service.bump_token_version(user.user_id).await?;

    // Try to extract session ID from cookie
    if let Some(cookie) = req.cookie("session_id") {
        // Delete session from Redis
        if let Err(e) = session_service.delete_session(cookie.value()).await {
            tracing::warn!("Failed to delete session during logout: {}", e);
        }
    }

    // Also handle JWT token if present
    if let Some(auth_header) = req.headers().get("Authorization")
        && let Ok(auth_str) = auth_header.to_str()
        && let Some(stripped) = auth_str.strip_prefix("Bearer ")
        && let Err(e) = token_service.blacklist_token(stripped).await
    {
        tracing::warn!("Failed to blacklist token during logout: {}", e);
    }

    // Create cookie to delete the session cookie
    let (is_secure, _cookie_ttl) = get_cookie_config(&req);

    let delete_session_cookie = actix_web::cookie::Cookie::build("session_id", "")
        .path("/")
        .http_only(true)
        .secure(is_secure)
        .same_site(actix_web::cookie::SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(0))
        .finish();

    Ok(HttpResponse::Ok()
        .cookie(delete_session_cookie)
        .json(json!({
            "message": "Logged out successfully",
            "timestamp": chrono::Utc::now().to_rfc3339()
        })))
}

// Get user profile
pub async fn get_profile(
    user: AuthenticatedUser,
    user_repo: web::Data<Arc<dyn UserRepository>>,
) -> Result<HttpResponse, AuthError> {
    tracing::debug!("Get profile request");

    let user_record = user_repo
        .find_by_id(user.user_id)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    let profile = UserProfile {
        user_id: user_record.user_id,
        email: user_record.email,
        username: user_record.username,
        avatar_url: user_record.avatar_url,
        auth_provider: user_record.auth_provider,
        email_verified: user_record.email_verified,
        created_at: user_record.created_at,
        last_login: user_record.last_login,
    };

    Ok(HttpResponse::Ok().json(profile))
}

// Refresh token handler
pub async fn refresh_token(
    req: web::Json<RefreshTokenRequest>,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    tracing::debug!("Token refresh request");

    let new_tokens = token_service.refresh_tokens(&req.refresh_token).await?;

    let response = RefreshTokenResponse {
        access_token: new_tokens.access_token,
        token_type: "Bearer".to_string(),
        expires_in: new_tokens.expires_in,
    };

    Ok(HttpResponse::Ok().json(response))
}

// Verify email with OTP handler (public endpoint)
pub async fn verify_email_with_otp(
    http_req: HttpRequest,
    req: web::Json<VerifyEmailRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    email_service: web::Data<Arc<EmailService>>,
    token_service: web::Data<Arc<TokenService>>,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    email_service
        .enforce_rate_limit(
            VERIFY_EMAIL_RATE_LIMIT_SCOPE,
            &client_ip_key(&http_req),
            VERIFY_EMAIL_IP_MAX_REQUESTS,
            VERIFY_EMAIL_IP_WINDOW_SECONDS,
        )
        .await?;

    // Validate the request before using its principal in a Redis key.
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    email_service
        .enforce_rate_limit(
            VERIFY_EMAIL_RATE_LIMIT_SCOPE,
            &format!("email:{}", normalized_principal(&req.email)),
            VERIFY_EMAIL_PRINCIPAL_MAX_REQUESTS,
            VERIFY_EMAIL_PRINCIPAL_WINDOW_SECONDS,
        )
        .await?;

    tracing::debug!("Email verification request with OTP");

    // Find user by email
    let user = user_repo
        .find_by_email(&req.email)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    // Check if already verified
    if user.email_verified {
        return Err(AuthError::Validation("Email already verified".to_string()));
    }

    // Verify OTP
    email_service.verify_otp(&req.email, &req.otp).await?;

    // Update user status to verified
    user_repo.verify_email(user.user_id).await?;

    // Generate tokens now that email is verified
    let tokens = token_service.mint_tokens(user.user_id).await?;

    // Update last login
    user_repo.update_last_login(user.user_id).await?;

    tracing::debug!("Creating session for user_id: {}", user.user_id);

    // Create session (stores user_id and tokens in Redis)
    let session_id = session_service
        .create_session(
            user.user_id,
            tokens.access_token.clone(),
            tokens.refresh_token.clone(),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to create session: {}", e);
            AuthError::Redis(format!("Failed to create session: {}", e))
        })?;

    // Never log the session id: it is a bearer credential (see combined_auth.rs).
    tracing::debug!("Session created successfully");

    // Create session cookie
    let (is_secure, cookie_ttl) = get_cookie_config(&http_req);

    let session_cookie = actix_web::cookie::Cookie::build("session_id", session_id)
        .path("/")
        .http_only(true)
        .secure(is_secure)
        .same_site(actix_web::cookie::SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(cookie_ttl))
        .finish();

    tracing::debug!("Session cookie created, sending response with cookie");

    // Return only success message (session created with cookie)
    Ok(HttpResponse::Ok().cookie(session_cookie).json(json!({
        "message": "Email verified successfully"
    })))
}

// Verify email handler (internal handler)
pub async fn verify_email(
    user: AuthenticatedUser,
    user_repo: web::Data<Arc<dyn UserRepository>>,
) -> Result<HttpResponse, AuthError> {
    tracing::debug!("Email verification request");

    user_repo.verify_email(user.user_id).await?;

    Ok(HttpResponse::Ok().json(json!({
        "message": "Email verified successfully",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

// Change password handler
pub async fn change_password(
    req: HttpRequest,
    user: AuthenticatedUser,
    body: web::Json<ChangePasswordRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    token_service: web::Data<Arc<TokenService>>,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    body.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    // Get user
    let user_record = user_repo
        .find_by_id(user.user_id)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    // Verify old password
    if let Some(password_hash) = &user_record.password_hash {
        if !tokio::task::spawn_blocking({
            let p = body.old_password.clone();
            let h = password_hash.clone();
            move || verify(&p, &h)
        })
        .await
        .map_err(|e| AuthError::Validation(format!("Join error: {}", e)))?
        .map_err(|e| AuthError::Validation(format!("Password verification failed: {}", e)))?
        {
            return Err(AuthError::InvalidCredentials);
        }
    } else {
        return Err(AuthError::InvalidCredentials);
    }

    // Hash new password
    let new_password_hash = tokio::task::spawn_blocking({
        let p = body.new_password.clone();
        move || hash(&p, DEFAULT_COST)
    })
    .await
    .map_err(|e| AuthError::Validation(format!("Join error: {}", e)))?
    .map_err(|e| AuthError::Validation(format!("Password hashing failed: {}", e)))?;

    // Update password in database
    user_repo
        .update_password(user.user_id, &new_password_hash)
        .await?;

    // Invalidate all previously issued access and refresh tokens.
    token_service.bump_token_version(user.user_id).await?;

    // Generate new tokens at the current version.
    let new_tokens = token_service.mint_tokens(user.user_id).await?;

    // Get current session ID from cookie if present
    let current_session_id = req.cookie("session_id").map(|c| c.value().to_string());

    if let Some(session_id) = current_session_id {
        // Update current session with new tokens (keep user logged in)
        session_service
            .update_session_tokens(
                &session_id,
                new_tokens.access_token.clone(),
                new_tokens.refresh_token.clone(),
            )
            .await
            .map_err(|e| {
                tracing::warn!("Failed to update current session tokens: {}", e);
                AuthError::Redis(format!("Failed to update session: {}", e))
            })?;

        // Revoke all other sessions (other devices)
        let revoked_count = session_service
            .revoke_user_sessions_except(user.user_id, &session_id)
            .await
            .map_err(|e| {
                tracing::warn!("Failed to revoke other sessions: {}", e);
                AuthError::Redis(format!("Failed to revoke sessions: {}", e))
            })?;

        tracing::debug!(
            "Password changed: updated current session, revoked {} other sessions",
            revoked_count
        );
    } else {
        // No session cookie found - this shouldn't happen in normal flow,
        // but handle gracefully by revoking all sessions
        tracing::warn!("Change password called without session cookie - revoking all sessions");
        if let Err(e) = session_service.revoke_all_user_sessions(user.user_id).await {
            tracing::error!(
                "Failed to revoke sessions after password change for user {}: {}",
                user.user_id,
                e
            );
            return Err(AuthError::Redis(
                "Failed to revoke sessions; please retry".to_string(),
            ));
        }
    }

    Ok(HttpResponse::Ok().json(json!({
        "message": "Password changed successfully",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

// Check if email exists handler
pub async fn check_email_exists(
    req: web::Json<CheckEmailRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    tracing::debug!("Check email exists request for: {}", req.email);

    match user_repo.find_by_email(&req.email).await? {
        Some(user) => Ok(HttpResponse::Ok().json(json!({
            "exists": true,
            "email_verified": user.email_verified
        }))),
        None => Ok(HttpResponse::Ok().json(json!({
            "exists": false,
            "email_verified": false
        }))),
    }
}

// resend verification email handler
pub async fn resend_verification_email(
    req: web::Json<CheckEmailRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    // validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    tracing::debug!("Resend verification email request for: {}", req.email);

    email_service
        .enforce_resend_verification_rate_limit(&req.email)
        .await?;

    // check if user exists by email
    let user = user_repo.find_by_email(&req.email).await?;

    match user {
        Some(user_model) => {
            // checking if email is already verified
            if user_model.email_verified {
                return Err(AuthError::Validation("Email already verified".to_string()));
            }

            // resend verification email
            match email_service
                .resend_verification_email(&req.email, &user_model.username)
                .await
            {
                Ok(new_otp_sent) => {
                    let message = if new_otp_sent {
                        "New verification code sent successfully"
                    } else {
                        "Verification code resent successfully"
                    };

                    Ok(HttpResponse::Ok().json(json!({
                        "message": message,
                        "new_code_sent": new_otp_sent,
                        "timestamp": chrono::Utc::now().to_rfc3339()
                    })))
                }
                Err(e) => {
                    tracing::error!("Failed to resend verification email: {}", e);
                    Err(e)
                }
            }
        }
        None => {
            tracing::warn!(
                "Resend verification requested for non-existent email: {}",
                req.email
            );
            Err(AuthError::UserNotFound)
        }
    }
}

// Forgot password handler
pub async fn forgot_password(
    req: web::Json<ForgotPasswordRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    tracing::debug!("Forgot password request for email: {}", req.email);

    email_service
        .enforce_password_reset_rate_limit(&req.email)
        .await?;

    // Try to find user by email
    if let Ok(Some(user)) = user_repo.find_by_email(&req.email).await {
        // Check if user has password_hash
        if user.password_hash.is_none() {
            // OAuth-only user - return same success message
            tracing::debug!(
                "Password reset requested for OAuth-only user: {}",
                req.email
            );
            return Ok(HttpResponse::Ok().json(json!({
                "message": "If an account exists with this email, a password reset code has been sent.",
                "timestamp": chrono::Utc::now().to_rfc3339()
            })));
        }

        // User exists and has password - send OTP
        email_service
            .send_password_reset_otp(&req.email, &user.username)
            .await?;
    } else {
        // User not exist
        tracing::debug!(
            "Password reset requested for non-existent email: {}",
            req.email
        );
    }

    // Always return same success message regardless of email existence
    Ok(HttpResponse::Ok().json(json!({
        "message": "If an account exists with this email, a password reset code has been sent.",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

// Verify reset OTP handler
pub async fn verify_reset_otp(
    req: web::Json<VerifyResetOtpRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    tracing::debug!("Verify reset OTP request for email: {}", req.email);

    // Find user by email
    let user = user_repo
        .find_by_email(&req.email)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    // Check if user has password_hash
    if user.password_hash.is_none() {
        return Err(AuthError::Validation(
            "Password reset is not available for OAuth-only accounts".to_string(),
        ));
    }

    // Verify OTP
    email_service
        .verify_password_reset_otp(&req.email, &req.otp)
        .await?;

    // Generate reset token
    let reset_token = email_service.generate_reset_token();

    // Store reset token with user_id
    email_service
        .store_reset_token(&reset_token, user.user_id)
        .await?;

    Ok(HttpResponse::Ok().json(json!({
        "reset_token": reset_token,
        "message": "OTP verified successfully. Use the reset_token to set your new password.",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

// Reset password handler
pub async fn reset_password(
    req: web::Json<ResetPasswordRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    token_service: web::Data<Arc<TokenService>>,
    session_service: web::Data<Arc<SessionService>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    tracing::debug!("Reset password request");

    // Validate reset token and get user_id
    let user_id = email_service.validate_reset_token(&req.reset_token).await?;

    // Find user
    let user = user_repo
        .find_by_id(user_id)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    // Verify user has password_hash
    if user.password_hash.is_none() {
        return Err(AuthError::Validation(
            "Password reset is not available for OAuth-only accounts".to_string(),
        ));
    }

    // Hash new password
    let new_password_hash = tokio::task::spawn_blocking({
        let p = req.new_password.clone();
        move || hash(&p, DEFAULT_COST)
    })
    .await
    .map_err(|e| AuthError::Validation(format!("Join error: {}", e)))?
    .map_err(|e| AuthError::Validation(format!("Password hashing failed: {}", e)))?;

    // Update password in database
    user_repo
        .update_password(user_id, &new_password_hash)
        .await?;

    // Invalidate all previously issued access and refresh tokens.
    token_service.bump_token_version(user_id).await?;

    // Revoke all user sessions
    if let Err(e) = session_service.revoke_all_user_sessions(user_id).await {
        tracing::error!(
            "Failed to revoke sessions after password reset for user {}: {}",
            user_id,
            e
        );
        return Err(AuthError::Redis(
            "Failed to revoke sessions; please retry".to_string(),
        ));
    }

    tracing::info!("Password reset successful for user_id: {}", user_id);

    Ok(HttpResponse::Ok().json(json!({
        "message": "Password reset successfully. Please login with your new password.",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::JwtConfig;
    use crate::dto::jwt_response_dto::JwtTokenResponse;
    use jsonwebtoken::{DecodingKey, EncodingKey};
    use redis::AsyncCommands;

    #[test]
    fn session_cookie_is_secure_exactly_when_the_request_used_https() {
        let forwarded_https = || {
            actix_web::test::TestRequest::default()
                .peer_addr("10.0.0.2:12345".parse().unwrap())
                .insert_header(("X-Forwarded-Proto", "https"))
        };
        let load_balancer = || {
            actix_web::web::Data::new(
                lib_middleware::TrustedProxies::parse(&["10.0.0.0/8"]).unwrap(),
            )
        };

        // Behind the load balancer TLS ends there, and its X-Forwarded-Proto carries the scheme.
        let behind_tls = forwarded_https()
            .app_data(load_balancer())
            .to_http_request();
        assert!(get_cookie_config(&behind_tls).0);

        // Any other peer's header is ignored: this plain-HTTP request did not use HTTPS.
        let untrusted = forwarded_https().to_http_request();
        assert!(!get_cookie_config(&untrusted).0);
        let direct = actix_web::test::TestRequest::default()
            .peer_addr("192.0.2.10:12345".parse().unwrap())
            .insert_header(("X-Forwarded-Proto", "https"))
            .app_data(load_balancer())
            .to_http_request();
        assert!(!get_cookie_config(&direct).0);

        let plain = actix_web::test::TestRequest::default().to_http_request();
        assert!(!get_cookie_config(&plain).0);
    }

    #[tokio::test]
    async fn refresh_token_stops_working_after_logout() {
        let user_id = Uuid::new_v4();
        let refresh_token = Uuid::new_v4().to_string();
        let redis_conn = crate::test_redis().await;
        let jwt_config = Arc::new(JwtConfig {
            private_key_path: String::new(),
            public_key_path: String::new(),
            access_token_ttl_minutes: 60,
            refresh_token_ttl_days: 30,
            issuer: "vex-gateway".to_string(),
            audience: "vex-api".to_string(),
        });
        let token_service = Arc::new(TokenService::new(
            jwt_config,
            redis_conn.clone(),
            Arc::new(EncodingKey::from_secret(b"unused")),
            Arc::new(DecodingKey::from_secret(b"unused")),
        ));
        let session_service = Arc::new(SessionService::new(redis_conn.clone(), 3600));

        let mut conn = redis_conn;
        let _: () = conn
            .set_ex(
                format!("refresh:{}", refresh_token),
                format!("{}:{}:0", user_id, Uuid::new_v4()),
                3600,
            )
            .await
            .unwrap();

        let response = logout(
            actix_web::test::TestRequest::default().to_http_request(),
            AuthenticatedUser { user_id },
            web::Data::new(token_service.clone()),
            web::Data::new(session_service),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), actix_web::http::StatusCode::OK);

        let result = token_service.refresh_tokens(&refresh_token).await;
        assert!(matches!(result, Err(AuthError::TokenRevoked)));
    }

    use crate::config::{EmailConfig, OtpConfig};
    use async_trait::async_trait;
    use lib_middleware::TrustedProxies;
    use rsa::{
        Generate, PrivateKey,
        pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey, LineEnding},
    };
    use secrecy::SecretString;
    use std::sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    };

    struct RejectIfCalledRepository {
        find_by_email_calls: AtomicUsize,
    }

    /// Serves a STALE user on `find_by_email` and the CURRENT one on `find_by_id`, modelling a
    /// password reset that commits between the two reads.
    struct RereadRepository {
        stale: Model,
        current: Model,
    }

    #[async_trait]
    impl UserRepository for RereadRepository {
        async fn find_by_id(&self, _user_id: Uuid) -> Result<Option<Model>, AuthError> {
            Ok(Some(self.current.clone()))
        }

        async fn find_by_email(&self, _email: &str) -> Result<Option<Model>, AuthError> {
            Ok(Some(self.stale.clone()))
        }

        async fn find_by_provider_id(
            &self,
            _provider: &str,
            _provider_id: &str,
        ) -> Result<Option<Model>, AuthError> {
            unreachable!()
        }

        async fn create_user(&self, _user: &Model) -> Result<Model, AuthError> {
            unreachable!()
        }

        async fn update_last_login(&self, _user_id: Uuid) -> Result<(), AuthError> {
            Ok(())
        }

        async fn verify_email(&self, _user_id: Uuid) -> Result<(), AuthError> {
            unreachable!()
        }

        async fn update_password(
            &self,
            _user_id: Uuid,
            _password_hash: &str,
        ) -> Result<(), AuthError> {
            unreachable!()
        }
    }

    struct LoginRepository {
        user: Model,
        find_by_email_calls: AtomicUsize,
    }

    #[async_trait]
    impl UserRepository for RejectIfCalledRepository {
        async fn find_by_id(&self, _user_id: Uuid) -> Result<Option<Model>, AuthError> {
            unreachable!()
        }

        async fn find_by_email(&self, _email: &str) -> Result<Option<Model>, AuthError> {
            self.find_by_email_calls.fetch_add(1, Ordering::SeqCst);
            Err(AuthError::InternalError(
                "repository must not be reached".to_string(),
            ))
        }

        async fn find_by_provider_id(
            &self,
            _provider: &str,
            _provider_id: &str,
        ) -> Result<Option<Model>, AuthError> {
            unreachable!()
        }

        async fn create_user(&self, _user: &Model) -> Result<Model, AuthError> {
            unreachable!()
        }

        async fn update_last_login(&self, _user_id: Uuid) -> Result<(), AuthError> {
            unreachable!()
        }

        async fn verify_email(&self, _user_id: Uuid) -> Result<(), AuthError> {
            unreachable!()
        }

        async fn update_password(
            &self,
            _user_id: Uuid,
            _password_hash: &str,
        ) -> Result<(), AuthError> {
            unreachable!()
        }
    }

    #[async_trait]
    impl UserRepository for LoginRepository {
        async fn find_by_id(&self, _user_id: Uuid) -> Result<Option<Model>, AuthError> {
            // The login path re-reads the user after capturing token_version, so that the hash it
            // verifies is never older than the version it mints at (finding G3-1).
            Ok(Some(self.user.clone()))
        }

        async fn find_by_email(&self, _email: &str) -> Result<Option<Model>, AuthError> {
            self.find_by_email_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(self.user.clone()))
        }

        async fn find_by_provider_id(
            &self,
            _provider: &str,
            _provider_id: &str,
        ) -> Result<Option<Model>, AuthError> {
            unreachable!()
        }

        async fn create_user(&self, _user: &Model) -> Result<Model, AuthError> {
            unreachable!()
        }

        async fn update_last_login(&self, _user_id: Uuid) -> Result<(), AuthError> {
            Ok(())
        }

        async fn verify_email(&self, _user_id: Uuid) -> Result<(), AuthError> {
            unreachable!()
        }

        async fn update_password(
            &self,
            _user_id: Uuid,
            _password_hash: &str,
        ) -> Result<(), AuthError> {
            unreachable!()
        }
    }

    fn login_user(email: String, password: &str) -> Model {
        let now = Utc::now();
        Model {
            user_id: Uuid::new_v4(),
            email,
            username: format!("user-{}", Uuid::new_v4()),
            password_hash: Some(hash(password, 4).unwrap()),
            auth_provider: "email".to_string(),
            provider_id: None,
            avatar_url: None,
            email_verified: true,
            status: UserStatus::Active,
            created_at: now,
            updated_at: now,
            last_login: None,
        }
    }

    fn test_rsa_keys() -> (Arc<EncodingKey>, Arc<DecodingKey>) {
        static KEYS: OnceLock<(Arc<EncodingKey>, Arc<DecodingKey>)> = OnceLock::new();

        KEYS.get_or_init(|| {
            let private_key = PrivateKey::generate();
            let public_key = private_key.to_public_key();
            let private_pem = private_key.to_pkcs1_pem(LineEnding::LF).unwrap();
            let public_pem = public_key.to_pkcs1_pem(LineEnding::LF).unwrap();

            (
                Arc::new(EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap()),
                Arc::new(DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap()),
            )
        })
        .clone()
    }

    async fn test_services() -> (Arc<EmailService>, Arc<TokenService>, Arc<SessionService>) {
        let conn = crate::test_redis().await;
        let email_config = EmailConfig {
            smtp_host: "127.0.0.1".to_string(),
            smtp_port: 2525,
            smtp_username: "test".to_string(),
            smtp_password: SecretString::new("test".to_string()),
            from_email: "test@example.com".to_string(),
        };
        let otp_config = OtpConfig {
            otp_length: 6,
            email_verification_expiry_minutes: 15,
            password_reset_expiry_minutes: 15,
        };
        let email_service =
            Arc::new(EmailService::new(&email_config, conn.clone(), &otp_config).unwrap());
        let jwt_config = Arc::new(JwtConfig {
            private_key_path: String::new(),
            public_key_path: String::new(),
            access_token_ttl_minutes: 15,
            refresh_token_ttl_days: 1,
            issuer: "test".to_string(),
            audience: "test".to_string(),
        });
        let (encoding_key, decoding_key) = test_rsa_keys();
        let token_service = Arc::new(TokenService::new(
            jwt_config,
            conn.clone(),
            encoding_key,
            decoding_key,
        ));
        let session_service = Arc::new(SessionService::new(conn, 60));

        (email_service, token_service, session_service)
    }

    async fn attempt_login(
        email: &str,
        password: &str,
        peer_addr: &str,
        user_repo: Arc<dyn UserRepository>,
        email_service: Arc<EmailService>,
        token_service: Arc<TokenService>,
        session_service: Arc<SessionService>,
    ) -> Result<HttpResponse, AuthError> {
        login(
            actix_web::test::TestRequest::default()
                .peer_addr(peer_addr.parse().unwrap())
                .to_http_request(),
            web::Json(LoginRequest {
                email: email.to_string(),
                password: password.to_string(),
            }),
            web::Data::new(user_repo),
            web::Data::new(token_service),
            web::Data::new(session_service),
            web::Data::new(email_service),
        )
        .await
    }

    #[tokio::test]
    async fn repeated_successful_logins_do_not_trip_principal_limit() {
        const PASSWORD: &str = "correct-horse-battery-staple";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        let repository = Arc::new(LoginRepository {
            user: login_user(email.clone(), PASSWORD),
            find_by_email_calls: AtomicUsize::new(0),
        });
        let user_repo: Arc<dyn UserRepository> = repository.clone();

        for _ in 0..=LOGIN_PRINCIPAL_MAX_REQUESTS {
            let response = attempt_login(
                &email,
                PASSWORD,
                "198.51.100.10:12345",
                user_repo.clone(),
                email_service.clone(),
                token_service.clone(),
                session_service.clone(),
            )
            .await
            .unwrap();
            assert!(response.status().is_success());
        }

        assert_eq!(
            repository.find_by_email_calls.load(Ordering::SeqCst),
            (LOGIN_PRINCIPAL_MAX_REQUESTS + 1) as usize
        );
    }

    #[tokio::test]
    async fn consecutive_login_failures_trip_principal_limit() {
        const PASSWORD: &str = "correct-horse-battery-staple";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        let repository = Arc::new(LoginRepository {
            user: login_user(email.clone(), PASSWORD),
            find_by_email_calls: AtomicUsize::new(0),
        });
        let user_repo: Arc<dyn UserRepository> = repository.clone();

        for _ in 0..LOGIN_PRINCIPAL_MAX_REQUESTS {
            let result = attempt_login(
                &email,
                "incorrect-password",
                "198.51.100.11:12345",
                user_repo.clone(),
                email_service.clone(),
                token_service.clone(),
                session_service.clone(),
            )
            .await;
            assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        }

        let result = attempt_login(
            &email,
            PASSWORD,
            "198.51.100.11:12345",
            user_repo,
            email_service,
            token_service,
            session_service,
        )
        .await;
        assert!(matches!(result, Err(AuthError::RateLimited { .. })));
        assert_eq!(
            repository.find_by_email_calls.load(Ordering::SeqCst),
            LOGIN_PRINCIPAL_MAX_REQUESTS as usize
        );
    }

    #[tokio::test]
    async fn successful_login_resets_prior_failures() {
        const PASSWORD: &str = "correct-horse-battery-staple";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        let repository = Arc::new(LoginRepository {
            user: login_user(email.clone(), PASSWORD),
            find_by_email_calls: AtomicUsize::new(0),
        });
        let user_repo: Arc<dyn UserRepository> = repository;

        for _ in 0..LOGIN_PRINCIPAL_MAX_REQUESTS - 1 {
            let result = attempt_login(
                &email,
                "incorrect-password",
                "198.51.100.12:12345",
                user_repo.clone(),
                email_service.clone(),
                token_service.clone(),
                session_service.clone(),
            )
            .await;
            assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        }

        let response = attempt_login(
            &email,
            PASSWORD,
            "198.51.100.12:12345",
            user_repo.clone(),
            email_service.clone(),
            token_service.clone(),
            session_service.clone(),
        )
        .await
        .unwrap();
        assert!(response.status().is_success());

        for _ in 0..LOGIN_PRINCIPAL_MAX_REQUESTS {
            let result = attempt_login(
                &email,
                "incorrect-password",
                "198.51.100.12:12345",
                user_repo.clone(),
                email_service.clone(),
                token_service.clone(),
                session_service.clone(),
            )
            .await;
            assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        }

        let result = attempt_login(
            &email,
            PASSWORD,
            "198.51.100.12:12345",
            user_repo,
            email_service,
            token_service,
            session_service,
        )
        .await;
        assert!(matches!(result, Err(AuthError::RateLimited { .. })));
    }

    #[tokio::test]
    async fn rejected_login_does_not_reach_password_verification() {
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        // The failure budget is keyed on (email, peer IP) so an attacker cannot lock a victim
        // out from another address (finding G3-2); seed the same composite key the handler uses.
        let principal = format!("email:{}|ip:127.0.0.1", normalized_principal(&email));
        for _ in 0..LOGIN_PRINCIPAL_MAX_REQUESTS {
            email_service
                .record_login_authentication(&principal, false, LOGIN_PRINCIPAL_WINDOW_SECONDS)
                .await
                .unwrap();
        }

        let repository = Arc::new(RejectIfCalledRepository {
            find_by_email_calls: AtomicUsize::new(0),
        });
        let user_repo: Arc<dyn UserRepository> = repository.clone();
        let request = actix_web::test::TestRequest::default()
            .peer_addr("127.0.0.1:12345".parse().unwrap())
            .to_http_request();
        let result = login(
            request,
            web::Json(LoginRequest {
                email,
                password: "correct-horse-battery-staple".to_string(),
            }),
            web::Data::new(user_repo),
            web::Data::new(token_service),
            web::Data::new(session_service),
            web::Data::new(email_service),
        )
        .await;

        assert!(matches!(result, Err(AuthError::RateLimited { .. })));
        assert_eq!(repository.find_by_email_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn login_verifies_the_password_hash_captured_after_the_token_version() {
        // Finding G3-1. The login path reads the user, captures token_version, then re-reads the
        // user so the hash it verifies is never OLDER than the version it will mint at. This
        // asserts the re-read actually happens and is what gets verified: the repository serves a
        // STALE hash on the first read (the old password) and the current hash on the re-read, so
        // a login with the old password must fail. Without the re-read it would succeed, and the
        // token would be minted at the new version and survive the revocation.
        const OLD_PASSWORD: &str = "old-correct-horse";
        const NEW_PASSWORD: &str = "new-correct-horse";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());

        let repository = Arc::new(RereadRepository {
            stale: login_user(email.clone(), OLD_PASSWORD),
            current: login_user(email.clone(), NEW_PASSWORD),
        });
        let user_repo: Arc<dyn UserRepository> = repository.clone();

        let result = attempt_login(
            &email,
            OLD_PASSWORD,
            "198.51.100.21:12345",
            user_repo,
            email_service.clone(),
            token_service,
            session_service,
        )
        .await;

        assert!(
            matches!(result, Err(AuthError::InvalidCredentials)),
            "the password must be checked against the re-read hash, not the first read; got {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn refused_logins_are_counted_as_login_authentication_failures() {
        const PASSWORD: &str = "correct-horse-battery-staple";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        let user_repo: Arc<dyn UserRepository> = Arc::new(LoginRepository {
            user: login_user(email.clone(), PASSWORD),
            find_by_email_calls: AtomicUsize::new(0),
        });
        let failures = metrics::auth_failures(AuthMethod::Login);
        let before = failures.get();

        let result = attempt_login(
            &email,
            "incorrect-password",
            "198.51.100.30:12345",
            user_repo,
            email_service,
            token_service,
            session_service,
        )
        .await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        // The other tests here refuse logins concurrently, so the count can rise by more.
        assert!(failures.get() > before);
    }

    #[tokio::test]
    async fn exhausted_budget_for_one_ip_does_not_lock_out_another_ip() {
        // Finding G3-2: with a principal-only failure bucket, an unauthenticated attacker could
        // burn a victim's budget from anywhere and lock them out of their own account, because
        // the success path that clears the bucket is unreachable while locked. Keying on
        // (email, peer IP) means the attacker only ever locks out themselves.
        const PASSWORD: &str = "correct-horse-battery-staple";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());

        let repository = Arc::new(LoginRepository {
            user: login_user(email.clone(), PASSWORD),
            find_by_email_calls: AtomicUsize::new(0),
        });

        // Drive the attacker's failures THROUGH the handler, so the handler's own keying is what
        // is under test. Seeding a bucket directly would still pass if the key reverted to email.
        for _ in 0..LOGIN_PRINCIPAL_MAX_REQUESTS {
            let user_repo: Arc<dyn UserRepository> = repository.clone();
            let _ = attempt_login(
                &email,
                "wrong-password",
                "203.0.113.9:12345",
                user_repo,
                email_service.clone(),
                token_service.clone(),
                session_service.clone(),
            )
            .await;
        }

        // The victim, from their own address with the correct password, must actually SUCCEED --
        // not merely avoid a rate-limit error.
        let user_repo: Arc<dyn UserRepository> = repository.clone();
        let result = attempt_login(
            &email,
            PASSWORD,
            "198.51.100.77:12345",
            user_repo,
            email_service.clone(),
            token_service,
            session_service,
        )
        .await;

        assert!(
            result.is_ok(),
            "the victim must still be able to log in from their own address; got {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn clients_behind_a_trusted_proxy_get_separate_failure_budgets() {
        // G96-t0 behind a load balancer: every client arrives from the proxy's address, so a
        // budget keyed on the TCP peer collapses to per-email and the lockout DoS returns. With
        // the proxy trusted, the key uses the client address the proxy forwarded.
        const PASSWORD: &str = "correct-horse-battery-staple";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        let repository = Arc::new(LoginRepository {
            user: login_user(email.clone(), PASSWORD),
            find_by_email_calls: AtomicUsize::new(0),
        });
        let trusted_proxies = web::Data::new(TrustedProxies::parse(&["10.0.0.0/8"]).unwrap());

        let mut attempts =
            vec![("203.0.113.9", "wrong-password"); LOGIN_PRINCIPAL_MAX_REQUESTS as usize + 1];
        attempts.push(("198.51.100.77", PASSWORD));
        let mut results = Vec::new();
        for (forwarded_client, password) in attempts {
            let user_repo: Arc<dyn UserRepository> = repository.clone();
            results.push(
                login(
                    actix_web::test::TestRequest::default()
                        .peer_addr("10.0.0.2:443".parse().unwrap())
                        .insert_header(("X-Forwarded-For", forwarded_client))
                        .app_data(trusted_proxies.clone())
                        .to_http_request(),
                    web::Json(LoginRequest {
                        email: email.clone(),
                        password: password.to_string(),
                    }),
                    web::Data::new(user_repo),
                    web::Data::new(token_service.clone()),
                    web::Data::new(session_service.clone()),
                    web::Data::new(email_service.clone()),
                )
                .await,
            );
        }

        let victim = results.pop().unwrap();
        let attacker_over_budget = results.pop().unwrap();
        assert!(
            matches!(attacker_over_budget, Err(AuthError::RateLimited { .. })),
            "the attacker's own forwarded address must exhaust its budget; got {:?}",
            attacker_over_budget.err()
        );
        assert!(
            victim.is_ok(),
            "a different client behind the same proxy must still log in; got {:?}",
            victim.err()
        );
    }

    #[tokio::test]
    async fn an_ipv6_client_has_one_failure_budget_for_its_whole_64() {
        // An IPv6 client is usually given a /64 and can send from any address in it: a budget per
        // address would give it a fresh one with each.
        const PASSWORD: &str = "correct-horse-battery-staple";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        let repository = Arc::new(LoginRepository {
            user: login_user(email.clone(), PASSWORD),
            find_by_email_calls: AtomicUsize::new(0),
        });

        let mut results = Vec::new();
        for host in 1..=LOGIN_PRINCIPAL_MAX_REQUESTS + 1 {
            let user_repo: Arc<dyn UserRepository> = repository.clone();
            results.push(
                attempt_login(
                    &email,
                    "wrong-password",
                    &format!("[2001:db8:113:64::{host:x}]:12345"),
                    user_repo,
                    email_service.clone(),
                    token_service.clone(),
                    session_service.clone(),
                )
                .await,
            );
        }

        let over_budget = results.pop().unwrap();
        assert!(
            matches!(over_budget, Err(AuthError::RateLimited { .. })),
            "a new address in the same /64 must not get a new budget; got {:?}",
            over_budget.err()
        );
    }

    #[tokio::test]
    async fn occupied_login_inflight_lock_rejects_password_verification() {
        const PASSWORD: &str = "correct-horse-battery-staple";
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        // The failure budget is keyed on (email, peer IP) so an attacker cannot lock a victim
        // out from another address (finding G3-2); seed the same composite key the handler uses.
        // Seed the lock under the SAME peer IP the attempt below uses: the principal is now
        // (email, peer IP), so a lock held for a different address must not block this one.
        let principal = format!("email:{}|ip:198.51.100.13", normalized_principal(&email));
        let lock_token = email_service
            .acquire_login_inflight_lock(&principal)
            .await
            .unwrap();
        let repository = Arc::new(LoginRepository {
            user: login_user(email.clone(), PASSWORD),
            find_by_email_calls: AtomicUsize::new(0),
        });
        let user_repo: Arc<dyn UserRepository> = repository.clone();

        let result = attempt_login(
            &email,
            "incorrect-password",
            "198.51.100.13:12345",
            user_repo,
            email_service.clone(),
            token_service,
            session_service,
        )
        .await;

        email_service
            .release_login_inflight_lock(&principal, &lock_token)
            .await
            .unwrap();
        assert!(matches!(result, Err(AuthError::RateLimited { .. })));
        assert_eq!(repository.find_by_email_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn signup_validation_precedes_principal_rate_limits() {
        let (email_service, _, _) = test_services().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        let username = format!("{}{}", Uuid::new_v4(), "x".repeat(101));
        let repository = Arc::new(RejectIfCalledRepository {
            find_by_email_calls: AtomicUsize::new(0),
        });
        let user_repo: Arc<dyn UserRepository> = repository.clone();

        let result = signup(
            actix_web::test::TestRequest::default()
                .peer_addr("203.0.113.201:12345".parse().unwrap())
                .to_http_request(),
            web::Json(SignupRequest {
                email: email.clone(),
                username: username.clone(),
                password: "correct-horse-battery-staple".to_string(),
            }),
            web::Data::new(user_repo),
            web::Data::new(email_service),
        )
        .await;

        assert!(matches!(result, Err(AuthError::Validation(_))));
        assert_eq!(repository.find_by_email_calls.load(Ordering::SeqCst), 0);

        let mut conn = crate::test_redis().await;
        let ip_key_exists: bool = conn
            .exists("rate_limit:auth:signup:ip:203.0.113.201")
            .await
            .unwrap();
        let email_key_exists: bool = conn
            .exists(format!("rate_limit:auth:signup:email:{}", email))
            .await
            .unwrap();
        let username_key_exists: bool = conn
            .exists(format!("rate_limit:auth:signup:username:{}", username))
            .await
            .unwrap();

        assert!(ip_key_exists);
        assert!(!email_key_exists);
        assert!(!username_key_exists);
    }

    #[tokio::test]
    async fn verify_email_validation_precedes_principal_rate_limit() {
        let (email_service, token_service, session_service) = test_services().await;
        let email = format!("{}{}@example.com", Uuid::new_v4().simple(), "x".repeat(40));
        let repository = Arc::new(RejectIfCalledRepository {
            find_by_email_calls: AtomicUsize::new(0),
        });
        let user_repo: Arc<dyn UserRepository> = repository.clone();

        let result = verify_email_with_otp(
            actix_web::test::TestRequest::default()
                .peer_addr("203.0.113.202:12345".parse().unwrap())
                .to_http_request(),
            web::Json(VerifyEmailRequest {
                email: email.clone(),
                otp: "123456".to_string(),
            }),
            web::Data::new(user_repo),
            web::Data::new(email_service),
            web::Data::new(token_service),
            web::Data::new(session_service),
        )
        .await;

        assert!(matches!(result, Err(AuthError::Validation(_))));
        assert_eq!(repository.find_by_email_calls.load(Ordering::SeqCst), 0);

        let mut conn = crate::test_redis().await;
        let ip_key_exists: bool = conn
            .exists("rate_limit:auth:verify_email:ip:203.0.113.202")
            .await
            .unwrap();
        let email_key_exists: bool = conn
            .exists(format!("rate_limit:auth:verify_email:email:{}", email))
            .await
            .unwrap();

        assert!(ip_key_exists);
        assert!(!email_key_exists);
    }

    #[tokio::test]
    async fn resend_verification_is_limited_per_email_before_the_lookup() {
        let (email_service, _, _) = test_services().await;
        let repository = Arc::new(RejectIfCalledRepository {
            find_by_email_calls: AtomicUsize::new(0),
        });
        let resend = |email: &str| {
            let user_repo: Arc<dyn UserRepository> = repository.clone();
            resend_verification_email(
                web::Json(CheckEmailRequest {
                    email: email.to_string(),
                }),
                web::Data::new(user_repo),
                web::Data::new(email_service.clone()),
            )
        };
        let email = format!("{}@example.com", Uuid::new_v4());

        // Three an hour per email reach the lookup, which this repository fails.
        for _ in 0..3 {
            assert!(matches!(
                resend(&email).await,
                Err(AuthError::InternalError(_))
            ));
        }
        assert!(matches!(
            resend(&email).await,
            Err(AuthError::RateLimited { .. })
        ));
        assert_eq!(repository.find_by_email_calls.load(Ordering::SeqCst), 3);

        // Another email has a limit of its own.
        let other_email = format!("{}@example.com", Uuid::new_v4());
        assert!(matches!(
            resend(&other_email).await,
            Err(AuthError::InternalError(_))
        ));
    }

    #[test]
    fn client_ip_key_uses_peer_address_not_forwarded_header() {
        let request = actix_web::test::TestRequest::default()
            .peer_addr("192.0.2.10:12345".parse().unwrap())
            .insert_header(("X-Forwarded-For", "198.51.100.7"))
            .to_http_request();

        assert_eq!(client_ip_key(&request), "ip:192.0.2.10");
    }

    /// Serves one password user and accepts its password update.
    struct PasswordRepository {
        user: Model,
    }

    #[async_trait]
    impl UserRepository for PasswordRepository {
        async fn find_by_id(&self, _user_id: Uuid) -> Result<Option<Model>, AuthError> {
            Ok(Some(self.user.clone()))
        }

        async fn find_by_email(&self, _email: &str) -> Result<Option<Model>, AuthError> {
            unreachable!()
        }

        async fn find_by_provider_id(
            &self,
            _provider: &str,
            _provider_id: &str,
        ) -> Result<Option<Model>, AuthError> {
            unreachable!()
        }

        async fn create_user(&self, _user: &Model) -> Result<Model, AuthError> {
            unreachable!()
        }

        async fn update_last_login(&self, _user_id: Uuid) -> Result<(), AuthError> {
            unreachable!()
        }

        async fn verify_email(&self, _user_id: Uuid) -> Result<(), AuthError> {
            unreachable!()
        }

        async fn update_password(
            &self,
            _user_id: Uuid,
            _password_hash: &str,
        ) -> Result<(), AuthError> {
            Ok(())
        }
    }

    /// One signed-in device: its session and the tokens minted for it.
    async fn sign_in(
        token_service: &TokenService,
        session_service: &SessionService,
        user_id: Uuid,
    ) -> (String, JwtTokenResponse) {
        let tokens = token_service.mint_tokens(user_id).await.unwrap();
        let session_id = session_service
            .create_session(
                user_id,
                tokens.access_token.clone(),
                tokens.refresh_token.clone(),
            )
            .await
            .unwrap();
        (session_id, tokens)
    }

    /// Tokens minted before the version bump are refused: the refresh token by `refresh_tokens`,
    /// the access token by the version check every JWT request passes (jwt_auth.rs).
    async fn assert_tokens_revoked(
        token_service: &TokenService,
        user_id: Uuid,
        tokens: &JwtTokenResponse,
    ) {
        assert!(matches!(
            token_service.refresh_tokens(&tokens.refresh_token).await,
            Err(AuthError::TokenRevoked)
        ));
        let claims = token_service.validate_jwt(&tokens.access_token).unwrap();
        assert!(claims.token_version < token_service.get_token_version(user_id).await.unwrap());
    }

    #[tokio::test]
    async fn password_reset_revokes_every_session_and_outstanding_token() {
        let (email_service, token_service, session_service) = test_services().await;
        let user = login_user(
            format!("reset-{}@example.com", Uuid::new_v4()),
            "old-password",
        );
        let user_id = user.user_id;
        let devices = [
            sign_in(&token_service, &session_service, user_id).await,
            sign_in(&token_service, &session_service, user_id).await,
        ];
        let reset_token = email_service.generate_reset_token();
        email_service
            .store_reset_token(&reset_token, user_id)
            .await
            .unwrap();
        let user_repo: Arc<dyn UserRepository> = Arc::new(PasswordRepository { user });

        let response = reset_password(
            web::Json(ResetPasswordRequest {
                reset_token,
                new_password: "new-password".to_string(),
            }),
            web::Data::new(user_repo),
            web::Data::new(token_service.clone()),
            web::Data::new(session_service.clone()),
            web::Data::new(email_service),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), actix_web::http::StatusCode::OK);
        for (session_id, tokens) in &devices {
            assert!(session_service.get_session(session_id).await.is_err());
            assert_tokens_revoked(&token_service, user_id, tokens).await;
        }
    }

    #[tokio::test]
    async fn password_change_without_a_session_cookie_revokes_every_session_and_token() {
        let (_, token_service, session_service) = test_services().await;
        let user = login_user(
            format!("change-{}@example.com", Uuid::new_v4()),
            "old-password",
        );
        let user_id = user.user_id;
        let devices = [
            sign_in(&token_service, &session_service, user_id).await,
            sign_in(&token_service, &session_service, user_id).await,
        ];
        let user_repo: Arc<dyn UserRepository> = Arc::new(PasswordRepository { user });

        let response = change_password(
            actix_web::test::TestRequest::default().to_http_request(),
            AuthenticatedUser { user_id },
            web::Json(ChangePasswordRequest {
                old_password: "old-password".to_string(),
                new_password: "new-password".to_string(),
            }),
            web::Data::new(user_repo),
            web::Data::new(token_service.clone()),
            web::Data::new(session_service.clone()),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), actix_web::http::StatusCode::OK);
        for (session_id, tokens) in &devices {
            assert!(session_service.get_session(session_id).await.is_err());
            assert_tokens_revoked(&token_service, user_id, tokens).await;
        }
    }

    #[tokio::test]
    async fn password_change_keeps_only_the_current_session_on_fresh_tokens() {
        let (_, token_service, session_service) = test_services().await;
        let user = login_user(
            format!("change-{}@example.com", Uuid::new_v4()),
            "old-password",
        );
        let user_id = user.user_id;
        let (current_session, current_tokens) =
            sign_in(&token_service, &session_service, user_id).await;
        let (other_session, other_tokens) =
            sign_in(&token_service, &session_service, user_id).await;
        let user_repo: Arc<dyn UserRepository> = Arc::new(PasswordRepository { user });

        let response = change_password(
            actix_web::test::TestRequest::default()
                .cookie(actix_web::cookie::Cookie::new(
                    "session_id",
                    current_session.clone(),
                ))
                .to_http_request(),
            AuthenticatedUser { user_id },
            web::Json(ChangePasswordRequest {
                old_password: "old-password".to_string(),
                new_password: "new-password".to_string(),
            }),
            web::Data::new(user_repo),
            web::Data::new(token_service.clone()),
            web::Data::new(session_service.clone()),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), actix_web::http::StatusCode::OK);
        // The device that changed the password stays signed in on tokens minted after the bump.
        let kept = session_service.get_session(&current_session).await.unwrap();
        assert!(
            token_service
                .refresh_tokens(&kept.refresh_token)
                .await
                .is_ok()
        );
        // The other device, and every token minted before the change, is revoked.
        assert!(session_service.get_session(&other_session).await.is_err());
        assert_tokens_revoked(&token_service, user_id, &current_tokens).await;
        assert_tokens_revoked(&token_service, user_id, &other_tokens).await;
    }
}

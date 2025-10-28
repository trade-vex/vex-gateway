use actix_web::{HttpRequest, HttpResponse, web};
use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
use validator::Validate;

use crate::dto::auth_request_dto::{
    ChangePasswordRequest, CheckEmailRequest, LoginRequest, RefreshTokenRequest, SignupRequest, VerifyEmailRequest,
};
use crate::dto::auth_response_dto::{
    RefreshTokenResponse, SignupResponse, UserInfo, UserProfile,
};
use crate::error::auth_error::AuthError;
use crate::middleware::jwt_auth::AuthenticatedUser;
use crate::models::user::{Model, UserStatus};
use crate::repository::UserRepository;
use crate::service::{EmailService, SessionService, TokenService};
use std::sync::Arc;

// Signup handler
pub async fn signup(
    req: web::Json<SignupRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

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
    req: web::Json<LoginRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    token_service: web::Data<Arc<TokenService>>,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    tracing::debug!("Login request");

    // Find user by email
    let user = user_repo
        .find_by_email(&req.email)
        .await?
        .ok_or(AuthError::InvalidCredentials)?;

    // Check if user can login
    if !user.can_login() {
        return Err(AuthError::EmailNotVerified);
    }

    // Verify password
    let password_hash = user
        .password_hash
        .clone()
        .ok_or(AuthError::InvalidCredentials)?;

    if !tokio::task::spawn_blocking({
        let p = req.password.clone();
        let h = password_hash.clone();
        move || verify(&p, &h)
    })
    .await
    .map_err(|e| AuthError::Validation(format!("Join error: {}", e)))?
    .map_err(|e| AuthError::Validation(format!("Password verification failed: {}", e)))?
    {
        return Err(AuthError::InvalidCredentials);
    }

    let user_id = user.user_id;

    // Generate tokens
    let tokens = token_service.mint_tokens(user_id).await?;

    // Update last login
    user_repo.update_last_login(user_id).await?;

    // Create session (store user_id and tokens in Redis)
    let session_id = session_service
        .create_session(user_id, tokens.access_token.clone(), tokens.refresh_token.clone())
        .await?;

    // Create session cookie
    let is_secure = std::env::var("FRONTEND_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string())
        .starts_with("https://");

    let cookie_ttl = 24 * 3600;

    let session_cookie = actix_web::cookie::Cookie::build("session_id", session_id)
        .path("/")
        .http_only(true)
        .secure(is_secure)
        .same_site(actix_web::cookie::SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(cookie_ttl))
        .finish();

    Ok(HttpResponse::Ok()
        .cookie(session_cookie)
        .json(json!({
            "message": "Logged in successfully"
        })))
}

// Logout handler
pub async fn logout(
    req: HttpRequest,
    token_service: web::Data<Arc<TokenService>>,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    // Try to extract session ID from cookie
    if let Some(cookie) = req.cookie("session_id") {
        // Delete session from Redis
        if let Err(e) = session_service.delete_session(cookie.value()).await {
            tracing::warn!("Failed to delete session during logout: {}", e);
        }
    }

    // Also handle JWT token if present
    if let Some(auth_header) = req.headers().get("Authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(stripped) = auth_str.strip_prefix("Bearer ") {
                if let Err(e) = token_service.blacklist_token(stripped).await {
                    tracing::warn!("Failed to blacklist token during logout: {}", e);
                }
            }
        }
    }

    // Create a cookie to delete the session cookie
    let is_secure = std::env::var("FRONTEND_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string())
        .starts_with("https://");

    let delete_cookie = actix_web::cookie::Cookie::build("session_id", "")
        .path("/")
        .http_only(true)
        .secure(is_secure)
        .same_site(actix_web::cookie::SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(0))
        .finish();

    Ok(HttpResponse::Ok()
        .cookie(delete_cookie)
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
    req: web::Json<VerifyEmailRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    email_service: web::Data<Arc<EmailService>>,
    token_service: web::Data<Arc<TokenService>>,
    session_service: web::Data<Arc<SessionService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

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

    // Get updated user info
    let _updated_user = user_repo
        .find_by_id(user.user_id)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    tracing::debug!("Creating session for user_id: {}", user.user_id);
    
    // Create session (stores user_id and tokens in Redis)
    let session_id = session_service
        .create_session(user.user_id, tokens.access_token.clone(), tokens.refresh_token.clone())
        .await
        .map_err(|e| {
            tracing::error!("Failed to create session: {}", e);
            AuthError::Redis(format!("Failed to create session: {}", e))
        })?;

    tracing::debug!("Session created successfully: {}", session_id);

    // Create session cookie
    let is_secure = std::env::var("FRONTEND_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string())
        .starts_with("https://");

    let cookie_ttl = 24 * 3600;

    let session_cookie = actix_web::cookie::Cookie::build("session_id", session_id)
        .path("/")
        .http_only(true)
        .secure(is_secure)
        .same_site(actix_web::cookie::SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(cookie_ttl))
        .finish();

    tracing::debug!("Session cookie created, sending response with cookie");

    // Return only success message (session created with cookie)
    Ok(HttpResponse::Ok()
        .cookie(session_cookie)
        .json(json!({
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
    user: AuthenticatedUser,
    req: web::Json<ChangePasswordRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    // Validate the request
    req.validate()
        .map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    // Get user
    let user_record = user_repo
        .find_by_id(user.user_id)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    // Verify old password
    if let Some(password_hash) = &user_record.password_hash {
        if !tokio::task::spawn_blocking({
            let p = req.old_password.clone();
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
        let p = req.new_password.clone();
        move || hash(&p, DEFAULT_COST)
    })
    .await
    .map_err(|e| AuthError::Validation(format!("Join error: {}", e)))?
    .map_err(|e| AuthError::Validation(format!("Password hashing failed: {}", e)))?;

    // Update password
    user_repo
        .update_password(user.user_id, &new_password_hash)
        .await?;

    // Revoke all existing sessions for this user
    token_service.revoke_user_sessions(user.user_id).await?;

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
        Some(user) => {
            Ok(HttpResponse::Ok().json(json!({
                "exists": true,
                "email_verified": user.email_verified
            })))
        }
        None => {
            Ok(HttpResponse::Ok().json(json!({
                "exists": false,
                "email_verified": false
            })))
        }
    }
}

// resend verification email handler
pub async fn resend_verification_email(
    req: web::Json<CheckEmailRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    email_service: web::Data<Arc<EmailService>>,
) -> Result<HttpResponse, AuthError> {
    // validate the request
    req.validate().map_err(|e| AuthError::Validation(format!("Validation failed: {}", e)))?;

    tracing::debug!("Resend verification email request for: {}", req.email);

    // check if user exists by email
    let user = user_repo.find_by_email(&req.email).await?;
    
    match user {
        Some(user_model) => {
            // checking if email is already verified
            if user_model.email_verified {
                return Err(AuthError::Validation("Email already verified".to_string()));
            }

            // resend verification email
            match email_service.resend_verification_email(&req.email, &user_model.username).await {
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
            tracing::warn!("Resend verification requested for non-existent email: {}", req.email);
            Err(AuthError::UserNotFound)
        }
    }
}

use actix_web::{HttpRequest, HttpResponse, web};
use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
use validator::Validate;

use crate::dto::auth_request_dto::{
    ChangePasswordRequest, LoginRequest, RefreshTokenRequest, SignupRequest,
};
use crate::dto::auth_response_dto::{LoginResponse, RefreshTokenResponse, UserInfo, UserProfile};
use crate::error::auth_error::AuthError;
use crate::middleware::jwt_auth::AuthenticatedUser;
use crate::models::user::{Model, UserStatus};
use crate::repository::UserRepository;
use crate::service::token_service::TokenService;
use std::sync::Arc;

// Signup handler
pub async fn signup(
    req: web::Json<SignupRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    token_service: web::Data<Arc<TokenService>>,
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
    let password_hash = hash(&req.password, DEFAULT_COST)
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

    // Generate tokens
    let tokens = token_service.mint_tokens(created_user.user_id).await?;

    // Update last login
    user_repo.update_last_login(created_user.user_id).await?;

    let user_info = UserInfo::from(created_user);
    let response = LoginResponse {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: tokens.expires_in,
        user: user_info,
    };

    Ok(HttpResponse::Created().json(response))
}

// Login handler
pub async fn login(
    req: web::Json<LoginRequest>,
    user_repo: web::Data<Arc<dyn UserRepository>>,
    token_service: web::Data<Arc<TokenService>>,
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

    if !verify(&req.password, &password_hash)
        .map_err(|e| AuthError::Validation(format!("Password verification failed: {}", e)))?
    {
        return Err(AuthError::InvalidCredentials);
    }

    let user_id = user.user_id;

    // Generate tokens
    let tokens = token_service.mint_tokens(user_id).await?;

    // Update last login
    user_repo.update_last_login(user_id).await?;

    let user_info = UserInfo::from(user);
    let response = LoginResponse {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: tokens.expires_in,
        user: user_info,
    };

    Ok(HttpResponse::Ok().json(response))
}

// Logout handler
pub async fn logout(
    req: HttpRequest,
    token_service: web::Data<Arc<TokenService>>,
) -> Result<HttpResponse, AuthError> {
    // Extract JWT token from Authorization header
    let token = match req.headers().get("Authorization") {
        Some(auth_header) => {
            let auth_str = auth_header
                .to_str()
                .map_err(|_| AuthError::InvalidRequest)?;
            if let Some(stripped) = auth_str.strip_prefix("Bearer ") {
                stripped.to_string()
            } else {
                return Err(AuthError::InvalidRequest);
            }
        }
        None => return Err(AuthError::InvalidRequest),
    };

    // Blacklist the token
    token_service.blacklist_token(&token).await?;

    Ok(HttpResponse::Ok().json(json!({
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

// Verify email handler
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
        if !verify(&req.old_password, password_hash)
            .map_err(|e| AuthError::Validation(format!("Password verification failed: {}", e)))?
        {
            return Err(AuthError::InvalidCredentials);
        }
    } else {
        return Err(AuthError::InvalidCredentials);
    }

    // Hash new password
    let new_password_hash = hash(&req.new_password, DEFAULT_COST)
        .map_err(|e| AuthError::Validation(format!("Password hashing failed: {}", e)))?;

    // Update password
    user_repo
        .update_password(user.user_id, &new_password_hash)
        .await?;

    Ok(HttpResponse::Ok().json(json!({
        "message": "Password changed successfully",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

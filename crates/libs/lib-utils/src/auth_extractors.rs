//! Authentication extractors for handlers
//!
//! This module provides extractors that can be used in handlers to get authenticated user information
//! from either JWT tokens or API keys.

use actix_web::{dev::Payload, web, Error as ActixError, FromRequest, HttpMessage, HttpRequest};
use futures_util::future::{ready, Ready};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use lib_middleware::api_key::security::ApiKeyConfig;

/// Represents an authenticated user ID extracted from either JWT or API key authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedUserId(pub Uuid);

impl From<AuthenticatedUser> for AuthenticatedUserId {
    fn from(user: AuthenticatedUser) -> Self {
        Self(user.user_id)
    }
}

impl From<&AuthenticatedUser> for AuthenticatedUserId {
    fn from(user: &AuthenticatedUser) -> Self {
        Self(user.user_id)
    }
}

/// Extractor that gets the authenticated user ID from either JWT or API key authentication
impl FromRequest for AuthenticatedUserId {
    type Error = ActixError;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        // First try to get JWT authenticated user
        if let Some(jwt_user) = req.extensions().get::<AuthenticatedUser>() {
            return ready(Ok(AuthenticatedUserId::from(jwt_user)));
        }

        // If no JWT user, try to get API key config and extract user info
        if let Some(_api_config) = req.extensions().get::<Arc<ApiKeyConfig>>() {
            // For API key authentication, we need to get the user_id from the request
            // This could be from query params, headers, or request body
            // For now, we'll return an error indicating that user_id must be provided
            return ready(Err(actix_web::error::ErrorBadRequest(
                "API key authentication requires user_id to be provided in request",
            )));
        }

        // No authentication found
        ready(Err(actix_web::error::ErrorUnauthorized(
            "Authentication required",
        )))
    }
}

/// Represents the full authentication context with user ID and authentication method
#[derive(Debug, Clone)]
pub enum AuthContext {
    Jwt(AuthenticatedUser),
    ApiKey(Arc<ApiKeyConfig>),
}

impl FromRequest for AuthContext {
    type Error = ActixError;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        // First try to get JWT authenticated user
        if let Some(jwt_user) = req.extensions().get::<AuthenticatedUser>() {
            return ready(Ok(AuthContext::Jwt(jwt_user.clone())));
        }

        // If no JWT user, try to get API key config
        if let Some(api_config) = req.extensions().get::<Arc<ApiKeyConfig>>() {
            return ready(Ok(AuthContext::ApiKey(api_config.clone())));
        }

        // No authentication found
        ready(Err(actix_web::error::ErrorUnauthorized(
            "Authentication required",
        )))
    }
}

impl AuthContext {
    /// Extract user ID from the authentication context
    pub fn user_id(&self) -> Option<Uuid> {
        match self {
            AuthContext::Jwt(user) => Some(user.user_id),
            AuthContext::ApiKey(_) => None, // API key doesn't directly contain user_id
        }
    }

    /// Check if this is JWT authentication
    pub fn is_jwt(&self) -> bool {
        matches!(self, AuthContext::Jwt(_))
    }

    /// Check if this is API key authentication
    pub fn is_api_key(&self) -> bool {
        matches!(self, AuthContext::ApiKey(_))
    }

    /// Get the API key config if this is API key authentication
    pub fn api_key_config(&self) -> Option<&ApiKeyConfig> {
        match self {
            AuthContext::ApiKey(config) => Some(config),
            _ => None,
        }
    }

    /// Get the JWT user if this is JWT authentication
    pub fn jwt_user(&self) -> Option<&AuthenticatedUser> {
        match self {
            AuthContext::Jwt(user) => Some(user),
            _ => None,
        }
    }
}

/// Helper function to extract user ID from request with fallback to provided user_id
pub fn extract_user_id_from_request(
    req: &HttpRequest,
    fallback_user_id: Option<Uuid>,
) -> Result<Uuid, ActixError> {
    // First try to get from JWT authentication
    if let Some(jwt_user) = req.extensions().get::<AuthenticatedUser>() {
        return Ok(jwt_user.user_id);
    }

    // If no JWT user, try to get from API key authentication with fallback
    if req.extensions().get::<Arc<ApiKeyConfig>>().is_some() {
        if let Some(user_id) = fallback_user_id {
            return Ok(user_id);
        }
        return Err(actix_web::error::ErrorBadRequest(
            "API key authentication requires user_id to be provided",
        ));
    }

    // No authentication found
    Err(actix_web::error::ErrorUnauthorized(
        "Authentication required",
    ))
}

/// Helper function to extract user ID from query parameters
pub fn extract_user_id_from_query(req: &HttpRequest) -> Result<Uuid, ActixError> {
    let query =
        web::Query::<std::collections::HashMap<String, String>>::from_query(req.query_string())
            .map_err(|_| {
                actix_web::error::ErrorBadRequest(
                    "Invalid query parameters",
                )
            })?;

    let user_id_str = query.get("user_id").ok_or_else(|| {
        actix_web::error::ErrorBadRequest(
            "user_id query parameter required",
        )
    })?;

    let user_id = Uuid::parse_str(user_id_str).map_err(|_| {
        actix_web::error::ErrorBadRequest("Invalid user_id format")
    })?;

    Ok(user_id)
}

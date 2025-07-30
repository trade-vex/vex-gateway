use thiserror::Error;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("OIDC discovery failed: {0}")]
    OidcDiscovery(String),

    #[error("OIDC token exchange failed: {0}")]
    TokenExchange(String),

    #[error("Invalid OIDC state or nonce")]
    InvalidState,

    #[error("JWT creation failed: {0}")]
    JwtCreation(#[from] jsonwebtoken::errors::Error),
    
    #[error("JWT validation failed: {0}")]
    JwtValidation(String),

    #[error("Token is expired")]
    TokenExpired,
    
    #[error("Token has been revoked")]
    TokenRevoked,

    #[error("Redis operation failed: {0}")]
    RedisError(#[from] deadpool_redis::PoolError),

    #[error("Redis command failed: {0}")]
    RedisCmdError(#[from] redis::RedisError),
    
    #[error("User not found")]
    UserNotFound,

    #[error("An internal error occurred: {0}")]
    Internal(String),
}


impl From<AuthError> for actix_web::Error {
    fn from(err: AuthError) -> Self {
        use actix_web::error::*;
        match err {
            AuthError::TokenExpired | AuthError::TokenRevoked | AuthError::JwtValidation(_) => ErrorUnauthorized(err),
            AuthError::UserNotFound => ErrorNotFound(err),
            _ => ErrorInternalServerError(err),
        }
    }
}
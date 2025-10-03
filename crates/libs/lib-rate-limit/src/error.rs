use thiserror::Error;

#[derive(Debug, Error)]
pub enum RateLimitError {
    #[error("Redis connection error: {0}")]
    RedisConnection(String),

    #[error("Redis operation error: {0}")]
    RedisOperation(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("Rate limit exceeded for user: {user_id}")]
    RateLimitExceeded { user_id: String },

    #[error("Invalid user ID: {0}")]
    InvalidUserId(String),

    #[error("Middleware error: {0}")]
    Middleware(String),
}

impl From<redis::RedisError> for RateLimitError {
    fn from(err: redis::RedisError) -> Self {
        match err.kind() {
            redis::ErrorKind::IoError => Self::RedisConnection(err.to_string()),
            _ => Self::RedisOperation(err.to_string()),
        }
    }
}

impl From<serde_json::Error> for RateLimitError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

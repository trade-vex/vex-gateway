use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiKeyError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("API key not found")]
    NotFound,

    #[error("Invalid signature: {0}")]
    InvalidSignature(String),

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Request payload too large")]
    PayloadTooLarge,

    #[error("Missing API key")]
    MissingApiKey,

    #[error("Missing signature")]
    MissingSignature,

    #[error("Invalid API key: {0}")]
    InvalidApiKey(String),

    #[error("Missing timestamp")]
    MissingTimestamp,

    #[error(
        "Future timestamp - request timestamp {request_time} is ahead of server time {server_time}"
    )]
    FutureTimestamp { request_time: u64, server_time: u64 },

    #[error("Request expired - request timestamp {request_time} is too old (server time: {server_time}, recv_window: {recv_window}ms)")]
    RequestExpired {
        request_time: u64,
        server_time: u64,
        recv_window: u64,
    },
}

impl From<sea_orm::DbErr> for ApiKeyError {
    fn from(err: sea_orm::DbErr) -> Self {
        ApiKeyError::Database(err.to_string())
    }
}

impl From<serde_json::Error> for ApiKeyError {
    fn from(err: serde_json::Error) -> Self {
        ApiKeyError::Serialization(err.to_string())
    }
}

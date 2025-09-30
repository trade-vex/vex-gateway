use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiKeyError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("API key not found")]
    NotFound,

    #[error("API key is disabled")]
    Disabled,

    #[error("API key has expired")]
    Expired,

    #[error("Invalid signature: {0}")]
    InvalidSignature(String),

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("Request window exceeded")]
    WindowExceeded,

    #[error("IP address not whitelisted: {0}")]
    IpNotWhitelisted(String),

    #[error("Permission denied for security type: {0}")]
    PermissionDenied(String),

    #[error("Invalid key material: {0}")]
    InvalidKeyMaterial(String),

    #[error("Missing required header: {0}")]
    MissingHeader(String),

    #[error("Invalid header format: {0}")]
    InvalidHeader(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Internal error: {0}")]
    Internal(String),

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

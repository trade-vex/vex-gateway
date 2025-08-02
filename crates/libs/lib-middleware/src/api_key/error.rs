use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiKeyError {
    #[error("Missing API key header")]
    MissingApiKey,
    
    #[error("Invalid API key: {0}")]
    InvalidApiKey(String),
    
    #[error("Insufficient permissions. Required: {required}, Available: {available}")]
    InsufficientPermissions {
        required: String,
        available: String,
    },
    
    #[error("Missing signature for signed endpoint")]
    MissingSignature,
    
    #[error("Invalid signature: {0}")]
    InvalidSignature(String),
    
    #[error("Missing timestamp")]
    MissingTimestamp,
    
    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),
    
    #[error("Request expired. Server time: {server_time}, Request time: {request_time}, Recv window: {recv_window}")]
    RequestExpired {
        server_time: u64,
        request_time: u64,
        recv_window: u64,
    },
    
    #[error("Future timestamp. Server time: {server_time}, Request time: {request_time}")]
    FutureTimestamp {
        server_time: u64,
        request_time: u64,
    },
    
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<std::io::Error> for ApiKeyError {
    fn from(err: std::io::Error) -> Self {
        ApiKeyError::Internal(err.to_string())
    }
}

impl From<base64::DecodeError> for ApiKeyError {
    fn from(err: base64::DecodeError) -> Self {
        ApiKeyError::InvalidSignature(format!("Base64 decode error: {}", err))
    }
}
use lib_middleware::api_key::error::ApiKeyError;
use sea_orm::DbErr;
use serde_json;

#[test]
fn test_api_key_error_variants() {
    // Test all error variants display correctly
    let database_error = ApiKeyError::Database("Connection failed".to_string());
    assert_eq!(
        database_error.to_string(),
        "Database error: Connection failed"
    );

    let not_found = ApiKeyError::NotFound;
    assert_eq!(not_found.to_string(), "API key not found");

    let disabled = ApiKeyError::Disabled;
    assert_eq!(disabled.to_string(), "API key is disabled");

    let expired = ApiKeyError::Expired;
    assert_eq!(expired.to_string(), "API key has expired");

    let invalid_signature = ApiKeyError::InvalidSignature("Bad format".to_string());
    assert_eq!(
        invalid_signature.to_string(),
        "Invalid signature: Bad format"
    );

    let invalid_timestamp = ApiKeyError::InvalidTimestamp("Not a number".to_string());
    assert_eq!(
        invalid_timestamp.to_string(),
        "Invalid timestamp: Not a number"
    );

    let window_exceeded = ApiKeyError::WindowExceeded;
    assert_eq!(window_exceeded.to_string(), "Request window exceeded");

    // let ip_blacklisted = ApiKeyError::IpBlacklisted("192.168.1.1".to_string());
    // assert_eq!(ip_blacklisted.to_string(), "IP address blacklisted: 192.168.1.1");

    let permission_denied = ApiKeyError::PermissionDenied("TRADE".to_string());
    assert_eq!(
        permission_denied.to_string(),
        "Permission denied for security type: TRADE"
    );

    let invalid_key_material = ApiKeyError::InvalidKeyMaterial("Invalid PEM".to_string());
    assert_eq!(
        invalid_key_material.to_string(),
        "Invalid key material: Invalid PEM"
    );

    let missing_header = ApiKeyError::MissingHeader("X-API-Key".to_string());
    assert_eq!(
        missing_header.to_string(),
        "Missing required header: X-API-Key"
    );

    let invalid_header = ApiKeyError::InvalidHeader("Malformed value".to_string());
    assert_eq!(
        invalid_header.to_string(),
        "Invalid header format: Malformed value"
    );

    let cache_error = ApiKeyError::Cache("Redis timeout".to_string());
    assert_eq!(cache_error.to_string(), "Cache error: Redis timeout");

    let serialization_error = ApiKeyError::Serialization("JSON parse error".to_string());
    assert_eq!(
        serialization_error.to_string(),
        "Serialization error: JSON parse error"
    );

    let internal_error = ApiKeyError::Internal("Unexpected condition".to_string());
    assert_eq!(
        internal_error.to_string(),
        "Internal error: Unexpected condition"
    );

    let missing_api_key = ApiKeyError::MissingApiKey;
    assert_eq!(missing_api_key.to_string(), "Missing API key");

    let missing_signature = ApiKeyError::MissingSignature;
    assert_eq!(missing_signature.to_string(), "Missing signature");

    let invalid_api_key = ApiKeyError::InvalidApiKey("Wrong format".to_string());
    assert_eq!(invalid_api_key.to_string(), "Invalid API key: Wrong format");

    let missing_timestamp = ApiKeyError::MissingTimestamp;
    assert_eq!(missing_timestamp.to_string(), "Missing timestamp");
}

#[test]
fn test_future_timestamp_error() {
    let error = ApiKeyError::FutureTimestamp {
        request_time: 1640995300000,
        server_time: 1640995200000,
    };

    assert_eq!(
        error.to_string(),
        "Future timestamp - request timestamp 1640995300000 is ahead of server time 1640995200000"
    );
}

#[test]
fn test_request_expired_error() {
    let error = ApiKeyError::RequestExpired {
        request_time: 1640995100000,
        server_time: 1640995200000,
        recv_window: 5000,
    };

    assert_eq!(
        error.to_string(),
        "Request expired - request timestamp 1640995100000 is too old (server time: 1640995200000, recv_window: 5000ms)"
    );
}

#[test]
fn test_error_from_sea_orm_db_err() {
    let db_error = DbErr::Custom("Test database error".to_string());
    let api_key_error: ApiKeyError = db_error.into();

    match api_key_error {
        ApiKeyError::Database(msg) => {
            assert!(msg.contains("Test database error"));
        }
        _ => panic!("Expected Database error variant"),
    }
}

#[test]
fn test_error_from_serde_json_error() {
    let json_error = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
    let api_key_error: ApiKeyError = json_error.into();

    match api_key_error {
        ApiKeyError::Serialization(msg) => {
            assert!(msg.contains("expected"));
        }
        _ => panic!("Expected Serialization error variant"),
    }
}

#[test]
fn test_error_debug_format() {
    let error = ApiKeyError::NotFound;
    let debug_str = format!("{:?}", error);
    assert_eq!(debug_str, "NotFound");

    let error_with_data = ApiKeyError::InvalidSignature("test".to_string());
    let debug_str = format!("{:?}", error_with_data);
    assert_eq!(debug_str, "InvalidSignature(\"test\")");
}

#[test]
fn test_error_send_sync() {
    // Ensure ApiKeyError implements Send and Sync for async usage
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    assert_send::<ApiKeyError>();
    assert_sync::<ApiKeyError>();
}

#[test]
fn test_error_std_error_trait() {
    let error = ApiKeyError::NotFound;
    let _: &dyn std::error::Error = &error;

    // Test that we can chain errors
    let source = ApiKeyError::Internal("root cause".to_string());
    let _wrapped = ApiKeyError::Database(source.to_string());
}

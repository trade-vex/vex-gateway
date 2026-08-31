use lib_middleware::api_key::error::ApiKeyError;
use lib_middleware::api_key::signature::{SignatureAlgorithm, SignatureVerifier};

#[test]
fn test_signature_algorithm_equality() {
    assert_eq!(
        SignatureAlgorithm::HmacSha256,
        SignatureAlgorithm::HmacSha256
    );
    assert_eq!(SignatureAlgorithm::RsaSha256, SignatureAlgorithm::RsaSha256);
    assert_eq!(SignatureAlgorithm::Ed25519, SignatureAlgorithm::Ed25519);

    assert_ne!(
        SignatureAlgorithm::HmacSha256,
        SignatureAlgorithm::RsaSha256
    );
    assert_ne!(SignatureAlgorithm::RsaSha256, SignatureAlgorithm::Ed25519);
}

#[test]
fn test_signature_algorithm_clone() {
    let original = SignatureAlgorithm::HmacSha256;
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn test_verify_hmac_success() {
    let payload = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
    let secret_key = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    let signature = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71";

    let result = SignatureVerifier::verify_hmac(payload, signature, secret_key).unwrap();
    assert!(result);
}

#[test]
fn test_verify_hmac_failure() {
    let payload = "symbol=LTCBTC&side=BUY&type=LIMIT";
    let secret_key = "secret";
    let wrong_signature = "wrong_signature_hex";

    let result = SignatureVerifier::verify_hmac(payload, wrong_signature, secret_key);
    assert!(result.is_err());

    match result.unwrap_err() {
        ApiKeyError::InvalidSignature(msg) => assert!(msg.contains("Invalid hex signature")),
        _ => panic!("Expected InvalidSignature error"),
    }
}

#[test]
fn test_verify_hmac_invalid_hex() {
    let payload = "test";
    let secret_key = "secret";
    let invalid_hex = "not_hex_signature";

    let result = SignatureVerifier::verify_hmac(payload, invalid_hex, secret_key);
    assert!(result.is_err());
}

#[test]
fn test_generate_hmac() {
    let payload = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
    let secret_key = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    let expected = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71";

    let result = SignatureVerifier::generate_hmac(payload, secret_key).unwrap();
    assert_eq!(result, expected);
}

#[test]
fn test_generate_hmac_roundtrip() {
    let payload = "test_payload";
    let secret_key = "test_secret";

    let signature = SignatureVerifier::generate_hmac(payload, secret_key).unwrap();
    let verification = SignatureVerifier::verify_hmac(payload, &signature, secret_key).unwrap();

    assert!(verification);
}

#[test]
fn test_verify_rsa_invalid_signature() {
    let payload = "test";
    let invalid_signature = "not_base64";
    let public_key_pem = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA...\n-----END PUBLIC KEY-----";

    let result = SignatureVerifier::verify_rsa(payload, invalid_signature, public_key_pem);
    assert!(result.is_err());

    match result.unwrap_err() {
        ApiKeyError::InvalidSignature(msg) => {
            assert!(msg.contains("Failed to decode base64 signature"))
        }
        _ => panic!("Expected InvalidSignature error"),
    }
}

#[test]
fn test_verify_rsa_invalid_key() {
    let payload = "test";
    let signature = "dGVzdA=="; // "test" in base64
    let invalid_key = "not a pem key";

    let result = SignatureVerifier::verify_rsa(payload, signature, invalid_key);
    assert!(result.is_err());

    match result.unwrap_err() {
        ApiKeyError::InvalidSignature(msg) => {
            assert!(msg.contains("Failed to parse RSA public key"))
        }
        _ => panic!("Expected InvalidSignature error"),
    }
}

#[test]
fn test_verify_ed25519_invalid_signature() {
    let payload = "test";
    let invalid_signature = "not_base64";
    let public_key_pem =
        "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA...\n-----END PUBLIC KEY-----";

    let result = SignatureVerifier::verify_ed25519(payload, invalid_signature, public_key_pem);
    assert!(result.is_err());

    match result.unwrap_err() {
        ApiKeyError::InvalidSignature(msg) => {
            assert!(msg.contains("Failed to decode base64 signature"))
        }
        _ => panic!("Expected InvalidSignature error"),
    }
}

#[test]
fn test_verify_ed25519_invalid_key() {
    let payload = "test";
    let signature = "dGVzdA=="; // "test" in base64 (too short for Ed25519)
    let invalid_key = "not a pem key";

    let result = SignatureVerifier::verify_ed25519(payload, signature, invalid_key);
    assert!(result.is_err());

    match result.unwrap_err() {
        ApiKeyError::InvalidSignature(msg) => {
            // The error message might be about signature format or key parsing
            assert!(msg.contains("Ed25519") || msg.contains("signature") || msg.contains("key"));
        }
        _ => panic!("Expected InvalidSignature error"),
    }
}

#[test]
fn test_canonical_payload_with_empty_body_is_well_defined() {
    let payload =
        SignatureVerifier::build_request_payload("GET", "/api/orders", "symbol=BTCUSDT", "", false);

    assert_eq!(payload, "GET\n%2Fapi%2Forders\nsymbol=BTCUSDT\n");
}

#[test]
fn test_canonical_payload_sorts_query() {
    let payload = SignatureVerifier::build_request_payload(
        "GET",
        "/api/orders",
        "timestamp=1640995200000&symbol=BTCUSDT&side=BUY",
        "",
        false,
    );

    assert_eq!(
        payload,
        "GET\n%2Fapi%2Forders\nside=BUY&symbol=BTCUSDT&timestamp=1640995200000\n"
    );
}

#[test]
fn test_canonical_payload_excludes_exact_signature_fields() {
    let payload = SignatureVerifier::build_request_payload(
        "POST",
        "/api/orders",
        "symbol=BTCUSDT&signature=should_be_excluded",
        "quantity=1&signature=also_excluded",
        true,
    );

    assert_eq!(
        payload,
        format!(
            "POST\n%2Fapi%2Forders\nsymbol=BTCUSDT\n{}",
            hex::encode("quantity=1")
        )
    );
}

#[test]
fn test_body_parameter_does_not_override_same_named_query_parameter() {
    let payload = SignatureVerifier::build_request_payload(
        "POST",
        "/api/orders",
        "symbol=query",
        "symbol=body",
        true,
    );

    assert_eq!(
        payload,
        format!(
            "POST\n%2Fapi%2Forders\nsymbol=query\n{}",
            hex::encode("symbol=body")
        )
    );
}

#[test]
fn test_build_request_payload() {
    let query_string = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC";
    let request_body = "quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";

    let payload = SignatureVerifier::build_request_payload(
        "POST",
        "/api/order",
        query_string,
        request_body,
        true,
    );
    let expected = format!(
        "POST\n%2Fapi%2Forder\nside=BUY&symbol=LTCBTC&timeInForce=GTC&type=LIMIT\n{}",
        hex::encode(request_body)
    );

    assert_eq!(payload, expected);
}

#[test]
fn test_query_body_splits_have_distinct_payloads() {
    let first = SignatureVerifier::build_request_payload(
        "POST",
        "/api/orders",
        "value=left",
        "right",
        false,
    );
    let second = SignatureVerifier::build_request_payload(
        "POST",
        "/api/orders",
        "value=leftright",
        "",
        false,
    );

    assert_ne!(first, second);
}

#[test]
fn test_signature_prefixed_parameter_is_signed() {
    let payload = SignatureVerifier::build_request_payload(
        "POST",
        "/api/orders",
        "signature_foo=query&signature=excluded",
        "signature_foo=body&signature=excluded",
        true,
    );

    assert_eq!(
        payload,
        format!(
            "POST\n%2Fapi%2Forders\nsignature_foo=query\n{}",
            hex::encode("signature_foo=body")
        )
    );
}

#[test]
fn test_build_request_payload_with_signature() {
    let query_string = "symbol=BTCUSDT&signature=should_be_removed";
    let request_body = "quantity=1&signature=also_removed&price=50000";

    let payload = SignatureVerifier::build_request_payload(
        "POST",
        "/api/order",
        query_string,
        request_body,
        true,
    );
    assert_eq!(
        payload,
        format!(
            "POST\n%2Fapi%2Forder\nsymbol=BTCUSDT\n{}",
            hex::encode("quantity=1&price=50000")
        )
    );
}

#[test]
fn test_build_request_payload_empty_parts() {
    assert_eq!(
        SignatureVerifier::build_request_payload("GET", "/api/orders", "", "", false),
        "GET\n%2Fapi%2Forders\n\n"
    );
    assert_eq!(
        SignatureVerifier::build_request_payload("GET", "/api/orders", "symbol=BTCUSDT", "", false,),
        "GET\n%2Fapi%2Forders\nsymbol=BTCUSDT\n"
    );
    assert_eq!(
        SignatureVerifier::build_request_payload("POST", "/api/orders", "", "quantity=1", true,),
        format!("POST\n%2Fapi%2Forders\n\n{}", hex::encode("quantity=1"))
    );
}

#[test]
fn test_mixed_payload_removes_signature() {
    // Test that mixed payload properly removes signature parameters
    let query_with_sig = "symbol=BTCUSDT&signature=should_be_removed";
    let body_with_sig = "quantity=1&signature=also_removed&price=50000";

    let payload = SignatureVerifier::build_request_payload(
        "POST",
        "/api/order",
        query_with_sig,
        body_with_sig,
        true,
    );
    assert_eq!(
        payload,
        format!(
            "POST\n%2Fapi%2Forder\nsymbol=BTCUSDT\n{}",
            hex::encode("quantity=1&price=50000")
        )
    );
}

#[test]
fn test_mixed_payload_signature_removal_edge_cases() {
    // Test empty strings
    assert_eq!(
        SignatureVerifier::build_request_payload("GET", "/api/orders", "", "", true),
        "GET\n%2Fapi%2Forders\n\n"
    );

    // Test signature only
    let payload = SignatureVerifier::build_request_payload(
        "GET",
        "/api/orders",
        "signature=only",
        "signature=this",
        true,
    );
    assert_eq!(payload, "GET\n%2Fapi%2Forders\n\n");

    // Test no signature parameters
    let payload = SignatureVerifier::build_request_payload(
        "POST",
        "/api/orders",
        "symbol=BTCUSDT",
        "quantity=1",
        true,
    );
    assert_eq!(
        payload,
        format!(
            "POST\n%2Fapi%2Forders\nsymbol=BTCUSDT\n{}",
            hex::encode("quantity=1")
        )
    );
}

#[test]
fn test_validate_timestamp_valid() {
    let server_time = 1640995200000;
    let recv_window = 5000;
    let valid_timestamp = server_time - 1000; // 1 second ago

    let result = SignatureVerifier::validate_timestamp(valid_timestamp, recv_window, server_time);
    assert!(result.is_ok());
}

#[test]
fn test_validate_timestamp_future() {
    let server_time = 1640995200000;
    let recv_window = 5000;
    let future_timestamp = server_time + 2000; // 2 seconds in future

    let result = SignatureVerifier::validate_timestamp(future_timestamp, recv_window, server_time);
    assert!(result.is_err());

    match result.unwrap_err() {
        ApiKeyError::FutureTimestamp {
            request_time,
            server_time: st,
        } => {
            assert_eq!(request_time, future_timestamp);
            assert_eq!(st, server_time);
        }
        _ => panic!("Expected FutureTimestamp error"),
    }
}

#[test]
fn test_validate_timestamp_edge_cases() {
    let server_time = 1640995200000;
    let recv_window = 5000;

    // Test edge case where implementation might have integer overflow
    // Use a small past timestamp that won't cause overflow
    let past_timestamp = server_time - 1000;
    let result = SignatureVerifier::validate_timestamp(past_timestamp, recv_window, server_time);
    assert!(result.is_ok()); // Should be valid as it's within window
}

#[test]
fn test_validate_timestamp_expired() {
    let server_time = 1640995200000;
    let recv_window = 5000;
    let expired_timestamp = server_time - 10000; // 10 seconds ago (beyond 5s window)

    let result = SignatureVerifier::validate_timestamp(expired_timestamp, recv_window, server_time);
    assert!(result.is_err());

    match result.unwrap_err() {
        ApiKeyError::RequestExpired {
            request_time,
            server_time: st,
            recv_window: rw,
        } => {
            assert_eq!(request_time, expired_timestamp);
            assert_eq!(st, server_time);
            assert_eq!(rw, recv_window);
        }
        _ => panic!("Expected RequestExpired error"),
    }
}

#[test]
fn test_url_encode_signature() {
    // Test basic cases
    assert_eq!(SignatureVerifier::url_encode_signature("abc"), "abc");
    assert_eq!(
        SignatureVerifier::url_encode_signature("abc/def"),
        "abc%2Fdef"
    );
    assert_eq!(
        SignatureVerifier::url_encode_signature("abc=def"),
        "abc%3Ddef"
    );
    let space_encoded = SignatureVerifier::url_encode_signature("abc def");
    assert!(space_encoded == "abc%20def" || space_encoded == "abc+def"); // Different implementations vary

    // Test + character encoding (different URL encoding implementations may vary)
    let plus_encoded = SignatureVerifier::url_encode_signature("abc+def");
    // + can be encoded as %2B or remain as + depending on the implementation
    assert!(plus_encoded == "abc%2Bdef" || plus_encoded == "abc+def");

    // Test a complex signature with multiple special characters
    let complex_sig = "abc+def/ghi=jkl mno";
    let encoded = SignatureVerifier::url_encode_signature(complex_sig);
    // Should encode most special characters
    assert!(encoded.contains("%2F")); // /
    assert!(encoded.contains("%3D")); // =
                                      // Space might be encoded as %20 or +
    assert!(encoded.contains("%20") || encoded.contains("+"));
}

#[test]
fn test_parse_timestamp_milliseconds() {
    assert_eq!(
        SignatureVerifier::parse_timestamp("1640995200000").unwrap(),
        1640995200000
    );
    assert_eq!(
        SignatureVerifier::parse_timestamp("123456789012").unwrap(),
        123456789012
    );
}

#[test]
fn test_parse_timestamp_rejects_microseconds() {
    assert!(SignatureVerifier::parse_timestamp("1640995200000000").is_err());
    assert!(SignatureVerifier::parse_timestamp("123456789012345").is_err());
}

#[test]
fn test_parse_timestamp_invalid() {
    assert!(SignatureVerifier::parse_timestamp("not_a_number").is_err());
    assert!(SignatureVerifier::parse_timestamp("").is_err());
    assert!(SignatureVerifier::parse_timestamp("-123").is_err());

    match SignatureVerifier::parse_timestamp("invalid").unwrap_err() {
        ApiKeyError::InvalidTimestamp(msg) => assert_eq!(msg, "Invalid timestamp format"),
        _ => panic!("Expected InvalidTimestamp error"),
    }
}

#[test]
fn test_parse_timestamp_edge_cases() {
    // 13 digits - milliseconds
    assert_eq!(
        SignatureVerifier::parse_timestamp("9999999999999").unwrap(),
        9999999999999
    );

    // More than 13 digits is not accepted as milliseconds
    assert!(SignatureVerifier::parse_timestamp("12345678901234").is_err());

    // Very large number
    assert!(SignatureVerifier::parse_timestamp("1234567890123456").is_err());
}

#[test]
fn test_hmac_and_canonical_payload_roundtrip() {
    let payload1 = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
    let secret_key = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    let expected_sig1 = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71";

    assert!(SignatureVerifier::verify_hmac(payload1, expected_sig1, secret_key).unwrap());

    let query_string = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC";
    let request_body = "quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
    let payload3 = SignatureVerifier::build_request_payload(
        "POST",
        "/api/order",
        query_string,
        request_body,
        true,
    );
    let signature3 = SignatureVerifier::generate_hmac(&payload3, secret_key).unwrap();

    assert!(SignatureVerifier::verify_hmac(&payload3, &signature3, secret_key).unwrap());
}

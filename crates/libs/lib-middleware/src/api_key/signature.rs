use crate::api_key::error::ApiKeyError;
use base64::{engine::general_purpose, Engine as _};
use ed25519_dalek::{Signature, Verifier as Ed25519Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use pkcs8::DecodePublicKey;
use rsa::{pkcs8::DecodePublicKey as _, Pkcs1v15Sign, PublicKey as RsaPublicKey};
use sha2::{Digest, Sha256};

/// Supported signature algorithms
#[derive(Debug, Clone, PartialEq)]
pub enum SignatureAlgorithm {
    HmacSha256,
    RsaSha256,
    Ed25519,
}

/// Signature verification for different key types
pub struct SignatureVerifier;

impl SignatureVerifier {
    /// Verify HMAC SHA256 signature
    pub fn verify_hmac(
        payload: &str,
        signature: &str,
        secret_key: &str,
    ) -> Result<bool, ApiKeyError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret_key.as_bytes())
            .map_err(|e| ApiKeyError::Internal(format!("HMAC creation failed: {}", e)))?;

        mac.update(payload.as_bytes());

        // Decode the hex signature
        let signature_bytes = hex::decode(signature)
            .map_err(|_| ApiKeyError::InvalidSignature("Invalid hex signature".to_string()))?;

        // Use the HMAC's built-in verify method
        Ok(mac.verify_slice(&signature_bytes).is_ok())
    }

    /// Verify RSA signature (base64 encoded) using RSASSA-PKCS1-v1_5 with SHA-256
    pub fn verify_rsa(
        payload: &str,
        signature: &str,
        public_key_pem: &str,
    ) -> Result<bool, ApiKeyError> {
        // Decode the base64 signature
        let signature_bytes = general_purpose::STANDARD.decode(signature).map_err(|e| {
            ApiKeyError::InvalidSignature(format!("Failed to decode base64 signature: {}", e))
        })?;

        // Parse the PEM public key
        let public_key = RsaPublicKey::from_public_key_pem(public_key_pem).map_err(|e| {
            ApiKeyError::InvalidSignature(format!("Failed to parse RSA public key: {}", e))
        })?;

        // Hash the payload with SHA-256
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        let hashed = hasher.finalize();

        // Verify using PKCS#1 v1.5 padding with SHA-256. rsa is built on sha2 0.11, so the
        // padding takes the digest OID from rsa's own sha2 re-export.
        let padding = Pkcs1v15Sign::new::<rsa::sha2::Sha256>();
        let verification_result = public_key.verify(padding, &hashed, &signature_bytes);

        Ok(verification_result.is_ok())
    }

    /// Verify Ed25519 signature (base64 encoded)
    /// Following the exact Binance specification for Ed25519
    pub fn verify_ed25519(
        payload: &str,
        signature: &str,
        public_key_pem: &str,
    ) -> Result<bool, ApiKeyError> {
        // Decode the base64 signature
        let signature_bytes = general_purpose::STANDARD.decode(signature).map_err(|e| {
            ApiKeyError::InvalidSignature(format!("Failed to decode base64 signature: {}", e))
        })?;

        // Parse the signature
        let signature = Signature::from_slice(&signature_bytes).map_err(|e| {
            ApiKeyError::InvalidSignature(format!("Invalid Ed25519 signature format: {}", e))
        })?;

        // Parse the PEM public key
        let public_key = VerifyingKey::from_public_key_pem(public_key_pem).map_err(|e| {
            ApiKeyError::InvalidSignature(format!("Failed to parse Ed25519 public key: {}", e))
        })?;

        // Verify the signature
        let verification_result = public_key.verify(payload.as_bytes(), &signature);

        Ok(verification_result.is_ok())
    }

    /// Generate HMAC SHA256 signature
    pub fn generate_hmac(payload: &str, secret_key: &str) -> Result<String, ApiKeyError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret_key.as_bytes())
            .map_err(|e| ApiKeyError::Internal(format!("HMAC creation failed: {}", e)))?;

        mac.update(payload.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Build the canonical signature payload for every request.
    pub fn build_request_payload(
        method: &str,
        path: &str,
        query_string: &str,
        request_body: &str,
        body_is_form: bool,
    ) -> String {
        // Remove signature from query string if present
        let clean_query = Self::remove_signature_from_query(query_string);

        let clean_body = if body_is_form {
            Self::remove_signature_from_body(request_body)
        } else {
            request_body.to_string()
        };
        let encoded_method: String =
            url::form_urlencoded::byte_serialize(method.as_bytes()).collect();
        let encoded_path: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
        let encoded_body = hex::encode(clean_body.as_bytes());

        format!("{encoded_method}\n{encoded_path}\n{clean_query}\n{encoded_body}")
    }

    /// Remove signature parameter from query string
    fn remove_signature_from_query(query_string: &str) -> String {
        if query_string.is_empty() {
            return String::new();
        }

        let mut params: Vec<_> = query_string
            .split('&')
            .filter_map(|param| {
                let (key, value) = url::form_urlencoded::parse(param.as_bytes()).next()?;
                (key != "signature").then(|| (key.into_owned(), value.into_owned()))
            })
            .collect();
        params.sort();

        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in params {
            serializer.append_pair(&key, &value);
        }
        serializer.finish()
    }

    /// Remove signature parameter from request body
    fn remove_signature_from_body(request_body: &str) -> String {
        if request_body.is_empty() {
            return String::new();
        }

        request_body
            .split('&')
            .filter(|param| {
                url::form_urlencoded::parse(param.as_bytes())
                    .next()
                    .is_none_or(|(key, _)| key != "signature")
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Validate timestamp
    pub fn validate_timestamp(
        timestamp: u64,
        recv_window: u64,
        server_time: u64,
    ) -> Result<(), ApiKeyError> {
        // Future-drift check with 1s tolerance (avoid overflow)
        if timestamp > server_time.saturating_add(1000) {
            return Err(ApiKeyError::FutureTimestamp {
                server_time,
                request_time: timestamp,
            });
        }

        // Expired if server_time > timestamp + recv_window (avoid overflow)
        if server_time > timestamp.saturating_add(recv_window) {
            return Err(ApiKeyError::RequestExpired {
                server_time,
                request_time: timestamp,
                recv_window,
            });
        }

        Ok(())
    }

    /// URL encode signature for safe transmission
    pub fn url_encode_signature(signature: &str) -> String {
        url::form_urlencoded::byte_serialize(signature.as_bytes()).collect()
    }

    /// Parse a timestamp expressed in milliseconds
    pub fn parse_timestamp(timestamp_str: &str) -> Result<u64, ApiKeyError> {
        let timestamp = timestamp_str
            .parse::<u64>()
            .map_err(|_| ApiKeyError::InvalidTimestamp("Invalid timestamp format".to_string()))?;

        if timestamp > 9999999999999 {
            return Err(ApiKeyError::InvalidTimestamp(
                "Timestamp must be in milliseconds".to_string(),
            ));
        }

        Ok(timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binance_example_1_hmac() {
        // Exact Binance Example 1 from documentation
        let payload = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        let secret_key = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
        let expected_signature = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71";

        let result =
            SignatureVerifier::verify_hmac(payload, expected_signature, secret_key).unwrap();
        assert!(result, "Binance Example 1 HMAC verification should pass");

        // Also test signature generation
        let generated = SignatureVerifier::generate_hmac(payload, secret_key).unwrap();
        assert_eq!(generated.to_lowercase(), expected_signature.to_lowercase());
    }

    #[test]
    fn test_canonical_mixed_payload() {
        let query_string = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC";
        let request_body = "quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";

        let payload = SignatureVerifier::build_request_payload(
            "POST",
            "/api/order",
            query_string,
            request_body,
            true,
        );
        let expected_payload = concat!(
            "POST\n%2Fapi%2Forder\n",
            "side=BUY&symbol=LTCBTC&timeInForce=GTC&type=LIMIT\n",
            "7175616e746974793d312670726963653d302e31267265637657696e646f773d",
            "353030302674696d657374616d703d31343939383237333139353539"
        );

        assert_eq!(payload, expected_payload);
    }

    #[test]
    fn test_build_request_payload_sorting() {
        let payload = SignatureVerifier::build_request_payload(
            "GET",
            "/api/orders",
            "symbol=BTCUSDT&side=BUY&timestamp=1640995200000&quantity=1",
            "",
            false,
        );

        // Parameters should be sorted alphabetically
        let expected = concat!(
            "GET\n%2Fapi%2Forders\n",
            "quantity=1&side=BUY&symbol=BTCUSDT&timestamp=1640995200000\n"
        );
        assert_eq!(
            payload, expected,
            "Parameters should be sorted alphabetically"
        );
    }

    #[test]
    fn test_timestamp_validation() {
        let recv_window = 5000;
        let server_time = 1640995200000;

        // Valid timestamp within window
        assert!(SignatureVerifier::validate_timestamp(
            server_time - 1000,
            recv_window,
            server_time
        )
        .is_ok());

        // Future timestamp (beyond 1 second tolerance)
        assert!(SignatureVerifier::validate_timestamp(
            server_time + 2000,
            recv_window,
            server_time
        )
        .is_err());

        // Expired timestamp
        assert!(SignatureVerifier::validate_timestamp(
            server_time - 10000,
            recv_window,
            server_time
        )
        .is_err());
    }

    #[test]
    fn test_parse_timestamp() {
        // Milliseconds
        assert_eq!(
            SignatureVerifier::parse_timestamp("1640995200000").unwrap(),
            1640995200000
        );

        // Microseconds are rejected rather than silently reinterpreted
        assert!(SignatureVerifier::parse_timestamp("1640995200000000").is_err());

        // Invalid format
        assert!(SignatureVerifier::parse_timestamp("invalid").is_err());
    }

    #[test]
    fn test_signature_parameter_removal() {
        let query_with_sig = "symbol=BTCUSDT&side=BUY&signature=abc123&timestamp=1640995200000";
        let clean_query = SignatureVerifier::remove_signature_from_query(query_with_sig);
        assert_eq!(
            clean_query,
            "side=BUY&symbol=BTCUSDT&timestamp=1640995200000"
        );

        let body_with_sig = "quantity=1&signature=def456&price=0.1";
        let clean_body = SignatureVerifier::remove_signature_from_body(body_with_sig);
        assert_eq!(clean_body, "quantity=1&price=0.1");

        let signature_prefixed = "signature_foo=kept&signature=removed";
        assert_eq!(
            SignatureVerifier::remove_signature_from_query(signature_prefixed),
            "signature_foo=kept"
        );
        assert_eq!(
            SignatureVerifier::remove_signature_from_body(signature_prefixed),
            "signature_foo=kept"
        );
    }

    #[test]
    fn test_url_encoding() {
        let signature_with_special_chars = "abc+def/ghi=";
        let encoded = SignatureVerifier::url_encode_signature(signature_with_special_chars);
        assert_eq!(encoded, "abc%2Bdef%2Fghi%3D");
    }
}

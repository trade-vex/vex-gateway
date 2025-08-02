use crate::api_key::error::ApiKeyError;
use hmac::{Hmac, Mac};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use base64::{Engine as _, engine::general_purpose};
use rsa::{Pkcs1v15Sign, RsaPublicKey};
use ed25519_dalek::{Signature, VerifyingKey, Verifier as Ed25519Verifier};
use pkcs8::DecodePublicKey;

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
        
        let expected_signature = hex::encode(mac.finalize().into_bytes());
        Ok(signature.to_lowercase() == expected_signature.to_lowercase())
    }

    /// Verify RSA signature (base64 encoded) using RSASSA-PKCS1-v1_5 with SHA-256
    /// Following the exact Binance specification
    pub fn verify_rsa(
        payload: &str,
        signature: &str,
        public_key_pem: &str,
    ) -> Result<bool, ApiKeyError> {
        // Decode the base64 signature
        let signature_bytes = general_purpose::STANDARD
            .decode(signature)
            .map_err(|e| ApiKeyError::InvalidSignature(format!("Failed to decode base64 signature: {}", e)))?;

        // Parse the PEM public key
        let public_key = RsaPublicKey::from_public_key_pem(public_key_pem)
            .map_err(|e| ApiKeyError::InvalidSignature(format!("Failed to parse RSA public key: {}", e)))?;

        // Hash the payload with SHA-256
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        let hashed = hasher.finalize();

        // Verify using PKCS#1 v1.5 padding with SHA-256
        let padding = Pkcs1v15Sign::new::<Sha256>();
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
        let signature_bytes = general_purpose::STANDARD
            .decode(signature)
            .map_err(|e| ApiKeyError::InvalidSignature(format!("Failed to decode base64 signature: {}", e)))?;

        // Parse the signature
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|e| ApiKeyError::InvalidSignature(format!("Invalid Ed25519 signature format: {}", e)))?;

        // Parse the PEM public key
        let public_key = VerifyingKey::from_public_key_pem(public_key_pem)
            .map_err(|e| ApiKeyError::InvalidSignature(format!("Failed to parse Ed25519 public key: {}", e)))?;

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

    /// Build signature payload from query parameters and body
    /// Following the exact Binance specification for payload construction
    pub fn build_payload(
        query_params: &HashMap<String, String>,
        body_params: &HashMap<String, String>,
    ) -> String {
        let mut all_params = HashMap::new();
        
        // Add query parameters (excluding signature)
        for (key, value) in query_params {
            if key != "signature" {
                all_params.insert(key.clone(), value.clone());
            }
        }
        
        // Add body parameters (excluding signature)
        for (key, value) in body_params {
            if key != "signature" {
                all_params.insert(key.clone(), value.clone());
            }
        }
        
        // Sort parameters alphabetically by key (case-sensitive)
        let mut sorted_params: Vec<_> = all_params.iter().collect();
        sorted_params.sort_by_key(|(key, _)| *key);
        
        // Build payload string exactly as Binance expects
        sorted_params
            .iter()
            .map(|(key, value)| format!("{}={}", key, value))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Build payload for mixed query string and request body as per Binance example 3
    /// This handles the case where query and body parameters are concatenated without '&' between them
    pub fn build_mixed_payload(
        query_string: &str,
        request_body: &str,
    ) -> String {
        // Remove signature from query string if present
        let clean_query = Self::remove_signature_from_query(query_string);
        
        // Remove signature from request body if present
        let clean_body = Self::remove_signature_from_body(request_body);
        
        // Concatenate without separator as per Binance example 3
        format!("{}{}", clean_query, clean_body)
    }

    /// Remove signature parameter from query string
    fn remove_signature_from_query(query_string: &str) -> String {
        if query_string.is_empty() {
            return String::new();
        }
        
        query_string
            .split('&')
            .filter(|param| !param.starts_with("signature="))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Remove signature parameter from request body
    fn remove_signature_from_body(request_body: &str) -> String {
        if request_body.is_empty() {
            return String::new();
        }
        
        request_body
            .split('&')
            .filter(|param| !param.starts_with("signature="))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Validate timestamp according to Binance specification
    pub fn validate_timestamp(
        timestamp: u64,
        recv_window: u64,
        server_time: u64,
    ) -> Result<(), ApiKeyError> {
        // Check if timestamp is in the future (with 1 second tolerance as per Binance spec)
        if timestamp > server_time + 1000 {
            return Err(ApiKeyError::FutureTimestamp {
                server_time,
                request_time: timestamp,
            });
        }

        // Check if request has expired according to recv_window
        if server_time - timestamp > recv_window {
            return Err(ApiKeyError::RequestExpired {
                server_time,
                request_time: timestamp,
                recv_window,
            });
        }

        Ok(())
    }

    /// URL encode signature for safe transmission as per Binance specification
    pub fn url_encode_signature(signature: &str) -> String {
        url::form_urlencoded::byte_serialize(signature.as_bytes()).collect()
    }

    /// Parse timestamp from milliseconds or microseconds as per Binance spec
    pub fn parse_timestamp(timestamp_str: &str) -> Result<u64, ApiKeyError> {
        let timestamp = timestamp_str
            .parse::<u64>()
            .map_err(|_| ApiKeyError::InvalidTimestamp("Invalid timestamp format".to_string()))?;

        // Binance accepts both milliseconds and microseconds
        // If the timestamp is too large, assume it's microseconds and convert to milliseconds
        if timestamp > 9999999999999 { // More than 13 digits, likely microseconds
            Ok(timestamp / 1000) // Convert microseconds to milliseconds
        } else {
            Ok(timestamp) // Already in milliseconds
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_binance_example_1_hmac() {
        // Exact Binance Example 1 from documentation
        let payload = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        let secret_key = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
        let expected_signature = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71";
        
        let result = SignatureVerifier::verify_hmac(payload, expected_signature, secret_key).unwrap();
        assert!(result, "Binance Example 1 HMAC verification should pass");
        
        // Also test signature generation
        let generated = SignatureVerifier::generate_hmac(payload, secret_key).unwrap();
        assert_eq!(generated.to_lowercase(), expected_signature.to_lowercase());
    }

    #[test]
    fn test_binance_example_3_mixed_payload() {
        // Exact Binance Example 3 - mixed query string and request body
        let query_string = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC";
        let request_body = "quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        
        let payload = SignatureVerifier::build_mixed_payload(query_string, request_body);
        let expected_payload = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTCquantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        
        assert_eq!(payload, expected_payload, "Mixed payload should match Binance Example 3");
        
        // Test signature generation
        let secret_key = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
        let expected_signature = "0fd168b8ddb4876a0358a8d14d0c9f3da0e9b20c5d52b2a00fcf7d1c602f9a77";
        
        let result = SignatureVerifier::verify_hmac(&payload, expected_signature, secret_key).unwrap();
        assert!(result, "Binance Example 3 HMAC verification should pass");
    }

    #[test]
    fn test_build_payload_sorting() {
        let mut query_params = HashMap::new();
        query_params.insert("symbol".to_string(), "BTCUSDT".to_string());
        query_params.insert("side".to_string(), "BUY".to_string());
        query_params.insert("timestamp".to_string(), "1640995200000".to_string());
        query_params.insert("quantity".to_string(), "1".to_string());
        
        let body_params = HashMap::new();
        
        let payload = SignatureVerifier::build_payload(&query_params, &body_params);
        
        // Parameters should be sorted alphabetically
        let expected = "quantity=1&side=BUY&symbol=BTCUSDT&timestamp=1640995200000";
        assert_eq!(payload, expected, "Parameters should be sorted alphabetically");
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
        ).is_ok());
        
        // Future timestamp (beyond 1 second tolerance)
        assert!(SignatureVerifier::validate_timestamp(
            server_time + 2000, 
            recv_window, 
            server_time
        ).is_err());
        
        // Expired timestamp
        assert!(SignatureVerifier::validate_timestamp(
            server_time - 10000, 
            recv_window, 
            server_time
        ).is_err());
    }

    #[test]
    fn test_parse_timestamp() {
        // Milliseconds
        assert_eq!(SignatureVerifier::parse_timestamp("1640995200000").unwrap(), 1640995200000);
        
        // Microseconds (should be converted to milliseconds)
        assert_eq!(SignatureVerifier::parse_timestamp("1640995200000000").unwrap(), 1640995200000);
        
        // Invalid format
        assert!(SignatureVerifier::parse_timestamp("invalid").is_err());
    }

    #[test]
    fn test_signature_parameter_removal() {
        let query_with_sig = "symbol=BTCUSDT&side=BUY&signature=abc123&timestamp=1640995200000";
        let clean_query = SignatureVerifier::remove_signature_from_query(query_with_sig);
        assert_eq!(clean_query, "symbol=BTCUSDT&side=BUY&timestamp=1640995200000");
        
        let body_with_sig = "quantity=1&signature=def456&price=0.1";
        let clean_body = SignatureVerifier::remove_signature_from_body(body_with_sig);
        assert_eq!(clean_body, "quantity=1&price=0.1");
    }

    #[test]
    fn test_url_encoding() {
        let signature_with_special_chars = "abc+def/ghi=";
        let encoded = SignatureVerifier::url_encode_signature(signature_with_special_chars);
        assert_eq!(encoded, "abc%2Bdef%2Fghi%3D");
    }
}
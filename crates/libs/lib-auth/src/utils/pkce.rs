//! PKCE (Proof Key for Code Exchange) utilities
//!
//! This module provides S256 challenge computation for the mobile OAuth flow's
//! "second hop" PKCE-like verification.

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::error::auth_error::AuthError;

/// Computes BASE64URL_NO_PAD(SHA256(verifier)) per RFC 7636 (S256 method)
///
/// # Arguments
///
/// * `verifier` - The plaintext verifier string (43-128 characters per RFC 7636)
///
/// # Returns
///
/// The S256 challenge string, or an error if the verifier length is invalid.
///
/// # Example
///
/// ```rust,ignore
/// let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
/// let challenge = s256_challenge(verifier)?;
/// // challenge is the base64url-encoded SHA256 hash
/// ```
pub fn s256_challenge(verifier: &str) -> Result<String, AuthError> {
    // RFC 7636 specifies verifier must be 43-128 characters
    let n = verifier.len();
    if !(43..=128).contains(&n) {
        return Err(AuthError::InvalidRequest);
    }

    let hash = Sha256::digest(verifier.as_bytes());
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_s256_challenge_valid() {
        // RFC 7636 Appendix B example
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let result = s256_challenge(verifier);
        assert!(result.is_ok());
        // The expected challenge for this verifier
        assert_eq!(
            result.unwrap(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn test_s256_challenge_too_short() {
        let verifier = "short"; // less than 43 chars
        let result = s256_challenge(verifier);
        assert!(result.is_err());
    }

    #[test]
    fn test_s256_challenge_too_long() {
        let verifier = "a".repeat(129); // more than 128 chars
        let result = s256_challenge(&verifier);
        assert!(result.is_err());
    }

    #[test]
    fn test_s256_challenge_min_length() {
        let verifier = "a".repeat(43); // exactly 43 chars
        let result = s256_challenge(&verifier);
        assert!(result.is_ok());
    }

    #[test]
    fn test_s256_challenge_max_length() {
        let verifier = "a".repeat(128); // exactly 128 chars
        let result = s256_challenge(&verifier);
        assert!(result.is_ok());
    }
}

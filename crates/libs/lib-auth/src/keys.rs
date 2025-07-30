use crate::error::AuthError;
use jsonwebtoken::{DecodingKey, EncodingKey};

/// Loads an RSA private key from a PEM file for signing JWTs.
pub fn load_private_key(path: &str) -> Result<EncodingKey, AuthError> {
    let key_data = std::fs::read(path)
        .map_err(|e| AuthError::Config(format!("Failed to read private key from {}: {}", path, e)))?;
    EncodingKey::from_rsa_pem(&key_data)
        .map_err(|e| AuthError::Config(format!("Failed to parse private key: {}", e)))
}

/// Loads an RSA public key from a PEM file for verifying JWTs.
pub fn load_public_key(path: &str) -> Result<DecodingKey, AuthError> {
    let key_data = std::fs::read(path)
        .map_err(|e| AuthError::Config(format!("Failed to read public key from {}: {}", path, e)))?;
    DecodingKey::from_rsa_pem(&key_data)
        .map_err(|e| AuthError::Config(format!("Failed to parse public key: {}", e)))
}
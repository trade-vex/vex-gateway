use crate::error::AuthError;
use jsonwebtoken::{DecodingKey, EncodingKey};
use zeroize::Zeroize;

/// Loads an RSA private key from a PEM file for signing JWTs.
pub fn load_private_key(path: impl AsRef<std::path::Path>) -> Result<EncodingKey, AuthError> {
    let path_ref = path.as_ref();
    let mut key_data = std::fs::read(path_ref)
        .map_err(|e| AuthError::Config(format!("Failed to read private key from {}: {}", path_ref.display(), e)))?;
    
    let result = EncodingKey::from_rsa_pem(&key_data)
        .map_err(|e| AuthError::Config(format!("Failed to parse private key from {}: {}", path_ref.display(), e)));
    
    // Securely zeroize the key data
    key_data.zeroize();
    
    result
}

/// Loads an RSA public key from a PEM file for verifying JWTs.
pub fn load_public_key(path: &str) -> Result<DecodingKey, AuthError> {
    let key_data = std::fs::read(path)
        .map_err(|e| AuthError::Config(format!("Failed to read public key from {}: {}", path, e)))?;
    DecodingKey::from_rsa_pem(&key_data)
        .map_err(|e| AuthError::Config(format!("Failed to parse public key: {}", e)))
}
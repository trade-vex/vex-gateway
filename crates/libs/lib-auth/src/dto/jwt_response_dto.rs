use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct JwtTokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64, // TTL in seconds for the access token
}

/// The claims embedded within our application's JWT.
#[derive(Serialize, Deserialize, Clone)]
pub struct AppClaims {
    // Standard claims
    pub sub: String, // Subject (our internal user ID)
    pub exp: i64,    // Expiration time
    pub iat: i64,    // Issued at
    pub iss: String, // Issuer (our app)
    pub aud: String, // Audience (the intended recipient, e.g., "api")

    // Custom claims
    pub jti: String, // JWT ID (for revocation)
}

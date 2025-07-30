use serde::{Deserialize, Serialize};

/// The query parameters received from the OIDC provider during the callback.
#[derive(Deserialize, Debug)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

/// The response sent to the client after a successful login or token refresh.
#[derive(Serialize, Debug)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64, // TTL in seconds for the access token
}

/// The claims embedded within our application's JWT.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppClaims {
    // Standard claims
    pub sub: String,    // Subject (our internal user ID)
    pub exp: usize,     // Expiration time
    pub iat: usize,     // Issued at
    pub iss: String,    // Issuer (our app)
    pub aud: String,    // Audience (the intended recipient, e.g., "api")
    
    // Custom claims
    pub jti: String,    // JWT ID (for revocation)
}

/// The request body for the token refresh endpoint.
#[derive(Deserialize, Debug)]
pub struct RefreshTokenRequest {
    pub refresh_token: String,
}
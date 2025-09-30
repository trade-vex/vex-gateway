use serde::{Deserialize, Serialize};
use crate::api_key::signature::SignatureAlgorithm;

/// Security types for API endpoints 
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sea_orm::DeriveActiveEnum, sea_orm::EnumIter)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "security_type")]
pub enum SecurityType {
    /// Endpoint can be accessed freely
    #[sea_orm(string_value = "none")]
    None,
    /// Endpoint requires sending a valid API-Key and signature
    #[sea_orm(string_value = "trade")]
    Trade,
    /// Endpoint requires sending a valid API-Key and signature
    #[sea_orm(string_value = "user_data")]
    UserData,
    /// Endpoint requires sending a valid API-Key
    #[sea_orm(string_value = "user_stream")]
    UserStream,
}

impl Default for SecurityType {
    fn default() -> Self {
        SecurityType::None
    }
}

impl std::fmt::Display for SecurityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecurityType::None => write!(f, "NONE"),
            SecurityType::Trade => write!(f, "TRADE"),
            SecurityType::UserData => write!(f, "USER_DATA"),
            SecurityType::UserStream => write!(f, "USER_STREAM"),
        }
    }
}

/// Key material for different signature algorithms
#[derive(Debug, Clone)]
pub enum KeyMaterial {
    /// HMAC secret key
    HmacSecret(String),
    /// RSA public key in PEM format
    RsaPublicKey(String),
    /// Ed25519 public key in PEM format
    Ed25519PublicKey(String),
}

impl KeyMaterial {
    pub fn algorithm(&self) -> SignatureAlgorithm {
        match self {
            KeyMaterial::HmacSecret(_) => SignatureAlgorithm::HmacSha256,
            KeyMaterial::RsaPublicKey(_) => SignatureAlgorithm::RsaSha256,
            KeyMaterial::Ed25519PublicKey(_) => SignatureAlgorithm::Ed25519,
        }
    }
}

/// API Key configuration and permissions
#[derive(Debug, Clone)]
pub struct ApiKeyConfig {
    pub api_key: String,
    pub key_material: Option<KeyMaterial>,
    pub permissions: Vec<SecurityType>,
    pub recv_window: u64, // Default 5000ms, max 60000ms
    pub enabled: bool,
    pub description: Option<String>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ip_whitelist: Vec<String>,
}

impl ApiKeyConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            key_material: None,
            permissions: vec![SecurityType::None],
            recv_window: 5000,
            enabled: true,
            description: None,
            created_at: Some(chrono::Utc::now()),
            expires_at: None,
            ip_whitelist: vec![],
        }
    }

    /// Configure HMAC secret key for signing
    pub fn with_hmac_secret(mut self, secret_key: String) -> Self {
        self.key_material = Some(KeyMaterial::HmacSecret(secret_key));
        self
    }

    /// Configure RSA public key for signature verification
    pub fn with_rsa_public_key(mut self, public_key_pem: String) -> Self {
        self.key_material = Some(KeyMaterial::RsaPublicKey(public_key_pem));
        self
    }

    /// Configure Ed25519 public key for signature verification
    pub fn with_ed25519_public_key(mut self, public_key_pem: String) -> Self {
        self.key_material = Some(KeyMaterial::Ed25519PublicKey(public_key_pem));
        self
    }

    /// Set specific permissions for this API key
    pub fn with_permissions(mut self, permissions: Vec<SecurityType>) -> Self {
        self.permissions = permissions;
        self
    }

    /// Set receive window (max 60000ms as per Binance spec)
    pub fn with_recv_window(mut self, recv_window: u64) -> Self {
        self.recv_window = recv_window.min(60000);
        self
    }

    /// Set enabled/disabled state
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set description for the API key
    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    /// Set expiration time
    pub fn with_expiration(mut self, expires_at: chrono::DateTime<chrono::Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Set IP whitelist
    pub fn with_ip_whitelist(mut self, ip_list: Vec<String>) -> Self {
        self.ip_whitelist = ip_list;
        self
    }

    /// Check if API key has permission for the required security type
    pub fn has_permission(&self, required: SecurityType) -> bool {
        if required == SecurityType::None {
            return true;
        }
        self.permissions.contains(&required)
    }

    /// Check if API key is currently valid (enabled and not expired)
    pub fn is_valid(&self) -> bool {
        if !self.enabled {
            return false;
        }

        if let Some(expires_at) = self.expires_at {
            if chrono::Utc::now() > expires_at {
                return false;
            }
        }

        true
    }

    /// Check if the request IP is allowed
    pub fn is_ip_allowed(&self, ip: &str) -> bool {
        if self.ip_whitelist.is_empty() {
            return true; // No whitelist means all IPs are allowed
        }
        
        self.ip_whitelist.contains(&ip.to_string())
    }

    /// Get the signature algorithm for this API key
    pub fn signature_algorithm(&self) -> Option<SignatureAlgorithm> {
        self.key_material.as_ref().map(|km| km.algorithm())
    }

    /// Get the key material for signature verification
    pub fn get_key_material(&self) -> Option<&KeyMaterial> {
        self.key_material.as_ref()
    }

    /// Check if this API key requires signatures (TRADE or USER_DATA)
    pub fn requires_signature(&self, security_type: SecurityType) -> bool {
        matches!(security_type, SecurityType::Trade | SecurityType::UserData)
    }
}

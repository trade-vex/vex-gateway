use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use sea_orm::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_key::security::{KeyMaterial, SecurityType};

#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "api_keys")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: Uuid,
    pub api_key: String,
    pub key_material_type: Option<KeyMaterialType>,
    pub key_material_value: Option<String>,
    // ✅ change from Json → Vec<SecurityType>
    pub permissions: Vec<SecurityType>,
    pub recv_window: i64,
    pub enabled: bool,
    pub description: Option<String>,
    pub ip_blacklist: Vec<String>,
    pub user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "key_material_type")]
pub enum KeyMaterialType {
    #[sea_orm(string_value = "hmac_secret")]
    HmacSecret,
    #[sea_orm(string_value = "rsa_public_key")]
    RsaPublicKey,
    #[sea_orm(string_value = "ed25519_public_key")]
    Ed25519PublicKey,
}

impl From<&KeyMaterial> for KeyMaterialType {
    fn from(key_material: &KeyMaterial) -> Self {
        match key_material {
            KeyMaterial::HmacSecret(_) => KeyMaterialType::HmacSecret,
            KeyMaterial::RsaPublicKey(_) => KeyMaterialType::RsaPublicKey,
            KeyMaterial::Ed25519PublicKey(_) => KeyMaterialType::Ed25519PublicKey,
        }
    }
}

impl KeyMaterialType {
    pub fn to_key_material(&self, value: &str) -> KeyMaterial {
        match self {
            KeyMaterialType::HmacSecret => KeyMaterial::HmacSecret(value.to_string()),
            KeyMaterialType::RsaPublicKey => KeyMaterial::RsaPublicKey(value.to_string()),
            KeyMaterialType::Ed25519PublicKey => KeyMaterial::Ed25519PublicKey(value.to_string()),
        }
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Convert database model to API key configuration
    pub fn to_api_key_config(&self) -> crate::api_key::security::ApiKeyConfig {
        let key_material = if let (Some(key_type), Some(key_value)) =
            (&self.key_material_type, &self.key_material_value)
        {
            Some(key_type.to_key_material(key_value))
        } else {
            None
        };

        // Parse permissions from JSON
        let _permissions_json = serde_json::to_value(
            self.permissions
                .iter()
                .map(|p| match p {
                    SecurityType::None => "none",
                    SecurityType::Trade => "trade",
                    SecurityType::UserData => "user_data",
                    SecurityType::UserStream => "user_stream",
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or(serde_json::json!(["none"]));

        crate::api_key::security::ApiKeyConfig {
            api_key: self.api_key.clone(),
            key_material,
            permissions: self.permissions.clone(),
            recv_window: self.recv_window as u64,
            enabled: self.enabled,
            description: self.description.clone(),
            created_at: Some(self.created_at),
            expires_at: self.expires_at,
            ip_blacklist: self.ip_blacklist.clone(),
        }
    }

    /// Check if the API key is currently valid (enabled and not expired)
    pub fn is_valid(&self) -> bool {
        self.enabled && (self.expires_at.is_none() || self.expires_at.unwrap() > Utc::now())
    }

    /// Check if the API key has the required permission
    pub fn has_permission(&self, required_permission: &SecurityType) -> bool {
        // let permissions = self
        //     .permissions
        //     .as_array()
        //     .and_then(|arr| {
        //         arr.iter()
        //             .filter_map(|v| v.as_str())
        //             .map(|s| match s {
        //                 "none" => Some(SecurityType::None),
        //                 "trade" => Some(SecurityType::Trade),
        //                 "user_data" => Some(SecurityType::UserData),
        //                 "user_stream" => Some(SecurityType::UserStream),
        //                 _ => None,
        //             })
        //             .collect::<Option<Vec<_>>>()
        //     })
        //     .unwrap_or_else(|| vec![SecurityType::None]);

        self.permissions.contains(required_permission) || self.permissions.contains(&SecurityType::None)
    }

    /// Check if the IP address is blacklisted (blocked)
    pub fn is_ip_blacklisted(&self, ip: &str) -> bool {
        self.ip_blacklist.contains(&ip.to_string())
    }

    /// Check if the IP address is allowed (not blacklisted)
    pub fn is_ip_allowed(&self, ip: &str) -> bool {
        !self.is_ip_blacklisted(ip)
    }
}

/// Helper struct for creating new API keys
#[derive(Debug, Clone)]
pub struct NewApiKey {
    pub api_key: String,
    pub key_material: Option<KeyMaterial>,
    pub permissions: Vec<SecurityType>,
    pub recv_window: u64,
    pub description: Option<String>,
    pub ip_blacklist: Vec<String>,
    pub user_id: Option<Uuid>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl NewApiKey {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            key_material: None,
            permissions: vec![SecurityType::None],
            recv_window: 5000,
            description: None,
            ip_blacklist: vec![],
            user_id: None,
            expires_at: None,
        }
    }

    pub fn to_active_model(self) -> ActiveModel {
        let (key_material_type, key_material_value) = if let Some(key_material) = &self.key_material
        {
            (
                Some(KeyMaterialType::from(key_material)),
                Some(match key_material {
                    KeyMaterial::HmacSecret(secret) => secret.clone(),
                    KeyMaterial::RsaPublicKey(key) => key.clone(),
                    KeyMaterial::Ed25519PublicKey(key) => key.clone(),
                }),
            )
        } else {
            (None, None)
        };

        // Convert permissions to JSON
        // let permissions_json = serde_json::to_value(
        //     self.permissions
        //         .iter()
        //         .map(|p| match p {
        //             SecurityType::None => "none",
        //             SecurityType::Trade => "trade",
        //             SecurityType::UserData => "user_data",
        //             SecurityType::UserStream => "user_stream",
        //         })
        //         .collect::<Vec<_>>(),
        // )
        // .unwrap_or(serde_json::json!(["none"]));

        let now = Utc::now();

        ActiveModel {
            id: Set(Uuid::new_v4()),
            api_key: Set(self.api_key),
            key_material_type: Set(key_material_type),
            key_material_value: Set(key_material_value),
            permissions: Set(self.permissions),
            recv_window: Set(self.recv_window as i64),
            enabled: Set(true),
            description: Set(self.description),
            ip_blacklist: Set(self.ip_blacklist),
            user_id: Set(self.user_id),
            created_at: Set(now),
            updated_at: Set(now),
            expires_at: Set(self.expires_at),
            last_used_at: Set(None),
        }
    }
}

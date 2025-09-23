//! lib-api/src/handlers/api_keys.rs

use actix_web::{web, HttpResponse, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};

use lib_middleware::{
    repository::api_key_repository::ApiKeyRepository,
    models::api_key::{NewApiKey, ActiveModel, Model as ApiKeyModel},
    api_key::security::{SecurityType, KeyMaterial},
};
use sea_orm::Set;

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub description: Option<String>,
    pub permissions: Vec<String>,
    pub recv_window: Option<u64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub ip_blacklist: Option<Vec<String>>,
    pub key_material_type: Option<String>,
    pub key_material_value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateApiKeyRequest {
    pub permissions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyResponse {
    pub id: Uuid,
    pub api_key: String,
    pub permissions: Vec<String>,
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

#[derive(Debug, Serialize)]
pub struct ApiKeyListResponse {
    pub api_keys: Vec<ApiKeyResponse>,
    pub total: usize,
}

impl From<ApiKeyModel> for ApiKeyResponse {
    fn from(model: ApiKeyModel) -> Self {
        // parsing permissions from JSON
        let permissions: Vec<String> = serde_json::from_value(model.permissions)
            .unwrap_or_else(|_| vec!["none".to_string()]);

        Self {
            id: model.id,
            api_key: model.api_key,
            permissions,
            recv_window: model.recv_window,
            enabled: model.enabled,
            description: model.description,
            ip_blacklist: model.ip_blacklist,
            user_id: model.user_id,
            created_at: model.created_at,
            updated_at: model.updated_at,
            expires_at: model.expires_at,
            last_used_at: model.last_used_at,
        }
    }
}

/// parse security type from string
fn parse_security_type(s: &str) -> Result<SecurityType, String> {
    match s.to_lowercase().as_str() {
        "none" => Ok(SecurityType::None),
        "trade" => Ok(SecurityType::Trade),
        "user_data" => Ok(SecurityType::UserData),
        "user_stream" => Ok(SecurityType::UserStream),
        _ => Err(format!("Invalid security type: {}", s)),
    }
}

/// parse key material from type and value
fn parse_key_material(key_type: &str, value: &str) -> Result<KeyMaterial, String> {
    match key_type.to_lowercase().as_str() {
        "hmac_secret" => Ok(KeyMaterial::HmacSecret(value.to_string())),
        "rsa_public_key" => Ok(KeyMaterial::RsaPublicKey(value.to_string())),
        "ed25519_public_key" => Ok(KeyMaterial::Ed25519PublicKey(value.to_string())),
        _ => Err(format!("Invalid key material type: {}", key_type)),
    }
}

/// create a new API key
pub async fn create_api_key(
    req: web::Json<CreateApiKeyRequest>,
    repo: web::Data<dyn ApiKeyRepository>,
    auth_user: lib_auth::middleware::jwt_auth::AuthenticatedUser,
) -> Result<HttpResponse> {
    // parse permissions
    let permissions: Result<Vec<SecurityType>, String> = req.permissions
        .iter()
        .map(|p| parse_security_type(p))
        .collect();
    
    let permissions = permissions.map_err(|e| {
        actix_web::error::ErrorBadRequest(format!("Invalid permissions: {}", e))
    })?;

    // parse key material if provided
    let key_material = if let (Some(key_type), Some(key_value)) = 
        (&req.key_material_type, &req.key_material_value) {
        Some(parse_key_material(key_type, key_value).map_err(|e| {
            actix_web::error::ErrorBadRequest(format!("Invalid key material: {}", e))
        })?)
    } else {
        None
    };

    // generating a new API key
    let api_key = format!("vex_{}", Uuid::new_v4().to_string().replace('-', ""));

    // create new API key
    let new_api_key = NewApiKey {
        api_key: api_key.clone(),
        key_material,
        permissions,
        recv_window: req.recv_window.unwrap_or(5000),
        description: req.description.clone(),
        ip_blacklist: req.ip_blacklist.clone().unwrap_or_default(),
        user_id: Some(auth_user.user_id),
        expires_at: req.expires_at,
    };

    match repo.create(new_api_key).await {
        Ok(created_key) => {
            let response = ApiKeyResponse::from(created_key);
            Ok(HttpResponse::Created().json(response))
        }
        Err(e) => {
            tracing::error!("Failed to create API key: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to create API key",
                "message": e.to_string()
            })))
        }
    }
}

/// get all API keys for the authenticated user
pub async fn list_api_keys(
    repo: web::Data<dyn ApiKeyRepository>,
    auth_user: lib_auth::middleware::jwt_auth::AuthenticatedUser,
) -> Result<HttpResponse> {
    match repo.find_by_user_id(auth_user.user_id).await {
        Ok(api_keys) => {
            let total = api_keys.len();
            let response = ApiKeyListResponse {
                api_keys: api_keys.into_iter().map(ApiKeyResponse::from).collect(),
                total,
            };
            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("Failed to list API keys: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to list API keys",
                "message": e.to_string()
            })))
        }
    }
}

/// get a specific API key by ID
pub async fn get_api_key(
    path: web::Path<Uuid>,
    repo: web::Data<dyn ApiKeyRepository>,
    auth_user: lib_auth::middleware::jwt_auth::AuthenticatedUser,
) -> Result<HttpResponse> {
    let api_key_id = path.into_inner();

    match repo.find_by_id(api_key_id).await {
        Ok(Some(api_key)) => {
            // checking if the API key belongs to the authenticated user
            if api_key.user_id != Some(auth_user.user_id) {
                return Ok(HttpResponse::Forbidden().json(serde_json::json!({
                    "error": "Access denied",
                    "message": "You don't have permission to access this API key"
                })));
            }

            let response = ApiKeyResponse::from(api_key);
            Ok(HttpResponse::Ok().json(response))
        }
        Ok(None) => {
            Ok(HttpResponse::NotFound().json(serde_json::json!({
                "error": "API key not found"
            })))
        }
        Err(e) => {
            tracing::error!("Failed to get API key: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to get API key",
                "message": e.to_string()
            })))
        }
    }
}

/// update an API key
pub async fn update_api_key(
    path: web::Path<Uuid>,
    req: web::Json<UpdateApiKeyRequest>,
    repo: web::Data<dyn ApiKeyRepository>,
    auth_user: lib_auth::middleware::jwt_auth::AuthenticatedUser,
) -> Result<HttpResponse> {
    let api_key_id = path.into_inner();

    // checking if the API key exists and belongs to the user
    match repo.find_by_id(api_key_id).await {
        Ok(Some(api_key)) => {
            if api_key.user_id != Some(auth_user.user_id) {
                return Ok(HttpResponse::Forbidden().json(serde_json::json!({
                    "error": "Access denied",
                    "message": "You don't have permission to update this API key"
                })));
            }
        }
        Ok(None) => {
            return Ok(HttpResponse::NotFound().json(serde_json::json!({
                "error": "API key not found"
            })));
        }
        Err(e) => {
            tracing::error!("Failed to find API key: {}", e);
            return Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to find API key",
                "message": e.to_string()
            })));
        }
    }

    // parsing permissions
    let permissions: Result<Vec<SecurityType>, String> = req.permissions
        .iter()
        .map(|p| parse_security_type(p))
        .collect();
    
    let permissions = permissions.map_err(|e| {
        actix_web::error::ErrorBadRequest(format!("Invalid permissions: {}", e))
    })?;

    // convert permissions to JSON for database storage
    let permissions_json = serde_json::to_value(
        permissions.iter()
            .map(|p| match p {
                SecurityType::None => "none",
                SecurityType::Trade => "trade",
                SecurityType::UserData => "user_data",
                SecurityType::UserStream => "user_stream",
            })
            .collect::<Vec<_>>()
    ).unwrap_or(serde_json::json!(["none"]));

    let updates = ActiveModel {
        id: sea_orm::Set(api_key_id),
        permissions: Set(permissions_json.into()),
        ..Default::default()
    };

    match repo.update(api_key_id, updates).await {
        Ok(updated_key) => {
            let response = ApiKeyResponse::from(updated_key);
            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!("Failed to update API key: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to update API key",
                "message": e.to_string()
            })))
        }
    }
}

/// delete an API key (soft delete by disabling)
pub async fn delete_api_key(
    path: web::Path<Uuid>,
    repo: web::Data<dyn ApiKeyRepository>,
    auth_user: lib_auth::middleware::jwt_auth::AuthenticatedUser,
) -> Result<HttpResponse> {
    let api_key_id = path.into_inner();

    // first, check if the API key exists and belongs to the user
    match repo.find_by_id(api_key_id).await {
        Ok(Some(api_key)) => {
            if api_key.user_id != Some(auth_user.user_id) {
                return Ok(HttpResponse::Forbidden().json(serde_json::json!({
                    "error": "Access denied",
                    "message": "You don't have permission to delete this API key"
                })));
            }
        }
        Ok(None) => {
            return Ok(HttpResponse::NotFound().json(serde_json::json!({
                "error": "API key not found"
            })));
        }
        Err(e) => {
            tracing::error!("Failed to find API key: {}", e);
            return Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to find API key",
                "message": e.to_string()
            })));
        }
    }

    match repo.delete(api_key_id).await {
        Ok(_) => {
            Ok(HttpResponse::Ok().json(serde_json::json!({
                "message": "API key deleted successfully"
            })))
        }
        Err(e) => {
            tracing::error!("Failed to delete API key: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to delete API key",
                "message": e.to_string()
            })))
        }
    }
}

/// Enable/disable an API key
pub async fn toggle_api_key(
    path: web::Path<Uuid>,
    req: web::Json<serde_json::Value>,
    repo: web::Data<dyn ApiKeyRepository>,
    auth_user: lib_auth::middleware::jwt_auth::AuthenticatedUser,
) -> Result<HttpResponse> {
    let api_key_id = path.into_inner();
    let enabled = req.get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| actix_web::error::ErrorBadRequest("Missing or invalid 'enabled' field"))?;

    // First, check if the API key exists and belongs to the user
    match repo.find_by_id(api_key_id).await {
        Ok(Some(api_key)) => {
            if api_key.user_id != Some(auth_user.user_id) {
                return Ok(HttpResponse::Forbidden().json(serde_json::json!({
                    "error": "Access denied",
                    "message": "You don't have permission to modify this API key"
                })));
            }
        }
        Ok(None) => {
            return Ok(HttpResponse::NotFound().json(serde_json::json!({
                "error": "API key not found"
            })));
        }
        Err(e) => {
            tracing::error!("Failed to find API key: {}", e);
            return Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to find API key",
                "message": e.to_string()
            })));
        }
    }

    match repo.set_enabled(api_key_id, enabled).await {
        Ok(_) => {
            let action = if enabled { "enabled" } else { "disabled" };
            Ok(HttpResponse::Ok().json(serde_json::json!({
                "message": format!("API key {} successfully", action)
            })))
        }
        Err(e) => {
            tracing::error!("Failed to toggle API key: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to toggle API key",
                "message": e.to_string()
            })))
        }
    }
}

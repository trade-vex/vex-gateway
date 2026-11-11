//! lib-api/src/handlers/api_keys.rs

use actix_web::{HttpResponse, Result, web};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use lib_middleware::{
    api_key::SecurityType,
    api_key::middleware::ApiKeyStore,
    api_key::security::KeyMaterial,
    client_ip::parse_ip_or_net,
    models::api_key::{ActiveModel, Model as ApiKeyModel, NewApiKey},
    repository::api_key_repository::ApiKeyRepository,
};
use sea_orm::Set;
use std::sync::Arc;

const INTERNAL_ERROR_MESSAGE: &str = "An internal error occurred. Please try again later.";

async fn invalidate_api_key_cache(cache: Option<&web::Data<ApiKeyStore>>, api_key: &str) {
    let Some(cache) = cache else {
        tracing::error!("API key cache unavailable after successful API key mutation");
        return;
    };

    cache.invalidate(api_key).await;
}

/// masks an API key for safe serialization
/// keep first 4 and last 4 characters and masks the rest with *
/// for short keys len <= 8 returns "****"
fn mask_api_key<S>(api_key: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let masked = if api_key.len() <= 8 {
        "****".to_string()
    } else {
        let start = &api_key[..4];
        let end = &api_key[api_key.len() - 4..];
        format!("{}****{}", start, end)
    };
    serializer.serialize_str(&masked)
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CreateApiKeyRequest {
    pub description: Option<String>,
    pub permissions: Vec<String>,
    pub recv_window: Option<u64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub ip_blacklist: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateApiKeyRequest {
    pub permissions: Vec<String>,
    pub ip_blacklist: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiKeyResponse {
    pub id: Uuid,
    #[serde(serialize_with = "mask_api_key")]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiKeyListResponse {
    pub api_keys: Vec<ApiKeyResponse>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateApiKeyResponse {
    pub api_key: String,          // Plaintext key - only returned during creation
    pub secret_key: String,       // Secret key for HMAC signing - only returned during creation
    pub key_info: ApiKeyResponse, // Masked key info for consistency
}

impl From<ApiKeyModel> for ApiKeyResponse {
    fn from(model: ApiKeyModel) -> Self {
        // parsing permissions from JSON
        // let permissions: Vec<String> =
        //     serde_json::from_value(model.permissions).unwrap_or_else(|_| vec!["none".to_string()]);

        Self {
            id: model.id,
            api_key: model.api_key,
            permissions: model
                .permissions
                .into_iter()
                .map(|p| p.to_string())
                .collect(),
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

/// A key's permission: `trade` (place and cancel orders) or `user_data` (every other route an API
/// key may reach). No route needs `none` or `user_stream`, so a key holding only those reaches
/// nothing.
fn parse_security_type(s: &str) -> Result<SecurityType, String> {
    match s.to_lowercase().as_str() {
        "trade" => Ok(SecurityType::Trade),
        "user_data" => Ok(SecurityType::UserData),
        _ => Err(format!("Invalid security type: {}", s)),
    }
}

/// Every entry must be an IP address or a CIDR range: the form the API-key middleware matches.
fn validate_ip_blacklist(ip_list: &[String]) -> Result<(), String> {
    match ip_list
        .iter()
        .find(|entry| parse_ip_or_net(entry).is_none())
    {
        Some(entry) => Err(format!("Invalid IP address or CIDR range: {entry}")),
        None => Ok(()),
    }
}

/// create a new API key
pub async fn create_api_key(
    req: web::Json<CreateApiKeyRequest>,
    repo: web::Data<Arc<dyn ApiKeyRepository>>,
    auth_user: AuthenticatedUser,
) -> Result<HttpResponse> {
    let user_id = auth_user.user_id;
    // parse permissions
    let permissions: Result<Vec<SecurityType>, String> = req
        .permissions
        .iter()
        .map(|p| parse_security_type(p))
        .collect();

    let permissions = permissions
        .map_err(|e| actix_web::error::ErrorBadRequest(format!("Invalid permissions: {}", e)))?;

    // validate IP blacklist if provided
    if let Some(ref ip_blacklist) = req.ip_blacklist {
        validate_ip_blacklist(ip_blacklist).map_err(|e| {
            actix_web::error::ErrorBadRequest(format!("Invalid IP blacklist: {}", e))
        })?;
    }

    // generating a new API key
    let api_key = format!("vex_{}", Uuid::new_v4().to_string().replace('-', ""));

    // The HMAC secret is always the server's: a client-chosen one may be weak.
    let secret_key = {
        use rand::Rng;
        let secret_bytes: [u8; 32] = rand::thread_rng().r#gen();
        base64::engine::general_purpose::STANDARD.encode(secret_bytes)
    };

    // create new API key with HMAC secret
    let new_api_key = NewApiKey {
        api_key: api_key.clone(),
        key_material: Some(KeyMaterial::HmacSecret(secret_key.clone())),
        permissions,
        recv_window: req.recv_window.unwrap_or(5000).min(60000),
        description: req.description.clone(),
        ip_blacklist: req.ip_blacklist.clone().unwrap_or_default(),
        user_id: Some(user_id),
        expires_at: req.expires_at,
    };

    match repo.as_ref().as_ref().create(new_api_key).await {
        Ok(created_key) => {
            let key_info = ApiKeyResponse::from(created_key);
            let response = CreateApiKeyResponse {
                api_key: api_key.clone(), // Plaintext key - only returned during creation
                secret_key: secret_key.clone(), // Secret key for HMAC signing - only returned during creation
                key_info,                       // Masked key info
            };
            Ok(HttpResponse::Created().json(response))
        }
        Err(e) => {
            tracing::error!(user_id = %user_id, error = %e, "failed to create API key");
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to create API key",
                "message": INTERNAL_ERROR_MESSAGE
            })))
        }
    }
}

/// Key `id` if it belongs to `user_id`, else the response to send (403, 404 or 500).
async fn owned_key(
    repo: &dyn ApiKeyRepository,
    id: Uuid,
    user_id: Uuid,
) -> Result<ApiKeyModel, HttpResponse> {
    match repo.find_by_id(id).await {
        Ok(Some(key)) if key.user_id == Some(user_id) => Ok(key),
        Ok(Some(_)) => Err(HttpResponse::Forbidden().json(serde_json::json!({
            "error": "Access denied",
            "message": "You don't have permission to access this API key"
        }))),
        Ok(None) => Err(HttpResponse::NotFound().json(serde_json::json!({
            "error": "API key not found"
        }))),
        Err(e) => {
            tracing::error!(user_id = %user_id, api_key_id = %id, error = %e, "failed to find API key");
            Err(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to find API key",
                "message": INTERNAL_ERROR_MESSAGE
            })))
        }
    }
}

/// get all API keys for the authenticated user
pub async fn list_api_keys(
    repo: web::Data<Arc<dyn ApiKeyRepository>>,
    auth_user: AuthenticatedUser,
) -> Result<HttpResponse> {
    let user_id = auth_user.user_id;

    match repo.as_ref().as_ref().find_by_user_id(user_id).await {
        Ok(api_keys) => {
            let total = api_keys.len();
            let response = ApiKeyListResponse {
                api_keys: api_keys.into_iter().map(ApiKeyResponse::from).collect(),
                total,
            };
            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!(user_id = %user_id, error = %e, "failed to list API keys");
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to list API keys",
                "message": INTERNAL_ERROR_MESSAGE
            })))
        }
    }
}

/// get a specific API key by ID
pub async fn get_api_key(
    path: web::Path<Uuid>,
    repo: web::Data<Arc<dyn ApiKeyRepository>>,
    auth_user: AuthenticatedUser,
) -> Result<HttpResponse> {
    let api_key_id = path.into_inner();

    match owned_key(repo.as_ref().as_ref(), api_key_id, auth_user.user_id).await {
        Ok(api_key) => Ok(HttpResponse::Ok().json(ApiKeyResponse::from(api_key))),
        Err(response) => Ok(response),
    }
}

/// update an API key
pub async fn update_api_key(
    path: web::Path<Uuid>,
    req: web::Json<UpdateApiKeyRequest>,
    repo: web::Data<Arc<dyn ApiKeyRepository>>,
    cache: Option<web::Data<ApiKeyStore>>,
    auth_user: AuthenticatedUser,
) -> Result<HttpResponse> {
    let api_key_id = path.into_inner();

    let user_id = auth_user.user_id;

    let api_key = match owned_key(repo.as_ref().as_ref(), api_key_id, user_id).await {
        Ok(api_key) => api_key,
        Err(response) => return Ok(response),
    };

    // parsing permissions
    let permissions: Result<Vec<SecurityType>, String> = req
        .permissions
        .iter()
        .map(|p| parse_security_type(p))
        .collect();

    let permissions = permissions
        .map_err(|e| actix_web::error::ErrorBadRequest(format!("Invalid permissions: {}", e)))?;

    // validate IP blacklist if provided
    if let Some(ref ip_blacklist) = req.ip_blacklist {
        validate_ip_blacklist(ip_blacklist).map_err(|e| {
            actix_web::error::ErrorBadRequest(format!("Invalid IP blacklist: {}", e))
        })?;
    }

    let mut updates = ActiveModel {
        id: sea_orm::Set(api_key_id),
        permissions: Set(permissions),
        ..Default::default()
    };

    // Update IP blacklist if provided
    if let Some(ip_blacklist) = req.ip_blacklist.clone() {
        updates.ip_blacklist = Set(ip_blacklist);
    }

    match repo.as_ref().update(api_key_id, updates).await {
        Ok(updated_key) => {
            invalidate_api_key_cache(cache.as_ref(), &api_key.api_key).await;
            let response = ApiKeyResponse::from(updated_key);
            Ok(HttpResponse::Ok().json(response))
        }
        Err(e) => {
            tracing::error!(user_id = %user_id, api_key_id = %api_key_id, error = %e, "failed to update API key");
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to update API key",
                "message": INTERNAL_ERROR_MESSAGE
            })))
        }
    }
}

/// delete an API key
pub async fn delete_api_key(
    path: web::Path<Uuid>,
    repo: web::Data<Arc<dyn ApiKeyRepository>>,
    cache: Option<web::Data<ApiKeyStore>>,
    auth_user: AuthenticatedUser,
) -> Result<HttpResponse> {
    let api_key_id = path.into_inner();

    let user_id = auth_user.user_id;

    let api_key = match owned_key(repo.as_ref().as_ref(), api_key_id, user_id).await {
        Ok(api_key) => api_key,
        Err(response) => return Ok(response),
    };

    match repo.as_ref().delete(api_key_id).await {
        Ok(_) => {
            invalidate_api_key_cache(cache.as_ref(), &api_key.api_key).await;
            Ok(HttpResponse::Ok().json(serde_json::json!({
                "message": "API key deleted successfully"
            })))
        }
        Err(e) => {
            tracing::error!(user_id = %user_id, api_key_id = %api_key_id, error = %e, "failed to delete API key");
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to delete API key",
                "message": INTERNAL_ERROR_MESSAGE
            })))
        }
    }
}

/// Enable/disable an API key
pub async fn toggle_api_key(
    path: web::Path<Uuid>,
    req: web::Json<serde_json::Value>,
    repo: web::Data<Arc<dyn ApiKeyRepository>>,
    cache: Option<web::Data<ApiKeyStore>>,
    auth_user: AuthenticatedUser,
) -> Result<HttpResponse> {
    let api_key_id = path.into_inner();
    let enabled = req
        .get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| actix_web::error::ErrorBadRequest("Missing or invalid 'enabled' field"))?;

    let user_id = auth_user.user_id;

    let api_key = match owned_key(repo.as_ref().as_ref(), api_key_id, user_id).await {
        Ok(api_key) => api_key,
        Err(response) => return Ok(response),
    };

    match repo.as_ref().set_enabled(api_key_id, enabled).await {
        Ok(_) => {
            if !enabled {
                invalidate_api_key_cache(cache.as_ref(), &api_key.api_key).await;
            }
            let action = if enabled { "enabled" } else { "disabled" };
            Ok(HttpResponse::Ok().json(serde_json::json!({
                "message": format!("API key {} successfully", action)
            })))
        }
        Err(e) => {
            tracing::error!(user_id = %user_id, api_key_id = %api_key_id, error = %e, "failed to toggle API key");
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to toggle API key",
                "message": INTERNAL_ERROR_MESSAGE
            })))
        }
    }
}

use actix_web::{web, HttpResponse, Result, Error};
use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize)]
pub struct UserProfile {
    pub user_id: String,
    pub email: Option<String>,
    pub username: Option<String>,
    pub auth_provider: String,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct UserDashboard {
    pub user_id: String,
    pub recent_activity: Vec<String>,
    pub account_summary: AccountSummary,
    pub api_keys_count: usize,
}

#[derive(Serialize)]
pub struct AccountSummary {
    pub total_balance: String,
    pub open_orders: u32,
    pub trading_enabled: bool,
}

#[derive(Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub permissions: Vec<String>,
    pub ip_whitelist: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct ApiKeyResponse {
    pub api_key: String,
    pub secret_key: String,
    pub name: String,
    pub permissions: Vec<String>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ApiKeyListItem {
    pub api_key: String,
    pub name: String,
    pub permissions: Vec<String>,
    pub created_at: String,
    pub last_used: Option<String>,
    pub enabled: bool,
}

// User Profile Endpoints
pub async fn get_user_profile(
    user: AuthenticatedUser,
) -> Result<HttpResponse, Error> {
    let profile = UserProfile {
        user_id: user.user_id.to_string(),
        email: user.email,
        username: user.username,
        auth_provider: user.auth_provider,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    Ok(HttpResponse::Ok().json(profile))
}

pub async fn get_user_dashboard(
    user: AuthenticatedUser,
) -> Result<HttpResponse, Error> {
    let dashboard = UserDashboard {
        user_id: user.user_id.to_string(),
        recent_activity: vec![
            "Logged in via GitHub".to_string(),
            "Created new API key".to_string(),
        ],
        account_summary: AccountSummary {
            total_balance: "0.00".to_string(),
            open_orders: 0,
            trading_enabled: true,
        },
        api_keys_count: 0, // In real app, fetch from database
    };

    Ok(HttpResponse::Ok().json(dashboard))
}

pub async fn update_user_settings(
    user: AuthenticatedUser,
    settings: web::Json<serde_json::Value>,
) -> Result<HttpResponse, Error> {
    // In real implementation, update user settings in database
    Ok(HttpResponse::Ok().json(json!({
        "user_id": user.user_id,
        "message": "Settings updated successfully",
        "updated_settings": settings.into_inner()
    })))
}

// API Key Management for Users
pub async fn create_api_key(
    _user: AuthenticatedUser,
    req: web::Json<CreateApiKeyRequest>,
) -> Result<HttpResponse, Error> {
    // Generate API key and secret (in real app, store in database)
    let api_key = generate_api_key();
    let secret_key = generate_secret_key();
    
    let response = ApiKeyResponse {
        api_key,
        secret_key,
        name: req.name.clone(),
        permissions: req.permissions.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    Ok(HttpResponse::Created().json(response))
}

pub async fn list_api_keys(
    user: AuthenticatedUser,
) -> Result<HttpResponse, Error> {
    // In real implementation, fetch from database
    let api_keys = vec![
        ApiKeyListItem {
            api_key: "vmPUZE6mv9SD5VNHk4HlWFsOr6aKE2zvsw0MuIgwCIPy6utIco14y7Ju91duEh8A".to_string(),
            name: "Trading Bot".to_string(),
            permissions: vec!["TRADE".to_string(), "USER_DATA".to_string()],
            created_at: chrono::Utc::now().to_rfc3339(),
            last_used: Some(chrono::Utc::now().to_rfc3339()),
            enabled: true,
        },
    ];

    Ok(HttpResponse::Ok().json(json!({
        "user_id": user.user_id,
        "api_keys": api_keys
    })))
}

pub async fn delete_api_key(
    user: AuthenticatedUser,
    path: web::Path<String>,
) -> Result<HttpResponse, Error> {
    let api_key = path.into_inner();
    
    // In real implementation, delete from database
    Ok(HttpResponse::Ok().json(json!({
        "user_id": user.user_id,
        "message": format!("API key {} deleted successfully", api_key)
    })))
}

// Helper functions for generating keys
fn generate_api_key() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..64).map(|_| {
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        chars[rng.gen_range(0..chars.len())] as char
    }).collect()
}

fn generate_secret_key() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..64).map(|_| {
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        chars[rng.gen_range(0..chars.len())] as char
    }).collect()
}
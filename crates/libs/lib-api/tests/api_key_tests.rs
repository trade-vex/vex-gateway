//! Unit tests for API key handlers
//!
//! These tests use the actual Postgres implementation of ApiKeyRepository
//! to test the real CRUD operations with the database, on the read model's schema.

mod support;

use actix_web::{HttpResponse, web};
use chrono::Utc;
use sea_orm::DatabaseConnection;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use support::require_database;
use uuid::Uuid;

use lib_api::handlers::api_key::{
    ApiKeyListResponse, ApiKeyResponse, CreateApiKeyRequest, ToggleApiKeyRequest,
    UpdateApiKeyRequest, create_api_key, delete_api_key, get_api_key, list_api_keys,
    toggle_api_key, update_api_key,
};
use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use lib_middleware::models::api_key::NewApiKey;
use lib_middleware::repository::api_key_repository::{ApiKeyRepository, PostgresApiKeyRepository};

fn repository(db: &DatabaseConnection) -> web::Data<Arc<dyn ApiKeyRepository>> {
    web::Data::new(Arc::new(PostgresApiKeyRepository::new(Arc::new(
        db.clone(),
    ))))
}

fn create_test_user() -> AuthenticatedUser {
    AuthenticatedUser {
        user_id: Uuid::new_v4(),
    }
}

fn create_test_request() -> CreateApiKeyRequest {
    CreateApiKeyRequest {
        description: Some("Test API Key".to_string()),
        permissions: vec!["trade".to_string(), "user_data".to_string()],
        recv_window: Some(5000),
        expires_at: Some(Utc::now() + chrono::Duration::days(30)),
        ip_blacklist: Some(vec!["192.168.1.1".to_string()]),
    }
}

/// Creates a key for `user` from [`create_test_request`] and returns its id.
async fn created_key(
    repo: &web::Data<Arc<dyn ApiKeyRepository>>,
    user: &AuthenticatedUser,
) -> Uuid {
    let response = create_api_key(web::Json(create_test_request()), repo.clone(), user.clone())
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    repo.find_by_user_id(user.user_id).await.unwrap()[0].id
}

async fn json<T: DeserializeOwned>(response: HttpResponse) -> T {
    let bytes = actix_web::body::to_bytes(response.into_body())
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// The lookup every API-key request makes answers only for enabled keys that have not
/// expired: it is what makes a disabled or expired key stop authenticating.
#[tokio::test]
async fn find_by_api_key_returns_only_enabled_unexpired_keys() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let day = chrono::Duration::days(1);
    for (key, expires_at) in [
        ("live", None),
        ("off", None),
        ("stale", Some(Utc::now() - day)),
        ("soon", Some(Utc::now() + day)),
    ] {
        let row = repo
            .create(NewApiKey {
                expires_at,
                ..NewApiKey::new(key.to_string())
            })
            .await
            .unwrap();
        if key == "off" {
            repo.set_enabled(row.id, false).await.unwrap();
        }
    }

    for (key, found) in [
        ("live", true),
        ("off", false),
        ("stale", false),
        ("soon", true),
    ] {
        let row = repo.find_by_api_key(key).await.unwrap();
        assert_eq!(row.is_some(), found, "{key}");
    }
}

#[tokio::test]
async fn test_create_api_key_success() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let user = create_test_user();
    let request = create_test_request();

    let response = create_api_key(web::Json(request.clone()), repo.clone(), user.clone())
        .await
        .unwrap();

    assert_eq!(response.status(), 201);
    let user_keys = repo.find_by_user_id(user.user_id).await.unwrap();
    assert_eq!(user_keys.len(), 1);
    let api_key_model = &user_keys[0];
    assert_eq!(api_key_model.user_id, Some(user.user_id));
    assert_eq!(api_key_model.description, request.description);
    assert!(api_key_model.api_key.starts_with("vex_"));
    assert!(api_key_model.enabled);
}

#[tokio::test]
async fn test_create_api_key_invalid_permissions() {
    let Some(db) = require_database().await else {
        return;
    };
    let mut request = create_test_request();
    request.permissions = vec!["invalid_permission".to_string()];

    let error = create_api_key(web::Json(request), repository(&db), create_test_user())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("Invalid permissions"));
}

#[tokio::test]
async fn test_list_api_keys_success() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let user = create_test_user();
    created_key(&repo, &user).await;
    let second = CreateApiKeyRequest {
        description: Some("Second API Key".to_string()),
        permissions: vec!["user_data".to_string()],
        recv_window: Some(10000),
        expires_at: None,
        ip_blacklist: None,
    };
    create_api_key(web::Json(second), repo.clone(), user.clone())
        .await
        .unwrap();

    let response = list_api_keys(repo.clone(), user.clone()).await.unwrap();

    assert_eq!(response.status(), 200);
    let list_response: ApiKeyListResponse = json(response).await;
    assert_eq!(list_response.total, 2);
    assert_eq!(list_response.api_keys.len(), 2);
    for api_key_response in &list_response.api_keys {
        assert_eq!(api_key_response.user_id, Some(user.user_id));
        assert!(api_key_response.api_key.starts_with("vex_"));
        assert!(api_key_response.enabled);
        // Verifying API keys are masked in the response
        assert!(api_key_response.api_key.contains("*"));
    }
    let descriptions: Vec<&String> = list_response
        .api_keys
        .iter()
        .filter_map(|key| key.description.as_ref())
        .collect();
    assert!(descriptions.contains(&&"Test API Key".to_string()));
    assert!(descriptions.contains(&&"Second API Key".to_string()));
}

#[tokio::test]
async fn test_get_api_key_success() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let user = create_test_user();
    let api_key_id = created_key(&repo, &user).await;

    let response = get_api_key(web::Path::from(api_key_id), repo.clone(), user.clone())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let api_key_response: ApiKeyResponse = json(response).await;
    assert_eq!(api_key_response.id, api_key_id);
    assert_eq!(api_key_response.user_id, Some(user.user_id));
    assert_eq!(
        api_key_response.description,
        create_test_request().description
    );
    assert!(api_key_response.api_key.starts_with("vex_"));
    assert!(api_key_response.enabled);
    // Verifying API key is masked in the response
    assert!(api_key_response.api_key.contains("*"));
}

#[tokio::test]
async fn test_get_api_key_not_found() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let non_existent_id = Uuid::new_v4();

    let response = get_api_key(
        web::Path::from(non_existent_id),
        repo.clone(),
        create_test_user(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), 404);
    assert!(repo.find_by_id(non_existent_id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_get_api_key_access_denied() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let (user1, user2) = (create_test_user(), create_test_user());
    let api_key_id = created_key(&repo, &user1).await;

    // Try to get API key with user2: should be denied
    let response = get_api_key(web::Path::from(api_key_id), repo.clone(), user2.clone())
        .await
        .unwrap();

    assert_eq!(response.status(), 403);
    let api_key = repo.find_by_id(api_key_id).await.unwrap().unwrap();
    assert_eq!(api_key.user_id, Some(user1.user_id));
}

#[tokio::test]
async fn test_update_api_key_success() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let user = create_test_user();
    let api_key_id = created_key(&repo, &user).await;
    let update_request = UpdateApiKeyRequest {
        permissions: vec!["user_data".to_string()],
        ip_blacklist: None,
    };

    let response = update_api_key(
        web::Path::from(api_key_id),
        web::Json(update_request),
        repo.clone(),
        None,
        user.clone(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), 200);
    let api_key_response: ApiKeyResponse = json(response).await;
    assert_eq!(api_key_response.id, api_key_id);
    assert_eq!(api_key_response.user_id, Some(user.user_id));
    assert!(api_key_response.api_key.starts_with("vex_"));
    assert!(api_key_response.enabled);
    // Verifying API key is masked in the response
    assert!(api_key_response.api_key.contains("*"));
    assert_eq!(api_key_response.permissions, vec!["USER_DATA"]);

    // The permissions were updated in the database too.
    let updated_key = repo.find_by_id(api_key_id).await.unwrap().unwrap();
    assert_eq!(updated_key.user_id, Some(user.user_id));
    let db_permissions: Vec<String> = updated_key
        .permissions
        .into_iter()
        .map(|p| p.to_string())
        .collect();
    assert_eq!(db_permissions, vec!["USER_DATA"]);
}

#[tokio::test]
async fn test_update_api_key_invalid_permissions() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let user = create_test_user();
    let api_key_id = created_key(&repo, &user).await;
    let update_request = UpdateApiKeyRequest {
        permissions: vec!["invalid_permission".to_string()],
        ip_blacklist: None,
    };

    let error = update_api_key(
        web::Path::from(api_key_id),
        web::Json(update_request),
        repo.clone(),
        None,
        user.clone(),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("Invalid permissions"));
}

#[tokio::test]
async fn test_delete_api_key_success() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let user = create_test_user();
    let api_key_id = created_key(&repo, &user).await;

    let response = delete_api_key(
        web::Path::from(api_key_id),
        repo.clone(),
        None,
        user.clone(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), 200);
    let delete_response: serde_json::Value = json(response).await;
    assert_eq!(delete_response["message"], "API key deleted successfully");

    // Gone, secret included: it cannot be switched back on.
    assert!(repo.find_by_id(api_key_id).await.unwrap().is_none());
    let toggle = toggle_api_key(
        web::Path::from(api_key_id),
        web::Json(ToggleApiKeyRequest { enabled: true }),
        repo.clone(),
        None,
        user.clone(),
    )
    .await
    .unwrap();
    assert_eq!(toggle.status(), 404);
}

#[tokio::test]
async fn test_toggle_api_key_disable_then_enable() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let user = create_test_user();
    let api_key_id = created_key(&repo, &user).await;

    for (enabled, message) in [
        (false, "API key disabled successfully"),
        (true, "API key enabled successfully"),
    ] {
        let response = toggle_api_key(
            web::Path::from(api_key_id),
            web::Json(ToggleApiKeyRequest { enabled }),
            repo.clone(),
            None,
            user.clone(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), 200);
        let toggle_response: serde_json::Value = json(response).await;
        assert_eq!(toggle_response["message"], message);
        let key = repo.find_by_id(api_key_id).await.unwrap().unwrap();
        assert_eq!(key.enabled, enabled);
    }
}

/// A key the API-key middleware let through is not a signed-in person: it must not create
/// a key only its thief knows, or change its own block list.
#[actix_web::test]
async fn signed_api_key_cannot_manage_api_keys() {
    use actix_web::{HttpMessage, dev::Service};
    use lib_middleware::api_key::{ApiKeyConfig, SecurityType};

    let Some(db) = require_database().await else {
        return;
    };
    let user_id = Uuid::new_v4();
    let repo = repository(&db);
    let app = actix_web::test::init_service(
        actix_web::App::new()
            // What the API-key middleware leaves for the handlers: the key, not a user.
            .wrap_fn(move |request, service| {
                request.extensions_mut().insert(Arc::new(ApiKeyConfig {
                    user_id: Some(user_id),
                    ..ApiKeyConfig::new("stolen-key".to_string())
                        .with_permissions(vec![SecurityType::Trade])
                }));
                service.call(request)
            })
            .service(
                web::scope("/api-key")
                    .app_data(repo.clone())
                    .configure(lib_api::routes::api_key_routes::configure),
            ),
    )
    .await;

    let request = actix_web::test::TestRequest::post()
        .uri("/api-key")
        .set_json(serde_json::json!({"permissions": ["trade"]}))
        .to_request();
    let response = actix_web::test::call_service(&app, request).await;

    assert_eq!(response.status(), 401);
    assert!(repo.find_by_user_id(user_id).await.unwrap().is_empty());
}

/// Every route that changes a key checks its owner; dropping one check must fail here.
#[tokio::test]
async fn another_users_key_is_forbidden_on_update_delete_and_toggle() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let (owner, other) = (create_test_user(), create_test_user());
    let id = created_key(&repo, &owner).await;
    let key = repo.find_by_id(id).await.unwrap().unwrap();

    let update = update_api_key(
        web::Path::from(id),
        web::Json(UpdateApiKeyRequest {
            permissions: vec!["user_data".to_string()],
            ip_blacklist: Some(vec![]),
        }),
        repo.clone(),
        None,
        other.clone(),
    )
    .await
    .unwrap();
    let delete = delete_api_key(web::Path::from(id), repo.clone(), None, other.clone())
        .await
        .unwrap();
    let toggle = toggle_api_key(
        web::Path::from(id),
        web::Json(ToggleApiKeyRequest { enabled: false }),
        repo.clone(),
        None,
        other.clone(),
    )
    .await
    .unwrap();

    for response in [update, delete, toggle] {
        assert_eq!(response.status(), 403);
    }
    let unchanged = repo.find_by_id(id).await.unwrap().unwrap();
    assert_eq!(unchanged, key);
}

/// The HMAC secret is always the server's 32 random bytes: a client-chosen one ("a") would
/// let anyone holding one signed request brute-force it.
#[tokio::test]
async fn create_api_key_never_stores_a_client_chosen_secret() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = repository(&db);
    let user = create_test_user();
    let request: CreateApiKeyRequest = serde_json::from_value(serde_json::json!({
        "permissions": ["trade"],
        "key_material_type": "hmac_secret",
        "key_material_value": "a"
    }))
    .unwrap();

    let response = create_api_key(web::Json(request), repo.clone(), user.clone())
        .await
        .unwrap();

    assert_eq!(response.status(), 201);
    let key = repo.find_by_user_id(user.user_id).await.unwrap().remove(0);
    let secret = key.key_material_value.unwrap();
    assert_ne!(secret, "a");
    assert_eq!(secret.len(), 44, "base64 of 32 bytes");
}

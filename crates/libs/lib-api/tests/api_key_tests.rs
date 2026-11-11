//! Unit tests for API key handlers
//!
//! These tests use the actual Postgres implementation of ApiKeyRepository
//! to test the real CRUD operations with the database, on the read model's schema.

mod common;

use actix_web::web;
use chrono::Utc;
use common::require_database;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use std::sync::Arc;
use uuid::Uuid;

use lib_api::handlers::api_key::{
    ApiKeyListResponse, ApiKeyResponse, CreateApiKeyRequest, UpdateApiKeyRequest, create_api_key,
    delete_api_key, get_api_key, list_api_keys, toggle_api_key, update_api_key,
};
use lib_auth::middleware::jwt_auth::AuthenticatedUser;
use lib_middleware::repository::api_key_repository::ApiKeyRepository;

async fn cleanup_test_data(db: &DatabaseConnection, user_id: Uuid) {
    let cleanup_sql = format!("DELETE FROM api_keys WHERE user_id = '{}'", user_id);
    db.execute_unprepared(&cleanup_sql)
        .await
        .expect("Failed to cleanup test data");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_api_key_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        // Clean up before test to ensure isolation
        cleanup_test_data(&db, user_id).await;

        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request = create_test_request();

        let result = create_api_key(
            web::Json(request.clone()),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 201);

                let user_keys = repo.find_by_user_id(user_id).await.unwrap();
                assert_eq!(user_keys.len(), 1);

                let api_key_model = &user_keys[0];
                assert_eq!(api_key_model.user_id, Some(user_id));
                assert_eq!(api_key_model.description, request.description);
                assert!(api_key_model.api_key.starts_with("vex_"));
                assert!(api_key_model.enabled);
            }
            Err(e) => {
                panic!("Handler returned error: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_create_api_key_invalid_permissions() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;
        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );

        let mut request = create_test_request();
        request.permissions = vec!["invalid_permission".to_string()];

        let result = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user,
        )
        .await;

        match result {
            Ok(_) => {
                panic!("Expected error for invalid permissions, but got success");
            }
            Err(e) => {
                assert!(e.to_string().contains("Invalid permissions"));
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_list_api_keys_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;

        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );

        let request1 = create_test_request();
        let request2 = CreateApiKeyRequest {
            description: Some("Second API Key".to_string()),
            permissions: vec!["user_data".to_string()],
            recv_window: Some(10000),
            expires_at: None,
            ip_blacklist: None,
        };

        // Create first API key
        let _result1 = create_api_key(
            web::Json(request1),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        // Create second API key
        let _result2 = create_api_key(
            web::Json(request2),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        let result = list_api_keys(
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                // Parsing the response body to validate the output
                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let list_response: ApiKeyListResponse = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(list_response.total, 2);
                assert_eq!(list_response.api_keys.len(), 2);

                for api_key_response in &list_response.api_keys {
                    assert_eq!(api_key_response.user_id, Some(user_id));
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
            Err(e) => {
                panic!("List API keys failed: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_get_api_key_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;

        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request = create_test_request();

        let create_result = create_api_key(
            web::Json(request.clone()),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        let api_key_id = match create_result {
            Ok(_) => {
                let user_keys = repo.find_by_user_id(user_id).await.unwrap();
                assert_eq!(user_keys.len(), 1);
                user_keys[0].id
            }
            Err(e) => {
                panic!("Failed to create API key: {}", e);
            }
        };

        let result = get_api_key(
            web::Path::from(api_key_id),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                // Parsing the response body to validate the output
                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let api_key_response: ApiKeyResponse = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(api_key_response.id, api_key_id);
                assert_eq!(api_key_response.user_id, Some(user_id));
                assert_eq!(api_key_response.description, request.description);
                assert!(api_key_response.api_key.starts_with("vex_"));
                assert!(api_key_response.enabled);

                // Verifying API key is masked in the response
                assert!(api_key_response.api_key.contains("*"));
            }
            Err(e) => {
                panic!("Get API key failed: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_get_api_key_not_found() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;
        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let non_existent_id = Uuid::new_v4();

        let result = get_api_key(
            web::Path::from(non_existent_id),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user,
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 404);

                let api_key = repo.find_by_id(non_existent_id).await.unwrap();
                assert!(api_key.is_none());
            }
            Err(e) => {
                panic!("Get API key failed: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_get_api_key_access_denied() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user1 = create_test_user();
        let user2 = create_test_user();
        let user1_id = user1.user_id;
        let user2_id = user2.user_id;
        cleanup_test_data(&db, user1_id).await;
        cleanup_test_data(&db, user2_id).await;
        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request = create_test_request();

        let create_result = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user1.clone(),
        )
        .await;

        let api_key_id = match create_result {
            Ok(_) => {
                let user_keys = repo.find_by_user_id(user1_id).await.unwrap();
                assert_eq!(user_keys.len(), 1);
                user_keys[0].id
            }
            Err(e) => {
                panic!("Failed to create API key: {}", e);
            }
        };

        // Try to get API key with user2: should be denied
        let result = get_api_key(
            web::Path::from(api_key_id),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user2.clone(),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 403);

                // Verifying the API key belongs to user1, not user2
                let api_key = repo.find_by_id(api_key_id).await.unwrap().unwrap();
                assert_eq!(api_key.user_id, Some(user1_id));
                assert_ne!(api_key.user_id, Some(user2_id));
            }
            Err(e) => {
                panic!("Get API key failed: {}", e);
            }
        }

        cleanup_test_data(&db, user1_id).await;
        cleanup_test_data(&db, user2_id).await;
    }

    #[tokio::test]
    async fn test_update_api_key_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;

        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request = create_test_request();

        let create_result = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        let api_key_id = match create_result {
            Ok(_) => {
                let user_keys = repo.find_by_user_id(user_id).await.unwrap();
                assert_eq!(user_keys.len(), 1);
                user_keys[0].id
            }
            Err(e) => {
                panic!("Failed to create API key: {}", e);
            }
        };

        let update_request = UpdateApiKeyRequest {
            permissions: vec!["user_data".to_string()],
            ip_blacklist: None,
        };

        let result = update_api_key(
            web::Path::from(api_key_id),
            web::Json(update_request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            None,
            user.clone(),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                // Parsing the response body to validate the output
                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let api_key_response: ApiKeyResponse = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(api_key_response.id, api_key_id);
                assert_eq!(api_key_response.user_id, Some(user_id));
                assert!(api_key_response.api_key.starts_with("vex_"));
                assert!(api_key_response.enabled);

                // Verifying API key is masked in the response
                assert!(api_key_response.api_key.contains("*"));

                // Verifying the permissions were updated
                assert_eq!(api_key_response.permissions, vec!["USER_DATA"]);

                // Verifying the update in the database by fetching the API key
                let updated_key = repo.find_by_id(api_key_id).await.unwrap().unwrap();
                assert_eq!(updated_key.id, api_key_id);
                assert_eq!(updated_key.user_id, Some(user_id));

                // Verifying the permissions were actually updated in the database
                // The permissions are stored as an array of SecurityType enums
                let db_permissions: Vec<String> = updated_key
                    .permissions
                    .into_iter()
                    .map(|p| p.to_string())
                    .collect();
                assert_eq!(db_permissions, vec!["USER_DATA"]);
            }
            Err(e) => {
                panic!("Update API key failed: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_update_api_key_invalid_permissions() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;
        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request = create_test_request();

        let create_result = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        let api_key_id = match create_result {
            Ok(_) => {
                let user_keys = repo.find_by_user_id(user_id).await.unwrap();
                assert_eq!(user_keys.len(), 1);
                user_keys[0].id
            }
            Err(e) => {
                panic!("Failed to create API key: {}", e);
            }
        };

        let update_request = UpdateApiKeyRequest {
            permissions: vec!["invalid_permission".to_string()],
            ip_blacklist: None,
        };

        let result = update_api_key(
            web::Path::from(api_key_id),
            web::Json(update_request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            None,
            user.clone(),
        )
        .await;

        match result {
            Ok(_) => {
                panic!("Expected error for invalid permissions, but got success");
            }
            Err(e) => {
                assert!(e.to_string().contains("Invalid permissions"));
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_delete_api_key_success() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;

        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request = create_test_request();

        let create_result = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        let api_key_id = match create_result {
            Ok(_) => {
                let user_keys = repo.find_by_user_id(user_id).await.unwrap();
                assert_eq!(user_keys.len(), 1);
                user_keys[0].id
            }
            Err(e) => {
                panic!("Failed to create API key: {}", e);
            }
        };

        let result = delete_api_key(
            web::Path::from(api_key_id),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            None,
            user.clone(),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                // Parsing the response body to validate the output
                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let delete_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(delete_response["message"], "API key deleted successfully");

                // Gone, secret included: it cannot be switched back on.
                assert!(repo.find_by_id(api_key_id).await.unwrap().is_none());
                let toggle = toggle_api_key(
                    web::Path::from(api_key_id),
                    web::Json(serde_json::json!({"enabled": true})),
                    web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
                    None,
                    user.clone(),
                )
                .await
                .unwrap();
                assert_eq!(toggle.status(), 404);
            }
            Err(e) => {
                panic!("Delete API key failed: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_toggle_api_key_enable() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;

        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request = create_test_request();

        let create_result = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        let api_key_id = match create_result {
            Ok(_) => {
                let user_keys = repo.find_by_user_id(user_id).await.unwrap();
                assert_eq!(user_keys.len(), 1);
                user_keys[0].id
            }
            Err(e) => {
                panic!("Failed to create API key: {}", e);
            }
        };

        // Disable API key first
        let disable_request = serde_json::json!({"enabled": false});
        let _disable_result = toggle_api_key(
            web::Path::from(api_key_id),
            web::Json(disable_request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            None,
            user.clone(),
        )
        .await;

        // Enable API key
        let enable_request = serde_json::json!({"enabled": true});
        let result = toggle_api_key(
            web::Path::from(api_key_id),
            web::Json(enable_request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            None,
            user.clone(),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                // Parsing the response body to validate the output
                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let toggle_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(toggle_response["message"], "API key enabled successfully");

                let enabled_key = repo.find_by_id(api_key_id).await.unwrap().unwrap();
                assert!(enabled_key.enabled);
            }
            Err(e) => {
                panic!("Toggle API key failed: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    #[tokio::test]
    async fn test_toggle_api_key_disable() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id;
        cleanup_test_data(&db, user_id).await;

        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request = create_test_request();

        let create_result = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user.clone(),
        )
        .await;

        let api_key_id = match create_result {
            Ok(_) => {
                let user_keys = repo.find_by_user_id(user_id).await.unwrap();
                assert_eq!(user_keys.len(), 1);
                user_keys[0].id
            }
            Err(e) => {
                panic!("Failed to create API key: {}", e);
            }
        };

        // Disable API key
        let disable_request = serde_json::json!({"enabled": false});
        let result = toggle_api_key(
            web::Path::from(api_key_id),
            web::Json(disable_request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            None,
            user.clone(),
        )
        .await;

        match result {
            Ok(response) => {
                assert_eq!(response.status(), 200);

                // Parsing the response body to validate the output
                let body = response.into_body();
                let bytes = actix_web::body::to_bytes(body).await.unwrap();
                let toggle_response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

                assert_eq!(toggle_response["message"], "API key disabled successfully");

                let disabled_key = repo.find_by_id(api_key_id).await.unwrap().unwrap();
                assert!(!disabled_key.enabled);
            }
            Err(e) => {
                panic!("Toggle API key failed: {}", e);
            }
        }

        cleanup_test_data(&db, user_id).await;
    }

    /// A key the API-key middleware let through is not a signed-in person: it must not create
    /// a key only its thief knows, or change its own block list.
    #[actix_web::test]
    async fn signed_api_key_cannot_manage_api_keys() {
        use actix_web::{HttpMessage, dev::Service};
        use lib_middleware::api_key::{ApiKeyConfig, SecurityType};

        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user_id = Uuid::new_v4();
        let repo: Arc<dyn ApiKeyRepository> = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
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
                        .app_data(web::Data::new(repo.clone()))
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
            return; // Skip test if database is unavailable
        };
        let (owner, other) = (create_test_user(), create_test_user());
        let repo: Arc<dyn ApiKeyRepository> = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let repo_data = || web::Data::new(repo.clone());
        create_api_key(web::Json(create_test_request()), repo_data(), owner.clone())
            .await
            .unwrap();
        let key = repo.find_by_user_id(owner.user_id).await.unwrap().remove(0);

        let update = update_api_key(
            web::Path::from(key.id),
            web::Json(UpdateApiKeyRequest {
                permissions: vec!["user_data".to_string()],
                ip_blacklist: Some(vec![]),
            }),
            repo_data(),
            None,
            other.clone(),
        )
        .await
        .unwrap();
        let delete = delete_api_key(web::Path::from(key.id), repo_data(), None, other.clone())
            .await
            .unwrap();
        let toggle = toggle_api_key(
            web::Path::from(key.id),
            web::Json(serde_json::json!({"enabled": false})),
            repo_data(),
            None,
            other.clone(),
        )
        .await
        .unwrap();

        for response in [update, delete, toggle] {
            assert_eq!(response.status(), 403);
        }
        let unchanged = repo.find_by_id(key.id).await.unwrap().unwrap();
        assert_eq!(unchanged, key);

        cleanup_test_data(&db, owner.user_id).await;
    }

    /// The HMAC secret is always the server's 32 random bytes: a client-chosen one ("a") would
    /// let anyone holding one signed request brute-force it.
    #[tokio::test]
    async fn create_api_key_never_stores_a_client_chosen_secret() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let repo: Arc<dyn ApiKeyRepository> = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );
        let request: CreateApiKeyRequest = serde_json::from_value(serde_json::json!({
            "permissions": ["trade"],
            "key_material_type": "hmac_secret",
            "key_material_value": "a"
        }))
        .unwrap();

        let response = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone()),
            user.clone(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), 201);
        let key = repo.find_by_user_id(user.user_id).await.unwrap().remove(0);
        let secret = key.key_material_value.unwrap();
        assert_ne!(secret, "a");
        assert_eq!(secret.len(), 44, "base64 of 32 bytes");

        cleanup_test_data(&db, user.user_id).await;
    }
}

//! Unit tests for API key handlers
//!
//! These tests use the actual Postgres implementation of ApiKeyRepository
//! to test the real CRUD operations with the database.

use actix_web::web;
use chrono::Utc;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use std::sync::Arc;
use uuid::Uuid;

use lib_api::handlers::api_key::{
    ApiKeyListResponse, ApiKeyResponse, AuthenticatedUser, CreateApiKeyRequest,
    UpdateApiKeyRequest, create_api_key, delete_api_key, get_api_key, list_api_keys,
    toggle_api_key, update_api_key,
};
use lib_middleware::repository::api_key_repository::ApiKeyRepository;

async fn setup_test_db() -> Result<DatabaseConnection, sea_orm::DbErr> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://vex:1234@localhost:5432/vex_gateway".to_string());

    // Add timeout to prevent hanging
    let connection_future = Database::connect(&database_url);
    let timeout_duration = std::time::Duration::from_secs(5);

    match tokio::time::timeout(timeout_duration, connection_future).await {
        Ok(Ok(db)) => Ok(db),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(sea_orm::DbErr::Custom(
            "Database connection timeout".to_string(),
        )),
    }
}

// Helper function to get database connection, return None if unavailable (test will be skipped)
async fn require_database() -> Option<DatabaseConnection> {
    setup_test_db_with_schema().await.ok()
}

async fn setup_test_db_with_schema() -> Result<DatabaseConnection, sea_orm::DbErr> {
    let db = setup_test_db().await?;

    let create_table_sql = r#"
        -- Create enum types for API key management (only if they don't exist)
        DO $$ BEGIN
            CREATE TYPE security_type AS ENUM ('none', 'trade', 'user_data', 'user_stream');
        EXCEPTION
            WHEN duplicate_object THEN null;
        END $$;

        DO $$ BEGIN
            CREATE TYPE key_material_type AS ENUM ('hmac_secret', 'rsa_public_key', 'ed25519_public_key');
        EXCEPTION
            WHEN duplicate_object THEN null;
        END $$;

        -- Create api_keys table if it doesn't exist (don't drop - allows parallel test execution)
        CREATE TABLE IF NOT EXISTS api_keys (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            api_key VARCHAR(255) NOT NULL UNIQUE,
            key_material_type key_material_type,
            key_material_value TEXT,
            permissions security_type[] NOT NULL DEFAULT ARRAY['none']::security_type[],
            recv_window BIGINT NOT NULL DEFAULT 5000,
            enabled BOOLEAN NOT NULL DEFAULT true,
            description TEXT,
            ip_blacklist TEXT[] DEFAULT ARRAY[]::TEXT[],
            user_id UUID,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            expires_at TIMESTAMPTZ,
            last_used_at TIMESTAMPTZ
        );

        -- Create indexes if they don't exist (idempotent)
        CREATE INDEX IF NOT EXISTS idx_api_keys_api_key ON api_keys(api_key);
        CREATE INDEX IF NOT EXISTS idx_api_keys_user_id ON api_keys(user_id);
        CREATE INDEX IF NOT EXISTS idx_api_keys_enabled ON api_keys(enabled);
        CREATE INDEX IF NOT EXISTS idx_api_keys_expires_at ON api_keys(expires_at);
        CREATE INDEX IF NOT EXISTS idx_api_keys_created_at ON api_keys(created_at);
        CREATE INDEX IF NOT EXISTS idx_api_keys_last_used_at ON api_keys(last_used_at);

        -- Partial index for enabled keys only (drop and recreate if exists)
        DROP INDEX IF EXISTS idx_api_keys_active;
        CREATE INDEX idx_api_keys_active ON api_keys(api_key) 
        WHERE enabled = true;
    "#;

    db.execute_unprepared(create_table_sql).await.map_err(|e| {
        sea_orm::DbErr::Custom(format!("Failed to create test database schema: {}", e))
    })?;

    Ok(db)
}

async fn cleanup_test_data(db: &DatabaseConnection, user_id: Uuid) {
    let cleanup_sql = format!("DELETE FROM api_keys WHERE user_id = '{}'", user_id);
    db.execute_unprepared(&cleanup_sql)
        .await
        .expect("Failed to cleanup test data");
}

fn create_test_user() -> AuthenticatedUser {
    use lib_auth::middleware::jwt_auth::AuthenticatedUser as JwtUser;
    AuthenticatedUser::Jwt(JwtUser {
        user_id: Uuid::new_v4(),
    })
}

fn create_test_request() -> CreateApiKeyRequest {
    CreateApiKeyRequest {
        description: Some("Test API Key".to_string()),
        permissions: vec!["trade".to_string(), "user_data".to_string()],
        recv_window: Some(5000),
        expires_at: Some(Utc::now() + chrono::Duration::days(30)),
        ip_blacklist: Some(vec!["192.168.1.1".to_string()]),
        key_material_type: Some("hmac_secret".to_string()),
        key_material_value: Some("test-secret-key".to_string()),
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
        let user_id = user.user_id().unwrap();
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
        let user_id = user.user_id().unwrap();
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
    async fn test_create_api_key_invalid_key_material() {
        let Some(db) = require_database().await else {
            return; // Skip test if database is unavailable
        };
        let user = create_test_user();
        let user_id = user.user_id().unwrap();
        cleanup_test_data(&db, user_id).await;
        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );

        let mut request = create_test_request();
        request.key_material_type = Some("invalid_type".to_string());

        let result = create_api_key(
            web::Json(request),
            web::Data::new(repo.clone() as Arc<dyn ApiKeyRepository>),
            user,
        )
        .await;

        match result {
            Ok(_) => {
                panic!("Expected error for invalid key material, but got success");
            }
            Err(e) => {
                assert!(e.to_string().contains("Invalid key material"));
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
        let user_id = user.user_id().unwrap();
        cleanup_test_data(&db, user_id).await;

        let repo = Arc::new(
            lib_middleware::repository::api_key_repository::PostgresApiKeyRepository::new(
                Arc::new(db.clone()),
            ),
        );

        let request1 = create_test_request();
        let request2 = CreateApiKeyRequest {
            description: Some("Second API Key".to_string()),
            permissions: vec!["user_stream".to_string()],
            recv_window: Some(10000),
            expires_at: None,
            ip_blacklist: None,
            key_material_type: None,
            key_material_value: None,
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
        let user_id = user.user_id().unwrap();
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
        let user_id = user.user_id().unwrap();
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
        let user1_id = user1.user_id().unwrap();
        let user2_id = user2.user_id().unwrap();
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
        let user_id = user.user_id().unwrap();
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
            permissions: vec!["user_stream".to_string()],
            ip_blacklist: None,
        };

        let result = update_api_key(
            web::Path::from(api_key_id),
            web::Json(update_request),
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
                assert!(api_key_response.api_key.starts_with("vex_"));
                assert!(api_key_response.enabled);

                // Verifying API key is masked in the response
                assert!(api_key_response.api_key.contains("*"));

                // Verifying the permissions were updated
                assert_eq!(api_key_response.permissions, vec!["USER_STREAM"]);

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
                assert_eq!(db_permissions, vec!["USER_STREAM"]);
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
        let user_id = user.user_id().unwrap();
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
        let user_id = user.user_id().unwrap();
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

                let deleted_key = repo.find_by_id(api_key_id).await.unwrap();
                assert!(deleted_key.is_some());
                assert!(!deleted_key.unwrap().enabled);
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
        let user_id = user.user_id().unwrap();
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
            user.clone(),
        )
        .await;

        // Enable API key
        let enable_request = serde_json::json!({"enabled": true});
        let result = toggle_api_key(
            web::Path::from(api_key_id),
            web::Json(enable_request),
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
        let user_id = user.user_id().unwrap();
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
}

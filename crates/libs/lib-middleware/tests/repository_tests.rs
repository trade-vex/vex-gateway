use chrono::{Duration, Utc};
use lib_middleware::api_key::error::ApiKeyError;
use lib_middleware::api_key::security::{KeyMaterial, SecurityType};
use lib_middleware::models::api_key::{KeyMaterialType, NewApiKey};
use lib_middleware::repository::api_key_repository::ApiKeyRepository;
use std::sync::Arc;
use uuid::Uuid;

// Mock implementation for testing
#[derive(Clone)]
struct MockApiKeyRepository {
    should_fail: bool,
    not_found: bool,
}

impl MockApiKeyRepository {
    fn new() -> Self {
        Self {
            should_fail: false,
            not_found: false,
        }
    }

    fn with_failure() -> Self {
        Self {
            should_fail: true,
            not_found: false,
        }
    }
}

#[async_trait::async_trait]
impl ApiKeyRepository for MockApiKeyRepository {
    async fn find_by_api_key(
        &self,
        api_key: &str,
    ) -> Result<Option<lib_middleware::models::api_key::Model>, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        if self.not_found || api_key == "not-found" {
            return Ok(None);
        }

        Ok(Some(lib_middleware::models::api_key::Model {
            id: Uuid::new_v4(),
            api_key: api_key.to_string(),
            key_material_type: Some(KeyMaterialType::HmacSecret),
            key_material_value: Some("mock-secret".to_string()),
            permissions: serde_json::json!(["TRADE", "USER_DATA"]),
            recv_window: 5000,
            enabled: true,
            description: Some("Mock API key".to_string()),
            ip_blacklist: vec![],
            user_id: Some(Uuid::new_v4()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
        }))
    }

    async fn find_by_id(
        &self,
        id: Uuid,
    ) -> Result<Option<lib_middleware::models::api_key::Model>, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        if self.not_found {
            return Ok(None);
        }

        Ok(Some(lib_middleware::models::api_key::Model {
            id,
            api_key: "mock-key".to_string(),
            key_material_type: Some(KeyMaterialType::HmacSecret),
            key_material_value: Some("mock-secret".to_string()),
            permissions: serde_json::json!(["USER_STREAM"]),
            recv_window: 5000,
            enabled: true,
            description: None,
            ip_blacklist: vec![],
            user_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
        }))
    }

    async fn find_by_user_id(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<lib_middleware::models::api_key::Model>, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        if self.not_found {
            return Ok(vec![]);
        }

        Ok(vec![
            lib_middleware::models::api_key::Model {
                id: Uuid::new_v4(),
                api_key: "user-key-1".to_string(),
                key_material_type: Some(KeyMaterialType::RsaPublicKey),
                key_material_value: Some("mock-rsa-key".to_string()),
                permissions: serde_json::json!(["TRADE"]),
                recv_window: 5000,
                enabled: true,
                description: Some("User's trading key".to_string()),
                ip_blacklist: vec!["192.168.1.1".to_string()],
                user_id: Some(user_id),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                expires_at: None,
                last_used_at: Some(Utc::now() - Duration::hours(2)),
            },
            lib_middleware::models::api_key::Model {
                id: Uuid::new_v4(),
                api_key: "user-key-2".to_string(),
                key_material_type: None,
                key_material_value: None,
                permissions: serde_json::json!(["USER_STREAM"]),
                recv_window: 5000,
                enabled: false,
                description: Some("User's stream key (disabled)".to_string()),
                ip_blacklist: vec![],
                user_id: Some(user_id),
                created_at: Utc::now() - Duration::days(10),
                updated_at: Utc::now() - Duration::days(1),
                expires_at: Some(Utc::now() + Duration::days(30)),
                last_used_at: None,
            },
        ])
    }

    async fn create(
        &self,
        new_api_key: NewApiKey,
    ) -> Result<lib_middleware::models::api_key::Model, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        let (key_material_type, key_material_value) = match &new_api_key.key_material {
            Some(KeyMaterial::HmacSecret(secret)) => {
                (Some(KeyMaterialType::HmacSecret), Some(secret.clone()))
            }
            Some(KeyMaterial::RsaPublicKey(key)) => {
                (Some(KeyMaterialType::RsaPublicKey), Some(key.clone()))
            }
            Some(KeyMaterial::Ed25519PublicKey(key)) => {
                (Some(KeyMaterialType::Ed25519PublicKey), Some(key.clone()))
            }
            None => (None, None),
        };

        let now = Utc::now();
        Ok(lib_middleware::models::api_key::Model {
            id: Uuid::new_v4(),
            api_key: new_api_key.api_key,
            key_material_type,
            key_material_value,
            permissions: serde_json::json!(new_api_key.permissions),
            recv_window: new_api_key.recv_window as i64,
            enabled: true, // Always enable newly created keys in mock
            description: new_api_key.description,
            ip_blacklist: new_api_key.ip_blacklist,
            user_id: new_api_key.user_id,
            created_at: now,
            updated_at: now,
            expires_at: new_api_key.expires_at,
            last_used_at: None,
        })
    }

    async fn update(
        &self,
        id: Uuid,
        _updates: lib_middleware::models::api_key::ActiveModel,
    ) -> Result<lib_middleware::models::api_key::Model, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        if self.not_found {
            return Err(ApiKeyError::NotFound);
        }

        Ok(lib_middleware::models::api_key::Model {
            id,
            api_key: "updated-key".to_string(),
            key_material_type: Some(KeyMaterialType::Ed25519PublicKey),
            key_material_value: Some("updated-key-material".to_string()),
            permissions: serde_json::json!(["USER_DATA"]),
            recv_window: 10000,
            enabled: true,
            description: Some("Updated description".to_string()),
            ip_blacklist: vec!["127.0.0.1".to_string()],
            user_id: Some(Uuid::new_v4()),
            created_at: Utc::now() - Duration::hours(1),
            updated_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::days(60)),
            last_used_at: None,
        })
    }

    async fn set_enabled(&self, _id: Uuid, _enabled: bool) -> Result<(), ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        if self.not_found {
            return Err(ApiKeyError::NotFound);
        }

        Ok(())
    }

    async fn update_last_used(
        &self,
        _id: Uuid,
        _timestamp: chrono::DateTime<Utc>,
    ) -> Result<(), ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        if self.not_found {
            return Err(ApiKeyError::NotFound);
        }

        Ok(())
    }

    async fn delete(&self, _id: Uuid) -> Result<(), ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        if self.not_found {
            return Err(ApiKeyError::NotFound);
        }

        Ok(())
    }

    async fn find_active_keys(
        &self,
        limit: Option<u64>,
    ) -> Result<Vec<lib_middleware::models::api_key::Model>, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        let active_keys = vec![
            lib_middleware::models::api_key::Model {
                id: Uuid::new_v4(),
                api_key: "active-key-1".to_string(),
                key_material_type: Some(KeyMaterialType::HmacSecret),
                key_material_value: Some("secret-1".to_string()),
                permissions: serde_json::json!(["TRADE"]),
                recv_window: 5000,
                enabled: true,
                description: Some("Active trading key".to_string()),
                ip_blacklist: vec![],
                user_id: Some(Uuid::new_v4()),
                created_at: Utc::now() - Duration::days(5),
                updated_at: Utc::now() - Duration::hours(1),
                expires_at: Some(Utc::now() + Duration::days(30)),
                last_used_at: Some(Utc::now() - Duration::minutes(10)),
            },
            lib_middleware::models::api_key::Model {
                id: Uuid::new_v4(),
                api_key: "active-key-2".to_string(),
                key_material_type: None,
                key_material_value: None,
                permissions: serde_json::json!(["USER_STREAM"]),
                recv_window: 5000,
                enabled: true,
                description: None,
                ip_blacklist: vec![],
                user_id: Some(Uuid::new_v4()),
                created_at: Utc::now() - Duration::days(1),
                updated_at: Utc::now() - Duration::hours(2),
                expires_at: None, // No expiry
                last_used_at: Some(Utc::now() - Duration::hours(1)),
            },
        ];

        if let Some(limit) = limit {
            Ok(active_keys.into_iter().take(limit as usize).collect())
        } else {
            Ok(active_keys)
        }
    }

    async fn find_expiring_keys(
        &self,
        _days: i64,
    ) -> Result<Vec<lib_middleware::models::api_key::Model>, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        Ok(vec![lib_middleware::models::api_key::Model {
            id: Uuid::new_v4(),
            api_key: "expiring-key".to_string(),
            key_material_type: Some(KeyMaterialType::RsaPublicKey),
            key_material_value: Some("expiring-rsa-key".to_string()),
            permissions: serde_json::json!(["TRADE", "USER_DATA"]),
            recv_window: 5000,
            enabled: true,
            description: Some("About to expire".to_string()),
            ip_blacklist: vec![],
            user_id: Some(Uuid::new_v4()),
            created_at: Utc::now() - Duration::days(60),
            updated_at: Utc::now() - Duration::days(1),
            expires_at: Some(Utc::now() + Duration::days(2)), // Expires in 2 days
            last_used_at: Some(Utc::now() - Duration::hours(6)),
        }])
    }

    async fn count_by_user(&self, _user_id: Uuid) -> Result<u64, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        Ok(3) // Mock count
    }

    async fn find_by_permission(
        &self,
        permission: SecurityType,
    ) -> Result<Vec<lib_middleware::models::api_key::Model>, ApiKeyError> {
        if self.should_fail {
            return Err(ApiKeyError::Database("Mock database error".to_string()));
        }

        let permission_str = permission.to_string();

        Ok(vec![lib_middleware::models::api_key::Model {
            id: Uuid::new_v4(),
            api_key: format!("key-with-{}", permission_str.to_lowercase()),
            key_material_type: Some(KeyMaterialType::HmacSecret),
            key_material_value: Some("permission-test-secret".to_string()),
            permissions: serde_json::json!([permission_str]),
            recv_window: 5000,
            enabled: true,
            description: Some(format!("Key with {} permission", permission_str)),
            ip_blacklist: vec![],
            user_id: Some(Uuid::new_v4()),
            created_at: Utc::now() - Duration::days(1),
            updated_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
        }])
    }
}

#[tokio::test]
async fn test_repository_find_by_api_key_success() {
    let repo = MockApiKeyRepository::new();
    let result = repo.find_by_api_key("test-key").await.unwrap();

    assert!(result.is_some());
    let model = result.unwrap();
    assert_eq!(model.api_key, "test-key");
    assert_eq!(model.key_material_type, Some(KeyMaterialType::HmacSecret));
    assert!(model.enabled);
}

#[tokio::test]
async fn test_repository_find_by_api_key_not_found() {
    let repo = MockApiKeyRepository::new();
    let result = repo.find_by_api_key("not-found").await.unwrap();

    assert!(result.is_none());
}

#[tokio::test]
async fn test_repository_find_by_api_key_database_error() {
    let repo = MockApiKeyRepository::with_failure();
    let result = repo.find_by_api_key("any-key").await;

    assert!(result.is_err());
    match result.unwrap_err() {
        ApiKeyError::Database(msg) => assert_eq!(msg, "Mock database error"),
        _ => panic!("Expected Database error"),
    }
}

#[tokio::test]
async fn test_repository_create() {
    let repo = MockApiKeyRepository::new();

    let new_api_key = NewApiKey {
        api_key: "new-test-key".to_string(),
        key_material: Some(KeyMaterial::Ed25519PublicKey(
            "ed25519-key-data".to_string(),
        )),
        permissions: vec![SecurityType::Trade, SecurityType::UserData],
        recv_window: 10000,
        description: Some("Test key creation".to_string()),
        ip_blacklist: vec!["192.168.1.1".to_string()],
        user_id: Some(Uuid::new_v4()),
        expires_at: Some(Utc::now() + Duration::days(90)),
    };

    let result = repo.create(new_api_key.clone()).await.unwrap();

    assert_eq!(result.api_key, new_api_key.api_key);
    assert_eq!(
        result.key_material_type,
        Some(KeyMaterialType::Ed25519PublicKey)
    );
    assert_eq!(result.recv_window, new_api_key.recv_window as i64);
    assert!(result.enabled); // Mock always creates enabled keys
    assert_eq!(result.description, new_api_key.description);
    assert_eq!(result.ip_blacklist, new_api_key.ip_blacklist);
    assert_eq!(result.user_id, new_api_key.user_id);
}

#[tokio::test]
async fn test_repository_trait_object() {
    // Test that the repository can be used as a trait object
    let repo: Arc<dyn ApiKeyRepository> = Arc::new(MockApiKeyRepository::new());

    let result = repo.find_by_api_key("trait-test").await.unwrap();
    assert!(result.is_some());

    let count = repo.count_by_user(Uuid::new_v4()).await.unwrap();
    assert_eq!(count, 3);
}

#[test]
fn test_repository_send_sync() {
    // Ensure repository traits are Send + Sync for async usage
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    assert_send::<MockApiKeyRepository>();
    assert_sync::<MockApiKeyRepository>();
    assert_send::<Box<dyn ApiKeyRepository>>();
    assert_sync::<Arc<dyn ApiKeyRepository>>();
}

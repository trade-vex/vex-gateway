use lib_middleware::models::api_key::{Model, NewApiKey, KeyMaterialType};
use lib_middleware::api_key::security::{SecurityType, KeyMaterial};
use chrono::{Utc, Duration};
use uuid::Uuid;
use serde_json::json;

#[test]
fn test_model_creation() {
    let model = Model {
        id: Uuid::new_v4(),
        api_key: "test-api-key".to_string(),
        key_material_type: Some(KeyMaterialType::HmacSecret),
        key_material_value: Some("secret-value".to_string()),
        permissions: json!(["TRADE", "USER_DATA"]),
        recv_window: 5000,
        enabled: true,
        description: Some("Test API key".to_string()),
        ip_blacklist: vec!["192.168.1.1".to_string()],
        user_id: Some(Uuid::new_v4()),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: Some(Utc::now() + Duration::days(30)),
        last_used_at: None,
    };
    
    assert_eq!(model.api_key, "test-api-key");
    assert_eq!(model.key_material_type, Some(KeyMaterialType::HmacSecret));
    assert_eq!(model.key_material_value, Some("secret-value".to_string()));
    assert_eq!(model.recv_window, 5000);
    assert!(model.enabled);
    assert_eq!(model.description, Some("Test API key".to_string()));
    assert_eq!(model.ip_blacklist, vec!["192.168.1.1".to_string()]);
    assert!(model.user_id.is_some());
    assert!(model.expires_at.is_some());
    assert!(model.last_used_at.is_none());
}

#[test]
fn test_model_clone() {
    let original = Model {
        id: Uuid::new_v4(),
        api_key: "test-api-key".to_string(),
        key_material_type: Some(KeyMaterialType::HmacSecret),
        key_material_value: Some("secret".to_string()),
        permissions: json!(["TRADE"]),
        recv_window: 5000,
        enabled: true,
        description: None,
        ip_blacklist: vec![],
        user_id: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: None,
        last_used_at: None,
    };
    
    let cloned = original.clone();
    assert_eq!(original.id, cloned.id);
    assert_eq!(original.api_key, cloned.api_key);
    assert_eq!(original.key_material_type, cloned.key_material_type);
    assert_eq!(original.permissions, cloned.permissions);
}

#[test]
fn test_model_serialization() {
    let model = Model {
        id: Uuid::new_v4(),
        api_key: "test-api-key".to_string(),
        key_material_type: Some(KeyMaterialType::RsaPublicKey),
        key_material_value: Some("pem-data".to_string()),
        permissions: json!(["USER_DATA", "USER_STREAM"]),
        recv_window: 10000,
        enabled: false,
        description: Some("Disabled key".to_string()),
        ip_blacklist: vec!["127.0.0.1".to_string(), "192.168.1.0/24".to_string()],
        user_id: Some(Uuid::new_v4()),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: Some(Utc::now() + Duration::hours(1)),
        last_used_at: Some(Utc::now() - Duration::minutes(30)),
    };
    
    let serialized = serde_json::to_string(&model).unwrap();
    let deserialized: Model = serde_json::from_str(&serialized).unwrap();
    
    assert_eq!(model.id, deserialized.id);
    assert_eq!(model.api_key, deserialized.api_key);
    assert_eq!(model.key_material_type, deserialized.key_material_type);
    assert_eq!(model.permissions, deserialized.permissions);
}

#[test]
fn test_key_material_type_variants() {
    // KeyMaterialType doesn't implement Display, so we test debug format instead
    assert_eq!(format!("{:?}", KeyMaterialType::HmacSecret), "HmacSecret");
    assert_eq!(format!("{:?}", KeyMaterialType::RsaPublicKey), "RsaPublicKey");
    assert_eq!(format!("{:?}", KeyMaterialType::Ed25519PublicKey), "Ed25519PublicKey");
}

#[test]
fn test_key_material_type_from_key_material() {
    let hmac_material = KeyMaterial::HmacSecret("secret".to_string());
    assert_eq!(KeyMaterialType::from(&hmac_material), KeyMaterialType::HmacSecret);
    
    let rsa_material = KeyMaterial::RsaPublicKey("pem".to_string());
    assert_eq!(KeyMaterialType::from(&rsa_material), KeyMaterialType::RsaPublicKey);
    
    let ed25519_material = KeyMaterial::Ed25519PublicKey("pem".to_string());
    assert_eq!(KeyMaterialType::from(&ed25519_material), KeyMaterialType::Ed25519PublicKey);
}

#[test]
fn test_key_material_type_serialization() {
    let hmac_type = KeyMaterialType::HmacSecret;
    let serialized = serde_json::to_string(&hmac_type).unwrap();
    assert_eq!(serialized, "\"HmacSecret\"");
    
    let deserialized: KeyMaterialType = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized, KeyMaterialType::HmacSecret);
    
    // Test actual sea_orm string values separately
    let rsa_type = KeyMaterialType::RsaPublicKey;
    let ed25519_type = KeyMaterialType::Ed25519PublicKey;
    
    // Test that the enum variants exist and can be serialized
    let _rsa_serialized = serde_json::to_string(&rsa_type).unwrap();
    let _ed25519_serialized = serde_json::to_string(&ed25519_type).unwrap();
}

#[test]
fn test_new_api_key_creation() {
    let user_id = Uuid::new_v4();
    let new_api_key = NewApiKey {
        api_key: "new-test-key".to_string(),
        key_material: Some(KeyMaterial::Ed25519PublicKey("ed25519-pem-data".to_string())),
        permissions: vec![SecurityType::Trade, SecurityType::UserData],
        recv_window: 15000,
        description: Some("Trading bot key".to_string()),
        ip_blacklist: vec!["10.0.0.0/8".to_string()],
        user_id: Some(user_id),
        expires_at: Some(Utc::now() + Duration::days(90)),
    };
    
    assert_eq!(new_api_key.api_key, "new-test-key");
    assert!(matches!(new_api_key.key_material, Some(KeyMaterial::Ed25519PublicKey(_))));
    assert_eq!(new_api_key.permissions, vec![SecurityType::Trade, SecurityType::UserData]);
    assert_eq!(new_api_key.recv_window, 15000);
    assert_eq!(new_api_key.description, Some("Trading bot key".to_string()));
    assert_eq!(new_api_key.ip_blacklist, vec!["10.0.0.0/8".to_string()]);
    assert_eq!(new_api_key.user_id, Some(user_id));
    assert!(new_api_key.expires_at.is_some());
}

#[test]
fn test_new_api_key_defaults() {
    let new_api_key = NewApiKey {
        api_key: "minimal-key".to_string(),
        key_material: None,
        permissions: vec![SecurityType::UserStream],
        recv_window: 5000,
        description: None,
        ip_blacklist: vec![],
        user_id: None,
        expires_at: None,
    };
    
    assert_eq!(new_api_key.api_key, "minimal-key");
    assert!(new_api_key.key_material.is_none());
    assert_eq!(new_api_key.permissions, vec![SecurityType::UserStream]);
    assert!(new_api_key.description.is_none());
    assert!(new_api_key.ip_blacklist.is_empty());
    assert!(new_api_key.user_id.is_none());
    assert!(new_api_key.expires_at.is_none());
}

#[test]
fn test_new_api_key_clone() {
    let original = NewApiKey {
        api_key: "clone-test".to_string(),
        key_material: Some(KeyMaterial::HmacSecret("secret".to_string())),
        permissions: vec![SecurityType::None],
        recv_window: 5000,
        description: Some("Clone test".to_string()),
        ip_blacklist: vec!["127.0.0.1".to_string()],
        user_id: Some(Uuid::new_v4()),
        expires_at: Some(Utc::now()),
    };
    
    let cloned = original.clone();
    assert_eq!(original.api_key, cloned.api_key);
    assert!(matches!(original.key_material, Some(KeyMaterial::HmacSecret(_))));
    assert_eq!(original.permissions, cloned.permissions);
    assert_eq!(original.user_id, cloned.user_id);
}

#[test]
fn test_new_api_key_debug() {
    let new_api_key = NewApiKey {
        api_key: "debug-test".to_string(),
        key_material: None,
        permissions: vec![],
        recv_window: 5000,
        description: None,
        ip_blacklist: vec![],
        user_id: None,
        expires_at: None,
    };
    
    let debug_str = format!("{:?}", new_api_key);
    assert!(debug_str.contains("debug-test"));
    assert!(debug_str.contains("recv_window: 5000"));
}

#[test]
fn test_model_with_empty_permissions() {
    let model = Model {
        id: Uuid::new_v4(),
        api_key: "empty-perms".to_string(),
        key_material_type: None,
        key_material_value: None,
        permissions: json!([]),
        recv_window: 5000,
        enabled: true,
        description: None,
        ip_blacklist: vec![],
        user_id: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: None,
        last_used_at: None,
    };
    
    assert_eq!(model.permissions, json!([]));
}

#[test]
fn test_model_with_complex_permissions() {
    let complex_permissions = json!([
        "TRADE",
        "USER_DATA",
        "USER_STREAM",
        {
            "custom_permission": {
                "scope": "limited",
                "resources": ["orders", "positions"]
            }
        }
    ]);
    
    let model = Model {
        id: Uuid::new_v4(),
        api_key: "complex-perms".to_string(),
        key_material_type: Some(KeyMaterialType::RsaPublicKey),
        key_material_value: Some("rsa-key".to_string()),
        permissions: complex_permissions.clone(),
        recv_window: 30000,
        enabled: true,
        description: Some("Complex permissions test".to_string()),
        ip_blacklist: vec![],
        user_id: Some(Uuid::new_v4()),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: None,
        last_used_at: None,
    };
    
    assert_eq!(model.permissions, complex_permissions);
}

#[test]
fn test_model_with_multiple_ip_blacklist() {
    let ip_list = vec![
        "192.168.1.0/24".to_string(),
        "10.0.0.0/8".to_string(),
        "172.16.0.0/12".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(), // IPv6 localhost
        "2001:db8::/32".to_string(), // IPv6 range
    ];
    
    let model = Model {
        id: Uuid::new_v4(),
        api_key: "multi-ip".to_string(),
        key_material_type: None,
        key_material_value: None,
        permissions: json!(["USER_STREAM"]),
        recv_window: 5000,
        enabled: true,
        description: Some("Multi-IP blacklist".to_string()),
        ip_blacklist: ip_list.clone(),
        user_id: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: None,
        last_used_at: None,
    };
    
    assert_eq!(model.ip_blacklist, ip_list);
    assert_eq!(model.ip_blacklist.len(), 6);
}

#[test]
fn test_model_edge_cases() {
    // Test with maximum recv_window
    let max_window_model = Model {
        id: Uuid::new_v4(),
        api_key: "max-window".to_string(),
        key_material_type: None,
        key_material_value: None,
        permissions: json!([]),
        recv_window: 60000, // Maximum allowed
        enabled: true,
        description: None,
        ip_blacklist: vec![],
        user_id: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: None,
        last_used_at: None,
    };
    
    assert_eq!(max_window_model.recv_window, 60000);
    
    // Test with very long description
    let long_description = "A".repeat(1000);
    let long_desc_model = Model {
        id: Uuid::new_v4(),
        api_key: "long-desc".to_string(),
        key_material_type: None,
        key_material_value: None,
        permissions: json!([]),
        recv_window: 5000,
        enabled: true,
        description: Some(long_description.clone()),
        ip_blacklist: vec![],
        user_id: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: None,
        last_used_at: None,
    };
    
    assert_eq!(long_desc_model.description, Some(long_description));
}

#[test]
fn test_uuid_consistency() {
    let id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    
    let model = Model {
        id,
        api_key: "uuid-test".to_string(),
        key_material_type: None,
        key_material_value: None,
        permissions: json!([]),
        recv_window: 5000,
        enabled: true,
        description: None,
        ip_blacklist: vec![],
        user_id: Some(user_id),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: None,
        last_used_at: None,
    };
    
    assert_eq!(model.id, id);
    assert_eq!(model.user_id, Some(user_id));
    
    // Ensure UUIDs are valid
    assert!(model.id.to_string().len() == 36);
    assert!(model.user_id.unwrap().to_string().len() == 36);
}

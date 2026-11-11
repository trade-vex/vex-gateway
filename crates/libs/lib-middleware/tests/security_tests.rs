use chrono::{Duration, Utc};
use lib_middleware::api_key::security::{ApiKeyConfig, KeyMaterial, SecurityType};
use lib_middleware::api_key::signature::SignatureAlgorithm;

#[test]
fn test_security_type_display() {
    assert_eq!(SecurityType::None.to_string(), "NONE");
    assert_eq!(SecurityType::Trade.to_string(), "TRADE");
    assert_eq!(SecurityType::UserData.to_string(), "USER_DATA");
    assert_eq!(SecurityType::UserStream.to_string(), "USER_STREAM");
}

#[test]
fn test_security_type_default() {
    assert_eq!(SecurityType::default(), SecurityType::None);
}

#[test]
fn test_security_type_equality() {
    assert_eq!(SecurityType::None, SecurityType::None);
    assert_eq!(SecurityType::Trade, SecurityType::Trade);
    assert_ne!(SecurityType::None, SecurityType::Trade);
}

#[test]
fn test_key_material_algorithm() {
    let hmac_key = KeyMaterial::HmacSecret("secret".to_string());
    assert_eq!(hmac_key.algorithm(), SignatureAlgorithm::HmacSha256);
}

#[test]
fn test_key_material_clone() {
    let original = KeyMaterial::HmacSecret("secret".to_string());
    let cloned = original.clone();

    let (KeyMaterial::HmacSecret(orig), KeyMaterial::HmacSecret(clone)) = (original, cloned);
    assert_eq!(orig, clone);
}

#[test]
fn test_api_key_config_new() {
    let config = ApiKeyConfig::new("test-api-key".to_string());

    assert_eq!(config.api_key, "test-api-key");
    assert!(config.key_material.is_none());
    assert_eq!(config.permissions.len(), 1);
    assert!(config.permissions.contains(&SecurityType::None));
    assert_eq!(config.recv_window, 5000);
    assert!(config.enabled);
    assert!(config.description.is_none());
    assert!(config.created_at.is_some());
    assert!(config.expires_at.is_none());
    assert!(config.ip_blacklist.is_empty());
}

#[test]
fn test_api_key_config_with_hmac_secret() {
    let config =
        ApiKeyConfig::new("test-key".to_string()).with_hmac_secret("my-secret".to_string());

    match config.key_material {
        Some(KeyMaterial::HmacSecret(secret)) => assert_eq!(secret, "my-secret"),
        _ => panic!("Expected HMAC secret"),
    }
}

#[test]
fn test_api_key_config_with_permissions() {
    let permissions = vec![SecurityType::Trade, SecurityType::UserData];
    let config = ApiKeyConfig::new("test-key".to_string()).with_permissions(permissions.clone());

    assert_eq!(config.permissions, permissions);
    assert!(!config.permissions.contains(&SecurityType::UserStream));
}

#[test]
fn test_api_key_config_with_recv_window() {
    let config = ApiKeyConfig::new("test-key".to_string()).with_recv_window(10000);

    assert_eq!(config.recv_window, 10000);

    // Test max limit enforcement
    let config_max = ApiKeyConfig::new("test-key".to_string()).with_recv_window(70000);

    assert_eq!(config_max.recv_window, 60000); // Should be capped at 60000
}

#[test]
fn test_api_key_config_with_enabled() {
    let config = ApiKeyConfig::new("test-key".to_string()).with_enabled(false);

    assert!(!config.enabled);
}

#[test]
fn test_api_key_config_with_description() {
    let description = "Trading API key".to_string();
    let config = ApiKeyConfig::new("test-key".to_string()).with_description(description.clone());

    assert_eq!(config.description, Some(description));
}

#[test]
fn test_api_key_config_with_expiration() {
    let expiry = Utc::now() + Duration::days(30);
    let config = ApiKeyConfig::new("test-key".to_string()).with_expiration(expiry);

    assert_eq!(config.expires_at, Some(expiry));
}

#[test]
fn test_api_key_config_with_ip_blacklist() {
    let ip_list = vec!["192.168.1.1".to_string(), "10.0.0.1".to_string()];
    let config = ApiKeyConfig::new("test-key".to_string()).with_ip_blacklist(ip_list.clone());

    assert_eq!(config.ip_blacklist, ip_list);
}

#[test]
fn test_api_key_config_has_permission() {
    let config = ApiKeyConfig::new("test-key".to_string())
        .with_permissions(vec![SecurityType::Trade, SecurityType::UserData]);

    // SecurityType::None should always return true
    assert!(config.has_permission(SecurityType::None));

    // Explicitly granted permissions
    assert!(config.has_permission(SecurityType::Trade));
    assert!(config.has_permission(SecurityType::UserData));

    // Not granted permission
    assert!(!config.has_permission(SecurityType::UserStream));
}

#[test]
fn test_api_key_config_is_valid() {
    let mut config = ApiKeyConfig::new("test-key".to_string());

    // Should be valid by default
    assert!(config.is_valid());

    // Disabled key should be invalid
    config.enabled = false;
    assert!(!config.is_valid());

    // Re-enable
    config.enabled = true;
    assert!(config.is_valid());

    // Expired key should be invalid
    config.expires_at = Some(Utc::now() - Duration::hours(1));
    assert!(!config.is_valid());

    // Future expiry should be valid
    config.expires_at = Some(Utc::now() + Duration::hours(1));
    assert!(config.is_valid());
}

#[test]
fn test_api_key_config_is_ip_allowed() {
    let mut config = ApiKeyConfig::new("test-key".to_string());

    // Empty blacklist should allow all IPs
    assert!(config.is_ip_allowed("192.168.1.1"));
    assert!(config.is_ip_allowed("10.0.0.1"));
    assert!(config.is_ip_allowed("127.0.0.1"));

    // With blacklist, blacklisted IPs should be denied
    config.ip_blacklist = vec!["192.168.1.1".to_string(), "127.0.0.1".to_string()];
    assert!(!config.is_ip_allowed("192.168.1.1"));
    assert!(!config.is_ip_allowed("127.0.0.1"));
    assert!(config.is_ip_allowed("10.0.0.1"));
    assert!(config.is_ip_allowed("8.8.8.8"));
}

#[test]
fn test_api_key_config_signature_algorithm() {
    let mut config = ApiKeyConfig::new("test-key".to_string());

    // No key material
    assert!(config.signature_algorithm().is_none());

    // HMAC key
    config.key_material = Some(KeyMaterial::HmacSecret("secret".to_string()));
    assert_eq!(
        config.signature_algorithm(),
        Some(SignatureAlgorithm::HmacSha256)
    );
}

#[test]
fn test_api_key_config_get_key_material() {
    let mut config = ApiKeyConfig::new("test-key".to_string());

    assert!(config.get_key_material().is_none());

    let key_material = KeyMaterial::HmacSecret("secret".to_string());
    config.key_material = Some(key_material.clone());

    match config.get_key_material() {
        Some(KeyMaterial::HmacSecret(secret)) => assert_eq!(secret, "secret"),
        _ => panic!("Expected HMAC secret"),
    }
}

#[test]
fn test_api_key_config_requires_signature() {
    let config = ApiKeyConfig::new("test-key".to_string());

    assert!(!config.requires_signature(SecurityType::None));
    assert!(!config.requires_signature(SecurityType::UserStream));
    assert!(config.requires_signature(SecurityType::Trade));
    assert!(config.requires_signature(SecurityType::UserData));
}

#[test]
fn test_api_key_config_chaining() {
    let config = ApiKeyConfig::new("test-key".to_string())
        .with_hmac_secret("secret".to_string())
        .with_permissions(vec![SecurityType::Trade])
        .with_recv_window(10000)
        .with_enabled(false)
        .with_description("Test key".to_string())
        .with_ip_blacklist(vec!["127.0.0.1".to_string()]);

    assert_eq!(config.api_key, "test-key");
    assert!(matches!(
        config.key_material,
        Some(KeyMaterial::HmacSecret(_))
    ));
    assert_eq!(config.permissions, vec![SecurityType::Trade]);
    assert_eq!(config.recv_window, 10000);
    assert!(!config.enabled);
    assert_eq!(config.description, Some("Test key".to_string()));
    assert_eq!(config.ip_blacklist, vec!["127.0.0.1".to_string()]);
}

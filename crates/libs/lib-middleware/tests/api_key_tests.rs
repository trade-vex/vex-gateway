//! The database row to API-key conversion the middleware runs on every lookup.

use chrono::{DateTime, Duration, Utc};
use lib_middleware::api_key::security::{KeyMaterial, SecurityType};
use lib_middleware::models::api_key::{KeyMaterialType, Model};
use uuid::Uuid;

fn row(enabled: bool, expires_at: Option<DateTime<Utc>>) -> Model {
    Model {
        id: Uuid::new_v4(),
        api_key: "vex_row".to_string(),
        key_material_type: Some(KeyMaterialType::HmacSecret),
        key_material_value: Some("row-secret".to_string()),
        permissions: vec![SecurityType::Trade, SecurityType::UserData],
        recv_window: 7000,
        enabled,
        description: Some("trading bot".to_string()),
        ip_blacklist: vec!["198.51.100.0/24".to_string()],
        user_id: Some(Uuid::new_v4()),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at,
        last_used_at: None,
    }
}

#[test]
fn to_api_key_config_maps_every_field() {
    let row = row(true, Some(Utc::now() + Duration::days(1)));

    let config = row.to_api_key_config();

    assert_eq!(config.api_key, row.api_key);
    assert!(matches!(
        config.key_material,
        Some(KeyMaterial::HmacSecret(ref secret)) if secret == "row-secret"
    ));
    assert_eq!(config.permissions, row.permissions);
    assert_eq!(config.recv_window, 7000);
    assert_eq!(config.enabled, row.enabled);
    assert_eq!(config.description, row.description);
    assert_eq!(config.created_at, Some(row.created_at));
    assert_eq!(config.expires_at, row.expires_at);
    assert_eq!(config.ip_blacklist, row.ip_blacklist);
    assert_eq!(config.user_id, row.user_id);

    // A row without a secret has no key material, so its signed requests are refused.
    let unsigned = Model {
        key_material_value: None,
        ..row
    };
    assert!(unsigned.to_api_key_config().key_material.is_none());
}

#[test]
fn model_is_valid_only_when_enabled_and_unexpired() {
    let minute = Duration::minutes(1);

    assert!(row(true, None).is_valid());
    assert!(row(true, Some(Utc::now() + minute)).is_valid());
    assert!(!row(false, None).is_valid());
    assert!(!row(true, Some(Utc::now() - minute)).is_valid());
}

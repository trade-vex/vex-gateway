use crate::error::auth_error::AuthError;
use crate::handlers::oauth::validate_callback_state_cookie;
use crate::service::oauth_service::OAuthCallbackMeta;

#[test]
fn callback_rejects_app_challenge_without_app_platform_or_state_cookie() {
    let meta = OAuthCallbackMeta {
        platform: None,
        app_challenge: Some("challenge".to_string()),
    };

    let result = validate_callback_state_cookie(&meta, false);

    assert!(matches!(result, Err(AuthError::StateMismatch)));
}

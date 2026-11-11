//! The user repository's statements that sign-up rests on, against the read model's Postgres
//! schema: only an unverified, pending sign-up can be deleted or verified.

mod support;

use std::sync::Arc;

use lib_auth::AuthError;
use lib_auth::models::user::{Model, UserStatus};
use lib_auth::repository::{PostgresUserRepository, UserRepository};
use support::require_database;
use uuid::Uuid;

/// A password sign-up with its own email and username, verified or not.
fn password_user(verified: bool) -> Model {
    let user_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    Model {
        user_id,
        email: format!("{user_id}@example.com"),
        username: user_id.to_string(),
        password_hash: Some("hash".to_string()),
        auth_provider: "email".to_string(),
        provider_id: None,
        avatar_url: None,
        email_verified: verified,
        status: if verified {
            UserStatus::Active
        } else {
            UserStatus::Pending
        },
        created_at: now,
        updated_at: now,
        last_login: None,
    }
}

#[tokio::test]
async fn only_an_unverified_pending_sign_up_can_be_deleted_or_verified() {
    let Some(db) = require_database().await else {
        return;
    };
    let repo = PostgresUserRepository::new(Arc::new(db));

    // A pending sign-up is deleted, frees its email, and cannot be deleted twice.
    let pending = repo.create_user(&password_user(false)).await.unwrap();
    assert_eq!(pending.status, UserStatus::Pending);
    repo.delete_unverified(pending.user_id).await.unwrap();
    assert!(repo.find_by_email(&pending.email).await.unwrap().is_none());
    assert!(matches!(
        repo.delete_unverified(pending.user_id).await,
        Err(AuthError::EmailAlreadyExists)
    ));
    let again = Model {
        user_id: Uuid::new_v4(),
        ..pending
    };
    repo.create_user(&again).await.unwrap();

    // Verifying it activates it once; after that it is neither verified again nor deleted.
    repo.verify_email(again.user_id).await.unwrap();
    let verified = repo.find_by_email(&again.email).await.unwrap().unwrap();
    assert!(verified.email_verified && verified.status == UserStatus::Active);
    assert!(matches!(
        repo.verify_email(again.user_id).await,
        Err(AuthError::InvalidCredentials)
    ));
    assert!(matches!(
        repo.delete_unverified(again.user_id).await,
        Err(AuthError::EmailAlreadyExists)
    ));

    // A row created verified (OAuth) stays.
    let oauth = repo.create_user(&password_user(true)).await.unwrap();
    assert!(matches!(
        repo.delete_unverified(oauth.user_id).await,
        Err(AuthError::EmailAlreadyExists)
    ));
    assert!(repo.find_by_email(&oauth.email).await.unwrap().is_some());

    // No such row.
    assert!(matches!(
        repo.verify_email(Uuid::new_v4()).await,
        Err(AuthError::InvalidCredentials)
    ));
}

use crate::dto::oauth::UserInfo;
use crate::error::auth_error::AuthError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow, Clone)]
pub struct User {
    pub user_id: Uuid,
    pub email: String,
    pub username: String,
    pub password_hash: Option<String>,
    pub auth_provider: String,
    pub provider_id: Option<String>,
    pub avatar_url: Option<String>,
    pub email_verified: bool,
    pub status: UserStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::Type)]
#[sqlx(type_name = "user_status", rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Pending,
    Suspended,
    Deleted,
}

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid) -> Result<Option<User>, AuthError>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, AuthError>;
    async fn find_by_provider_id(
        &self,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<User>, AuthError>;
    async fn create_user(&self, user: &User) -> Result<User, AuthError>;
    async fn update_last_login(&self, user_id: Uuid) -> Result<(), AuthError>;
    async fn verify_email(&self, user_id: Uuid) -> Result<(), AuthError>;
    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<(), AuthError>;
}

pub struct PostgresUserRepository {
    pool: Arc<PgPool>,
}

impl PostgresUserRepository {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserRepository for PostgresUserRepository {
    async fn find_by_id(&self, user_id: Uuid) -> Result<Option<User>, AuthError> {
        let user = sqlx::query_as!(
            User,
            r#"
            SELECT user_id, email, username, password_hash, auth_provider, 
                   provider_id, avatar_url, email_verified, 
                   status as "status: UserStatus", 
                   created_at, updated_at, last_login
            FROM users 
            WHERE user_id = $1 AND status != 'deleted'
            "#,
            user_id
        )
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| AuthError::Database(format!("Failed to find user by ID: {}", e)))?;

        Ok(user)
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<User>, AuthError> {
        let user = sqlx::query_as!(
            User,
            r#"
            SELECT user_id, email, username, password_hash, auth_provider, 
                   provider_id, avatar_url, email_verified, 
                   status as "status: UserStatus", 
                   created_at, updated_at, last_login
            FROM users 
            WHERE email = $1 AND status != 'deleted'
            "#,
            email
        )
        .fetch_optional(&*self.pool)
        .await?;

        Ok(user)
    }

    async fn find_by_provider_id(
        &self,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<User>, AuthError> {
        let user = sqlx::query_as!(
            User,
            r#"
            SELECT user_id, email, username, password_hash, auth_provider, 
                   provider_id, avatar_url, email_verified, 
                   status as "status: UserStatus", 
                   created_at, updated_at, last_login
            FROM users 
            WHERE auth_provider = $1 AND provider_id = $2 AND status != 'deleted'
            "#,
            provider,
            provider_id
        )
        .fetch_optional(&*self.pool)
        .await?;

        Ok(user)
    }

    async fn create_user(&self, user: &User) -> Result<User, AuthError> {
        let created_user = sqlx::query_as!(
            User,
            r#"
            INSERT INTO users (
                user_id, email, username, password_hash, auth_provider, 
                provider_id, avatar_url, email_verified, status, 
                created_at, updated_at, last_login
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING 
                user_id, email, username, password_hash, auth_provider, 
                provider_id, avatar_url, email_verified, 
                status as "status: UserStatus", 
                created_at, updated_at, last_login
            "#,
            user.user_id,
            user.email,
            user.username,
            user.password_hash,
            user.auth_provider,
            user.provider_id,
            user.avatar_url,
            user.email_verified,
            user.status as _, // Cast enum for sqlx
            user.created_at,
            user.updated_at,
            user.last_login
        )
        .fetch_one(&*self.pool)
        .await?;

        Ok(created_user)
    }

    async fn update_last_login(&self, user_id: Uuid) -> Result<(), AuthError> {
        let now = Utc::now();
        sqlx::query!(
            "UPDATE users SET last_login = $1, updated_at = $1 WHERE user_id = $2",
            now,
            user_id
        )
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn verify_email(&self, user_id: Uuid) -> Result<(), AuthError> {
        let now = Utc::now();
        sqlx::query!(
            "UPDATE users SET email_verified = true, status = 'active', updated_at = $1 WHERE user_id = $2",
            now,
            user_id
        )
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<(), AuthError> {
        let now = Utc::now();
        sqlx::query!(
            "UPDATE users SET password_hash = $1, updated_at = $2 WHERE user_id = $3",
            password_hash,
            now,
            user_id
        )
        .execute(&*self.pool)
        .await?;

        Ok(())
    }
}

impl From<User> for UserInfo {
    fn from(user: User) -> Self {
        Self {
            user_id: user.user_id,
            email: user.email,
            username: user.username,
            avatar_url: user.avatar_url,
            auth_provider: user.auth_provider,
            email_verified: user.email_verified,
            created_at: user.created_at,
        }
    }
}

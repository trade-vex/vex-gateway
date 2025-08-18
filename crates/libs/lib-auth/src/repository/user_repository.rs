use crate::dto::oauth::UserInfo;
use crate::error::auth_error::AuthError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use sea_orm::*;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key)]
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

#[derive(Debug, Clone, PartialEq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "user_status")]
pub enum UserStatus {
    #[sea_orm(string_value = "active")]
    Active,
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "suspended")]
    Suspended,
    #[sea_orm(string_value = "deleted")]
    Deleted,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid) -> Result<Option<Model>, AuthError>;
    async fn find_by_email(&self, email: &str) -> Result<Option<Model>, AuthError>;
    async fn find_by_provider_id(
        &self,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<Model>, AuthError>;
    async fn create_user(&self, user: &Model) -> Result<Model, AuthError>;
    async fn update_last_login(&self, user_id: Uuid) -> Result<(), AuthError>;
    async fn verify_email(&self, user_id: Uuid) -> Result<(), AuthError>;
    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<(), AuthError>;
}

pub struct PostgresUserRepository {
    db: Arc<DatabaseConnection>,
}

impl PostgresUserRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UserRepository for PostgresUserRepository {
    async fn find_by_id(&self, user_id: Uuid) -> Result<Option<Model>, AuthError> {
        let user = Entity::find_by_id(user_id)
            .filter(Column::Status.ne(UserStatus::Deleted))
            .one(&*self.db)
            .await
            .map_err(|e| AuthError::Database(format!("Failed to find user by ID: {}", e)))?;

        Ok(user)
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<Model>, AuthError> {
        let user = Entity::find()
            .filter(Column::Email.eq(email))
            .filter(Column::Status.ne(UserStatus::Deleted))
            .one(&*self.db)
            .await
            .map_err(|e| AuthError::Database(format!("Failed to find user by email: {}", e)))?;

        Ok(user)
    }

    async fn find_by_provider_id(
        &self,
        provider: &str,
        provider_id: &str,
    ) -> Result<Option<Model>, AuthError> {
        let user = Entity::find()
            .filter(Column::AuthProvider.eq(provider))
            .filter(Column::ProviderId.eq(provider_id))
            .filter(Column::Status.ne(UserStatus::Deleted))
            .one(&*self.db)
            .await
            .map_err(|e| {
                AuthError::Database(format!("Failed to find user by provider ID: {}", e))
            })?;

        Ok(user)
    }

    async fn create_user(&self, user: &Model) -> Result<Model, AuthError> {
        let user_model = ActiveModel {
            user_id: Set(user.user_id),
            email: Set(user.email.clone()),
            username: Set(user.username.clone()),
            password_hash: Set(user.password_hash.clone()),
            auth_provider: Set(user.auth_provider.clone()),
            provider_id: Set(user.provider_id.clone()),
            avatar_url: Set(user.avatar_url.clone()),
            email_verified: Set(user.email_verified),
            status: Set(user.status.clone()),
            created_at: Set(user.created_at),
            updated_at: Set(user.updated_at),
            last_login: Set(user.last_login),
        };

        let created_user = user_model
            .insert(&*self.db)
            .await
            .map_err(|e| AuthError::Database(format!("Failed to create user: {}", e)))?;

        Ok(created_user)
    }

    async fn update_last_login(&self, user_id: Uuid) -> Result<(), AuthError> {
        let now = Utc::now();
        let update_result = Entity::update_many()
            .col_expr(Column::LastLogin, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::UserId.eq(user_id))
            .exec(&*self.db)
            .await
            .map_err(|e| AuthError::Database(format!("Failed to update last login: {}", e)))?;

        if update_result.rows_affected == 0 {
            return Err(AuthError::Database("User not found".to_string()));
        }

        Ok(())
    }

    async fn verify_email(&self, user_id: Uuid) -> Result<(), AuthError> {
        let now = Utc::now();
        let update_result = Entity::update_many()
            .col_expr(Column::EmailVerified, Expr::value(true))
            .col_expr(Column::Status, Expr::value(UserStatus::Active))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::UserId.eq(user_id))
            .exec(&*self.db)
            .await
            .map_err(|e| AuthError::Database(format!("Failed to verify email: {}", e)))?;

        if update_result.rows_affected == 0 {
            return Err(AuthError::Database("User not found".to_string()));
        }

        Ok(())
    }

    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<(), AuthError> {
        let now = Utc::now();
        let update_result = Entity::update_many()
            .col_expr(Column::PasswordHash, Expr::value(password_hash))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::UserId.eq(user_id))
            .exec(&*self.db)
            .await
            .map_err(|e| AuthError::Database(format!("Failed to update password: {}", e)))?;

        if update_result.rows_affected == 0 {
            return Err(AuthError::Database("User not found".to_string()));
        }

        Ok(())
    }
}

impl From<Model> for UserInfo {
    fn from(user: Model) -> Self {
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

use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use uuid::Uuid;

/// User model with database column definitions.
///
/// # Required Database Indexes
///
/// For optimal query performance, ensure these indexes exist:
/// - `ux_users_email` - UNIQUE index on `email` (for email lookup and login)
/// - `ux_users_username` - UNIQUE index on `username` (for username uniqueness)
/// - `ix_users_auth_provider_provider_id` - Composite on `(auth_provider, provider_id)` (for OAuth lookup)
/// - `ix_users_status` - Index on `status` (for filtering active/pending users)
#[derive(Debug, Clone, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub user_id: Uuid,
    #[sea_orm(unique_key = "ux_users_email")]
    pub email: String,
    #[sea_orm(unique_key = "ux_users_username")]
    pub username: String,
    pub password_hash: Option<String>,
    #[sea_orm(indexed)]
    pub auth_provider: String,
    #[sea_orm(indexed)]
    pub provider_id: Option<String>,
    pub avatar_url: Option<String>,
    pub email_verified: bool,
    #[sea_orm(indexed)]
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

// Business logic methods
impl Model {
    pub fn is_active(&self) -> bool {
        self.status == UserStatus::Active
    }

    pub fn can_login(&self) -> bool {
        self.status == UserStatus::Active && self.email_verified
    }
}

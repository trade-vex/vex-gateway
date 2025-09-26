use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
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

// Business logic methods
impl Model {
    pub fn is_active(&self) -> bool {
        self.status == UserStatus::Active
    }

    pub fn can_login(&self) -> bool {
        self.status == UserStatus::Active && self.email_verified
    }
}

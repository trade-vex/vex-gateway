use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_balances")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub user_id: Uuid,
    pub available: i64,
    pub locked: i64,
    pub last_updated: DateTime<Utc>,
}

impl ActiveModelBehavior for ActiveModel {}

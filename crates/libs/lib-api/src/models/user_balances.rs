//! lib-api/src/models/user_balances.rs

use serde::{Deserialize, Serialize};
use sea_orm::prelude::*;
use chrono::{DateTime, Utc};

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_balances")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub user_id: i64,
    pub available: i64,
    pub locked: i64,
    pub last_updated: DateTime<Utc>,
}

impl ActiveModelBehavior for ActiveModel {}

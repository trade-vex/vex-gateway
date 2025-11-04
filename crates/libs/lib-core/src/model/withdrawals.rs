use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Withdrawal events from trading.withdrawals_events table
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "withdrawals_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub user_id: i64,
    pub asset_id: i32,
    pub amount: i64,
    pub timestamp: i64,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<DateTime<Utc>>,
}

impl ActiveModelBehavior for ActiveModel {}

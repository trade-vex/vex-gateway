use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Balance events from trading.balances_events table
/// Populated by Kafka consumer from BalanceEvent messages
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "balances_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub user_id: u64,
    pub asset_id: i32,
    pub available: i64,
    pub locked: i64,
    pub total: i64,
    pub timestamp: i64,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<DateTime<Utc>>,
}

impl ActiveModelBehavior for ActiveModel {}

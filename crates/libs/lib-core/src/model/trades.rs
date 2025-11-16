use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Trade events from trading.trades_events table
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "trades_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub maker_user_id: u64,
    pub taker_user_id: u64,
    pub market_id: i32,
    pub price: i64,
    pub size: i64,
    pub maker_order_id: u64,
    pub taker_order_id: u64,
    pub timestamp: i64,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<DateTime<Utc>>,
}

impl ActiveModelBehavior for ActiveModel {}

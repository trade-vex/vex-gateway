use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Order events from trading.orders_events table
/// Populated by Kafka consumer from OrderEvent messages
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "orders_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub order_order_id: u64,
    pub order_user_id: u64,
    pub order_price: i64,
    pub order_size: i64,
    pub order_side: String,
    pub order_timestamp: i64,
    pub market_id: i32,
    pub time_in_force: String,
    pub status: String,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<DateTime>,
}

impl ActiveModelBehavior for ActiveModel {}

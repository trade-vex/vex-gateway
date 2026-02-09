use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Order events from trading.orders_events table
/// Populated by Kafka consumer from OrderEvent messages
/// Note: order_order_id and order_user_id stored as Decimal to handle u64 range (> i64::MAX)
/// Column names match the schema with flattened Order struct prefix (order_*)
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "orders_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub market_id: i32,
    #[sea_orm(column_type = "BigInteger")]
    pub order_order_id: i64,
    #[sea_orm(column_type = "BigInteger")]
    pub order_user_id: i64,
    #[sea_orm(column_type = "BigInteger")]
    pub order_price: i64,
    #[sea_orm(column_type = "BigInteger")]
    pub order_size: i64,
    #[sea_orm(column_type = "BigInteger")]
    pub order_filled_size: i64,
    pub order_side: String,
    pub order_time_in_force: String,
    pub order_status: String,
    #[sea_orm(column_type = "BigInteger")]
    pub order_timestamp: i64,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<DateTime>,
}

impl ActiveModelBehavior for ActiveModel {}

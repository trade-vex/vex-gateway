use chrono::NaiveDateTime;
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Trade events from trading.trades_events table
/// Note: All ID fields stored as Decimal to handle u64 range (> i64::MAX)
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "trades_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    #[sea_orm(column_type = "Decimal(Some((20, 0)))")]
    pub maker_user_id: Decimal,
    #[sea_orm(column_type = "Decimal(Some((20, 0)))")]
    pub taker_user_id: Decimal,
    pub market_id: i32,
    #[sea_orm(column_type = "Decimal(Some((38, 12)))")]
    pub price: Decimal,
    #[sea_orm(column_type = "Decimal(Some((38, 18)))")]
    pub size: Decimal,
    #[sea_orm(column_type = "Decimal(Some((20, 0)))")]
    pub maker_order_id: Decimal,
    #[sea_orm(column_type = "Decimal(Some((20, 0)))")]
    pub taker_order_id: Decimal,
    pub timestamp: i64,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<NaiveDateTime>,
}

impl ActiveModelBehavior for ActiveModel {}

use chrono::NaiveDateTime;
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Balance events from trading.balances_events table
/// Populated by Kafka consumer from BalanceEvent messages
/// Note: user_id stored as Decimal to handle u64 range (> i64::MAX)
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "balances_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    #[sea_orm(column_type = "Decimal(Some((20, 0)))")]
    pub user_id: Decimal,
    pub asset_id: i32,
    #[sea_orm(column_type = "Decimal(Some((38, 18)))")]
    pub available: Decimal,
    #[sea_orm(column_type = "Decimal(Some((38, 18)))")]
    pub locked: Decimal,
    #[sea_orm(column_type = "Decimal(Some((38, 18)))")]
    pub total: Decimal,
    pub timestamp: i64,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<NaiveDateTime>,
}

impl ActiveModelBehavior for ActiveModel {}

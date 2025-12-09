use chrono::NaiveDateTime;
use rust_decimal::Decimal;
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Withdrawal events from trading.withdrawals_events table
/// Note: user_id stored as Decimal to handle u64 range (> i64::MAX)
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "withdrawals_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    #[sea_orm(column_type = "Decimal(Some((20, 0)))")]
    pub user_id: Decimal,
    pub asset_id: i32,
    #[sea_orm(column_type = "Decimal(Some((38, 18)))")]
    pub amount: Decimal,
    pub timestamp: i64,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<NaiveDateTime>,
}

impl ActiveModelBehavior for ActiveModel {}

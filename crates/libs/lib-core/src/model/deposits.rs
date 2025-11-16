use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Deposit events from trading.deposits_events table
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "deposits_events", schema_name = "trading")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub user_id: u64,
    pub asset_id: i32,
    #[sea_orm(column_type = "Decimal(Some((38, 18)))")]
    pub amount: Decimal,
    pub timestamp: i64,
    #[sea_orm(column_type = "Timestamp")]
    pub created_at: Option<DateTime<Utc>>,
}

impl ActiveModelBehavior for ActiveModel {}

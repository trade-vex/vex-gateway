use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// User favorite markets mapping
/// Maps users to their favorited markets for quick access
#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_favorite_markets")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i32,
    /// User ID as u64 (derived from UUID via XOR operation)
    pub user_id: u64,
    /// Market ID (bit-packed u32)
    pub market_id: i32,
    /// Timestamp when the market was favorited
    #[sea_orm(column_type = "TimestampWithTimeZone")]
    pub favorited_at: Option<DateTime<Utc>>,
}

impl ActiveModelBehavior for ActiveModel {}


//! lib-api/src/models/orders.rs

use serde::{Deserialize, Serialize};
use sea_orm::prelude::*;

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

#[derive(Debug, Clone, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "orders")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub order_id: i64,
    pub user_id: i64,
    pub price: i64,
    pub size: i64,
    pub side: String,
    pub timestamp: i64,
}

impl ActiveModelBehavior for ActiveModel {}

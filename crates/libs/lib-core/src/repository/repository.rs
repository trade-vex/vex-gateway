use crate::model::orders::{
    ActiveModel as OrderActiveModel, Column as OrderColumn, Entity as OrderEntity,
    Model as OrderModel,
};
use crate::model::user_balances::{
    ActiveModel as UserBalanceActiveModel, Column as UserBalanceColumn,
    Entity as UserBalanceEntity, Model as UserBalanceModel,
};
use async_trait::async_trait;
use sea_orm::prelude::*;
use sea_orm::*;
use std::sync::Arc;
use uuid::Uuid;

// Error type for trading operations
#[derive(Debug)]
pub enum TradingError {
    Database(String),
    NotFound,
}

impl std::fmt::Display for TradingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradingError::Database(msg) => write!(f, "Database error: {}", msg),
            TradingError::NotFound => write!(f, "Not found"),
        }
    }
}

impl std::error::Error for TradingError {}

impl From<DbErr> for TradingError {
    fn from(err: DbErr) -> Self {
        TradingError::Database(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, TradingError>;

#[async_trait]
pub trait TradingRepository: Send + Sync {
    // Balance operations
    async fn get_user_balances(&self, user_id: Uuid) -> Result<Vec<UserBalanceModel>>;
    async fn get_user_balance(&self, user_id: Uuid) -> Result<Option<UserBalanceModel>>;
    async fn update_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()>;
    async fn create_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()>;

    // Order operations
    async fn get_open_orders(&self, user_id: Uuid) -> Result<Vec<OrderModel>>;
    async fn get_order_by_id(&self, order_id: u64, user_id: Uuid) -> Result<Option<OrderModel>>;
    async fn create_order(&self, order: OrderModel) -> Result<u64>;
    async fn count_user_orders(&self, user_id: Uuid) -> Result<u64>;
}

// PostgreSQL implementation
pub struct PostgresTradingRepository {
    db: Arc<DatabaseConnection>,
}

impl PostgresTradingRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl TradingRepository for PostgresTradingRepository {
    async fn get_user_balances(&self, user_id: Uuid) -> Result<Vec<UserBalanceModel>> {
        // Query trading.balances_events by user_id
        // Note: Kafka writes user_id as i64, but we receive Uuid from API
        let user_id_i64 = user_id.as_u128() as i64;

        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id_i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_user_balance(&self, user_id: Uuid) -> Result<Option<UserBalanceModel>> {
        let user_id_i64 = user_id.as_u128() as i64;

        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id_i64))
            .order_by_desc(UserBalanceColumn::Timestamp)
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn update_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()> {
        let user_id_i64 = user_id.as_u128() as i64;
        let total = available + locked;

        // Insert new balance event (this is an event log table, not mutable state)
        let balance = UserBalanceActiveModel {
            user_id: Set(user_id_i64),
            asset_id: Set(0), // Default asset, adjust as needed
            available: Set(available as i64),
            locked: Set(locked as i64),
            total: Set(total as i64),
            timestamp: Set(chrono::Utc::now().timestamp()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn create_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()> {
        let user_id_i64 = user_id.as_u128() as i64;
        let total = available + locked;

        let balance = UserBalanceActiveModel {
            user_id: Set(user_id_i64),
            asset_id: Set(0), // Default asset, adjust as needed
            available: Set(available as i64),
            locked: Set(locked as i64),
            total: Set(total as i64),
            timestamp: Set(chrono::Utc::now().timestamp()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn get_open_orders(&self, user_id: Uuid) -> Result<Vec<OrderModel>> {
        // Query trading.orders_events by order_user_id
        let user_id_i64 = user_id.as_u128() as i64;

        OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id_i64))
            .order_by_desc(OrderColumn::OrderTimestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_order_by_id(&self, order_id: u64, user_id: Uuid) -> Result<Option<OrderModel>> {
        let user_id_i64 = user_id.as_u128() as i64;

        OrderEntity::find()
            .filter(
                OrderColumn::OrderOrderId
                    .eq(order_id as i64)
                    .and(OrderColumn::OrderUserId.eq(user_id_i64)),
            )
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn create_order(&self, order: OrderModel) -> Result<u64> {
        let active_model = OrderActiveModel {
            order_order_id: Set(order.order_order_id),
            order_user_id: Set(order.order_user_id),
            order_price: Set(order.order_price),
            order_size: Set(order.order_size),
            order_side: Set(order.order_side.to_string()),
            order_timestamp: Set(order.order_timestamp),
            market_id: Set(order.market_id),
            ..Default::default()
        };

        let result = active_model.insert(&*self.db).await?;
        Ok(result.order_order_id as u64)
    }

    async fn count_user_orders(&self, user_id: Uuid) -> Result<u64> {
        let user_id_i64 = user_id.as_u128() as i64;

        let count = OrderEntity::find()
            .filter(OrderColumn::OrderUserId.eq(user_id_i64))
            .count(&*self.db)
            .await?;
        Ok(count as u64)
    }
}

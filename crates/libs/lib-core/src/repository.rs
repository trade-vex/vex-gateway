use async_trait::async_trait;
use sea_orm::prelude::*;
use sea_orm::*;
use std::sync::Arc;
use uuid::Uuid;
use crate::orders::{Model as OrderModel, Entity as OrderEntity, Column as OrderColumn, ActiveModel as OrderActiveModel};
use crate::user_balances::{Model as UserBalanceModel, Entity as UserBalanceEntity, Column as UserBalanceColumn, ActiveModel as UserBalanceActiveModel};

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
        UserBalanceEntity::find()
            .filter(UserBalanceColumn::UserId.eq(user_id))
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_user_balance(&self, user_id: Uuid) -> Result<Option<UserBalanceModel>> {
        UserBalanceEntity::find_by_id(user_id)
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn update_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()> {
        let mut balance: UserBalanceActiveModel = UserBalanceEntity::find_by_id(user_id)
            .one(&*self.db)
            .await?
            .ok_or(TradingError::NotFound)?
            .into();

        balance.available = Set(available as i64);
        balance.locked = Set(locked as i64);
        balance.last_updated = Set(chrono::Utc::now());

        balance.update(&*self.db).await?;
        Ok(())
    }

    async fn create_user_balance(&self, user_id: Uuid, available: u64, locked: u64) -> Result<()> {
        let balance = UserBalanceActiveModel {
            user_id: Set(user_id),
            available: Set(available as i64),
            locked: Set(locked as i64),
            last_updated: Set(chrono::Utc::now()),
            ..Default::default()
        };

        balance.insert(&*self.db).await?;
        Ok(())
    }

    async fn get_open_orders(&self, user_id: Uuid) -> Result<Vec<OrderModel>> {
        OrderEntity::find()
            .filter(OrderColumn::UserId.eq(user_id))
            .order_by_desc(OrderColumn::Timestamp)
            .all(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn get_order_by_id(&self, order_id: u64, user_id: Uuid) -> Result<Option<OrderModel>> {
        OrderEntity::find()
            .filter(
                OrderColumn::OrderId.eq(order_id as i64)
                    .and(OrderColumn::UserId.eq(user_id))
            )
            .one(&*self.db)
            .await
            .map_err(Into::into)
    }

    async fn create_order(&self, order: OrderModel) -> Result<u64> {
        let active_model = OrderActiveModel {
            user_id: Set(order.user_id),
            price: Set(order.price),
            size: Set(order.size),
            side: Set(order.side.to_string()),
            timestamp: Set(order.timestamp),
            ..Default::default()
        };

        let result = active_model.insert(&*self.db).await?;
        Ok(result.order_id as u64)
    }

    async fn count_user_orders(&self, user_id: Uuid) -> Result<u64> {
        let count = OrderEntity::find()
            .filter(OrderColumn::UserId.eq(user_id))
            .count(&*self.db)
            .await?;
        Ok(count as u64)
    }
}

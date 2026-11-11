use async_trait::async_trait;
use chrono::Utc;
use sea_orm::prelude::*;
use sea_orm::*;
use std::sync::Arc;
use uuid::Uuid;

use crate::api_key::error::ApiKeyError;
use crate::models::api_key::{ActiveModel, Column, Entity, Model, NewApiKey};

#[async_trait]
pub trait ApiKeyRepository: Send + Sync {
    /// Find API key by key string
    async fn find_by_api_key(&self, api_key: &str) -> Result<Option<Model>, ApiKeyError>;

    /// Find API key by ID
    async fn find_by_id(&self, id: Uuid) -> Result<Option<Model>, ApiKeyError>;

    /// Find all API keys for a user
    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Vec<Model>, ApiKeyError>;

    /// Create a new API key
    async fn create(&self, new_api_key: NewApiKey) -> Result<Model, ApiKeyError>;

    /// Update API key
    async fn update(&self, id: Uuid, updates: ActiveModel) -> Result<Model, ApiKeyError>;

    /// Enable/disable API key
    async fn set_enabled(&self, id: Uuid, enabled: bool) -> Result<(), ApiKeyError>;

    /// Delete API key
    async fn delete(&self, id: Uuid) -> Result<(), ApiKeyError>;
}

pub struct PostgresApiKeyRepository {
    db: Arc<DatabaseConnection>,
}

impl PostgresApiKeyRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ApiKeyRepository for PostgresApiKeyRepository {
    async fn find_by_api_key(&self, api_key: &str) -> Result<Option<Model>, ApiKeyError> {
        let result = Entity::find()
            .filter(Column::ApiKey.eq(api_key))
            .filter(Column::Enabled.eq(true))
            .filter(
                Column::ExpiresAt
                    .is_null()
                    .or(Column::ExpiresAt.gt(Utc::now())),
            )
            .one(&*self.db)
            .await?;

        Ok(result)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<Model>, ApiKeyError> {
        let result = Entity::find_by_id(id).one(&*self.db).await?;

        Ok(result)
    }

    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Vec<Model>, ApiKeyError> {
        let results = Entity::find()
            .filter(Column::UserId.eq(user_id))
            .order_by_desc(Column::CreatedAt)
            .all(&*self.db)
            .await?;

        Ok(results)
    }

    async fn create(&self, new_api_key: NewApiKey) -> Result<Model, ApiKeyError> {
        let active_model = new_api_key.to_active_model();

        let result = active_model.insert(&*self.db).await?;

        Ok(result)
    }

    async fn update(&self, id: Uuid, mut updates: ActiveModel) -> Result<Model, ApiKeyError> {
        // Ensure we're updating the correct record and set updated_at
        updates.id = Set(id);
        updates.updated_at = Set(Utc::now());

        let result = updates.update(&*self.db).await?;

        Ok(result)
    }

    async fn set_enabled(&self, id: Uuid, enabled: bool) -> Result<(), ApiKeyError> {
        let update_result = Entity::update_many()
            .col_expr(Column::Enabled, Expr::value(enabled))
            .col_expr(Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(Column::Id.eq(id))
            .exec(&*self.db)
            .await?;

        if update_result.rows_affected == 0 {
            return Err(ApiKeyError::NotFound);
        }

        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), ApiKeyError> {
        let result = Entity::delete_by_id(id).exec(&*self.db).await?;
        if result.rows_affected == 0 {
            return Err(ApiKeyError::NotFound);
        }
        Ok(())
    }
}

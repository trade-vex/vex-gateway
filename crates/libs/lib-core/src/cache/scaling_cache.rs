use rust_decimal::Decimal;
use sea_orm::prelude::*;
use sea_orm::{DatabaseConnection, Statement};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
struct MarketScaling {
    quote_scale_k: u64,
    base_asset_id: u16,
}

/// Scaling cache that loads market and asset scaling factors from the database
/// and provides methods to descale u64 values to human-readable decimals
#[derive(Clone)]
pub struct ScalingCache {
    markets: Arc<RwLock<HashMap<u32, MarketScaling>>>,
    assets: Arc<RwLock<HashMap<u16, u64>>>,
    db: Arc<DatabaseConnection>,
}

impl ScalingCache {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self {
            markets: Arc::new(RwLock::new(HashMap::new())),
            assets: Arc::new(RwLock::new(HashMap::new())),
            db,
        }
    }

    /// Load all scaling factors from the database
    pub async fn load_all(&self) -> Result<(), String> {
        self.load_markets().await?;
        self.load_assets().await?;
        info!(
            "Scaling cache loaded: {} markets, {} assets",
            self.markets.read().unwrap().len(),
            self.assets.read().unwrap().len()
        );
        Ok(())
    }

    async fn load_markets(&self) -> Result<(), String> {
        let query = r#"
            SELECT market_id, quote_scale_k, base_asset, quote_asset
            FROM markets
            WHERE active = true
        "#;

        let stmt = Statement::from_string(sea_orm::DatabaseBackend::Postgres, query.to_string());

        let result = self
            .db
            .query_all(stmt)
            .await
            .map_err(|e| format!("Failed to load markets: {}", e))?;

        let mut new_markets = HashMap::new();
        for row in result {
            // Access columns by name
            let market_id: i32 = row
                .try_get::<i32>("", "market_id")
                .map_err(|e| format!("Failed to parse market_id: {}", e))?;
            let quote_scale_k: i64 = row
                .try_get::<i64>("", "quote_scale_k")
                .map_err(|e| format!("Failed to parse quote_scale_k: {}", e))?;
            let base_asset: i16 = row
                .try_get::<i16>("", "base_asset")
                .map_err(|e| format!("Failed to parse base_asset: {}", e))?;

            new_markets.insert(
                market_id as u32,
                MarketScaling {
                    quote_scale_k: quote_scale_k as u64,
                    base_asset_id: base_asset as u16,
                },
            );
        }

        *self.markets.write().unwrap() = new_markets;
        debug!(
            "Loaded {} markets into scaling cache",
            self.markets.read().unwrap().len()
        );
        Ok(())
    }

    async fn load_assets(&self) -> Result<(), String> {
        let query = r#"
            SELECT asset_id, native_scale
            FROM assets
            WHERE active = true
        "#;

        let stmt = Statement::from_string(sea_orm::DatabaseBackend::Postgres, query.to_string());

        let result = self
            .db
            .query_all(stmt)
            .await
            .map_err(|e| format!("Failed to load assets: {}", e))?;

        let mut new_assets = HashMap::new();
        for row in result {
            // Access columns by name
            let asset_id: i16 = row
                .try_get::<i16>("", "asset_id")
                .map_err(|e| format!("Failed to parse asset_id: {}", e))?;
            let native_scale: i64 = row
                .try_get::<i64>("", "native_scale")
                .map_err(|e| format!("Failed to parse native_scale: {}", e))?;

            new_assets.insert(asset_id as u16, native_scale as u64);
        }

        *self.assets.write().unwrap() = new_assets;
        debug!(
            "Loaded {} assets into scaling cache",
            self.assets.read().unwrap().len()
        );
        Ok(())
    }

    /// Descale a price value using quote_scale_k for the given market
    pub fn scale_price(&self, market_id: u32, price_u64: Decimal) -> Decimal {
        let markets = self.markets.read().unwrap();
        if let Some(scaling) = markets.get(&market_id) {
            if scaling.quote_scale_k > 0 {
                price_u64 / Decimal::from(scaling.quote_scale_k)
            } else {
                warn!(
                    "Invalid quote_scale_k for market {}, returning raw value",
                    market_id
                );
                price_u64
            }
        } else {
            warn!(
                "Market {} not found in scaling cache, returning raw value",
                market_id
            );
            price_u64
        }
    }

    /// Descale a volume/size value using base asset native_scale for the given market
    pub fn scale_volume(&self, market_id: u32, volume_u64: Decimal) -> Decimal {
        let markets = self.markets.read().unwrap();
        if let Some(scaling) = markets.get(&market_id) {
            let assets = self.assets.read().unwrap();
            if let Some(native_scale) = assets.get(&scaling.base_asset_id) {
                if *native_scale > 0 {
                    volume_u64 / Decimal::from(*native_scale)
                } else {
                    warn!(
                        "Invalid native_scale for asset {}, returning raw value",
                        scaling.base_asset_id
                    );
                    volume_u64
                }
            } else {
                warn!(
                    "Asset {} not found in scaling cache, returning raw value",
                    scaling.base_asset_id
                );
                volume_u64
            }
        } else {
            warn!(
                "Market {} not found in scaling cache, returning raw value",
                market_id
            );
            volume_u64
        }
    }

    /// Descale a balance amount using asset native_scale
    pub fn scale_balance(&self, asset_id: u16, amount_u64: Decimal) -> Decimal {
        let assets = self.assets.read().unwrap();
        if let Some(native_scale) = assets.get(&asset_id) {
            if *native_scale > 0 {
                amount_u64 / Decimal::from(*native_scale)
            } else {
                warn!(
                    "Invalid native_scale for asset {}, returning raw value",
                    asset_id
                );
                amount_u64
            }
        } else {
            warn!(
                "Asset {} not found in scaling cache, returning raw value",
                asset_id
            );
            amount_u64
        }
    }

    /// Refresh the cache by reloading from database
    pub async fn refresh(&self) -> Result<(), String> {
        self.load_all().await
    }
}

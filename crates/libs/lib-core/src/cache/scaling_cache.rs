use rust_decimal::prelude::ToPrimitive;
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

    /// A cache of the given scales instead of the database's: `markets` as (market_id, base asset,
    /// quote_scale_k), `assets` as (asset_id, native_scale). For tests.
    pub fn with_scales(markets: &[(u32, u16, u64)], assets: &[(u16, u64)]) -> Self {
        let cache = Self::new(Arc::new(DatabaseConnection::default()));
        *cache.markets.write().unwrap() = markets
            .iter()
            .map(|&(market_id, base_asset_id, quote_scale_k)| {
                (
                    market_id,
                    MarketScaling {
                        quote_scale_k,
                        base_asset_id,
                    },
                )
            })
            .collect();
        *cache.assets.write().unwrap() = assets.iter().copied().collect();
        cache
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

    /// `price`, in the quote asset, as the engine's integer: a multiple of the market's price step
    /// 1/quote_scale_k. The error is for the client.
    pub fn price_to_engine(&self, market_id: u32, price: Decimal) -> Result<u64, String> {
        let markets = self.markets.read().unwrap();
        let market = markets
            .get(&market_id)
            .ok_or_else(|| format!("unknown market {market_id}"))?;
        to_engine(price, market.quote_scale_k, "price")
    }

    /// `volume`, in the base asset, as the engine's integer: a multiple of the asset's smallest
    /// unit 1/native_scale. The error is for the client.
    pub fn volume_to_engine(&self, market_id: u32, volume: Decimal) -> Result<u64, String> {
        let base_asset_id = self
            .markets
            .read()
            .unwrap()
            .get(&market_id)
            .ok_or_else(|| format!("unknown market {market_id}"))?
            .base_asset_id;
        let scale = *self
            .assets
            .read()
            .unwrap()
            .get(&base_asset_id)
            .ok_or_else(|| format!("unknown asset {base_asset_id}"))?;
        to_engine(volume, scale, "size")
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
}

/// `value * scale` as a `u64`, if it is a whole number in range. `checked_mul` and `to_u64` are
/// the overflow guards: a `Decimal` holds up to ~7.9e28, a `u64` 1.8e19. `scale` is never 0 (the
/// schema checks it).
fn to_engine(value: Decimal, scale: u64, what: &str) -> Result<u64, String> {
    let scaled = value
        .checked_mul(Decimal::from(scale))
        .ok_or_else(|| format!("{what} is out of range"))?;
    if scaled.is_sign_negative() || !scaled.is_integer() {
        return Err(format!(
            "{what} must be a non-negative multiple of {}",
            (Decimal::ONE / Decimal::from(scale)).normalize()
        ));
    }
    scaled
        .to_u64()
        .ok_or_else(|| format!("{what} is out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_and_volume_convert_to_engine_units() {
        let cache = ScalingCache::with_scales(&[(65538, 2, 10)], &[(2, 100_000_000)]);
        let dec = |value: &str| value.parse::<Decimal>().unwrap();

        assert_eq!(cache.price_to_engine(65538, dec("3000.1")), Ok(30001));
        assert_eq!(cache.price_to_engine(65538, dec("3000.10")), Ok(30001));
        assert_eq!(
            cache.price_to_engine(65538, dec("3000.15")),
            Err("price must be a non-negative multiple of 0.1".to_string())
        );
        assert_eq!(cache.volume_to_engine(65538, dec("0.5")), Ok(50_000_000));
        assert_eq!(
            cache.volume_to_engine(65538, dec("0.000000001")),
            Err("size must be a non-negative multiple of 0.00000001".to_string())
        );
        assert!(cache.volume_to_engine(65538, dec("-1")).is_err());
        assert_eq!(
            cache.volume_to_engine(65538, dec("100000000000000000000")),
            Err("size is out of range".to_string())
        );
        assert_eq!(
            cache.price_to_engine(65538, Decimal::MAX),
            Err("price is out of range".to_string())
        );
        assert_eq!(
            cache.price_to_engine(65539, dec("1")),
            Err("unknown market 65539".to_string())
        );
        assert_eq!(
            cache.volume_to_engine(65539, dec("1")),
            Err("unknown market 65539".to_string())
        );
    }
}

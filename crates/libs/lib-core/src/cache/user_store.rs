use crate::cache::scaling_cache::ScalingCache;
use crate::dto::trading::UserBalanceResponse;
use crate::model::user_balances::Model as UserBalanceModel;
use rust_decimal::Decimal;
use std::sync::Arc;

/// Turns the read model's balance rows (`TradingRepository::get_user_balances`) into API
/// responses, descaled with the scaling cache.
#[derive(Clone)]
pub struct UserBalanceStore {
    scaling_cache: Arc<ScalingCache>,
}

impl UserBalanceStore {
    pub fn new(scaling_cache: Arc<ScalingCache>) -> Self {
        Self { scaling_cache }
    }

    /// The response for a `trading.balances` row, whose `u64` values it decodes with `as u64`.
    pub fn to_response(&self, model: UserBalanceModel) -> UserBalanceResponse {
        let asset_id = model.asset_id as u16;
        let amount = |stored: i64| {
            self.scaling_cache
                .scale_balance(asset_id, Decimal::from(stored as u64))
        };

        UserBalanceResponse {
            user_id: model.user_id as u64,
            asset_id: model.asset_id as u32,
            available: amount(model.available),
            locked: amount(model.locked),
            total: amount(model.total),
            timestamp: model.event_ts as u64,
        }
    }
}

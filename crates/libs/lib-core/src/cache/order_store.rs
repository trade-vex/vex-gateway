use crate::cache::scaling_cache::ScalingCache;
use crate::dto::trading::{OrderHistoryResponse, OrderResponse, Side};
use crate::model::orders::Model as OrderModel;
use rust_decimal::Decimal;
use std::sync::Arc;
use tracing::error;

/// Turns the read model's order rows (`TradingRepository`'s order pages) into API responses,
/// descaled with the scaling cache. The rows' `u64` values are decoded with `as u64`.
#[derive(Clone)]
pub struct OrderStore {
    scaling_cache: Arc<ScalingCache>,
}

impl OrderStore {
    pub fn new(scaling_cache: Arc<ScalingCache>) -> Self {
        Self { scaling_cache }
    }

    /// Convert the stored status (STATUS_*) to the API's form
    fn normalize_status(status: &str) -> String {
        match status {
            "STATUS_REJECTED" => "Rejected",
            "STATUS_PLACED" => "Placed",
            "STATUS_CANCELLED" => "Cancelled",
            "STATUS_PARTIALLY_FILLED" => "PartiallyFilled",
            "STATUS_FILLED" => "Filled",
            "STATUS_PROCESSING" => "Processing",
            "STATUS_PROCESSED" => "Processed",
            // An unknown value is passed through rather than guessed at.
            other => other,
        }
        .to_string()
    }

    /// The stored side, or `None` (logged) when it is not one of core's `Side` names. Such a
    /// row is skipped: guessing a side would show the user an order on the wrong side of the
    /// book. The handlers' `total` comes from the repository's count, so a page holding a
    /// skipped row is short.
    fn parse_side(model: &OrderModel) -> Option<Side> {
        match model.order_side.parse() {
            Ok(side) => Some(side),
            Err(_) => {
                error!(
                    order_id = model.order_order_id as u64,
                    order_side = %model.order_side,
                    "Skipping stored order with an unknown order_side"
                );
                None
            }
        }
    }

    fn price(&self, model: &OrderModel) -> Decimal {
        let price = Decimal::from(model.order_price as u64);
        self.scaling_cache
            .scale_price(model.market_id as u32, price)
    }

    fn volume(&self, model: &OrderModel, volume: u64) -> Decimal {
        self.scaling_cache
            .scale_volume(model.market_id as u32, Decimal::from(volume))
    }

    /// The open-order response: size is what remains unfilled. `None` when the stored side is
    /// unknown; see `parse_side`.
    pub fn to_response(&self, model: OrderModel) -> Option<OrderResponse> {
        let side = Self::parse_side(&model)?;
        let remaining = (model.order_size as u64).saturating_sub(model.order_filled_size as u64);
        Some(OrderResponse {
            order_id: model.order_order_id as u64,
            user_id: model.order_user_id as u64,
            market_id: model.market_id as u32,
            price: self.price(&model),
            size: self.volume(&model, remaining),
            side,
            timestamp: model.event_ts as u64,
            status: Self::normalize_status(&model.order_status),
            time_in_force: model.order_time_in_force,
        })
    }

    /// The order-history response: size is the original size, filled what was filled. `None`
    /// when the stored side is unknown; see `parse_side`.
    pub fn to_history_response(&self, model: OrderModel) -> Option<OrderHistoryResponse> {
        let side = Self::parse_side(&model)?;
        Some(OrderHistoryResponse {
            order_id: model.order_order_id as u64,
            user_id: model.order_user_id as u64,
            market_id: model.market_id as u32,
            price: self.price(&model),
            size: self.volume(&model, model.order_size as u64),
            filled: self.volume(&model, model.order_filled_size as u64),
            side,
            timestamp: model.event_ts as u64,
            status: Self::normalize_status(&model.order_status),
            time_in_force: model.order_time_in_force,
        })
    }
}

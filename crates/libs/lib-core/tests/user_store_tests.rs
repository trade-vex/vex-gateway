//! `UserBalanceStore`: the API responses for the read model's `trading.balances` rows.

use lib_core::cache::{scaling_cache::ScalingCache, user_store::UserBalanceStore};
use lib_core::model::user_balances::Model as UserBalanceModel;
use rust_decimal::Decimal;
use sea_orm::DatabaseConnection;
use std::sync::Arc;

/// vex-core's `u64` values reach Postgres with the same bits in `BIGINT` columns, so those above
/// `i64::MAX` are stored negative: the response decodes them with `as u64`.
#[test]
fn u64_values_above_i64_max_are_decoded_from_their_bits() {
    let ten_to_the_19 = 10_000_000_000_000_000_000_u64;
    let row = UserBalanceModel {
        user_id: ((1_u64 << 63) + 1) as i64,
        asset_id: 3,
        available: ten_to_the_19 as i64,
        locked: (1_u64 << 63) as i64,
        total: u64::MAX as i64,
        event_ts: 1_700_000_000_000_000_000,
    };

    // A scaling cache that knows no asset, so that the store answers the stored integers.
    let scaling_cache = ScalingCache::new(Arc::new(DatabaseConnection::default()));
    let response = UserBalanceStore::new(Arc::new(scaling_cache)).to_response(row);

    assert_eq!(response.user_id, (1 << 63) + 1);
    assert_eq!(response.asset_id, 3);
    assert_eq!(response.available, Decimal::from(ten_to_the_19));
    assert_eq!(response.locked, Decimal::from(1_u64 << 63));
    assert_eq!(response.total, Decimal::from(u64::MAX));
    assert_eq!(response.timestamp, 1_700_000_000_000_000_000);
    // The id travels as a string, which keeps every u64 exact.
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["user_id"], ((1_u64 << 63) + 1).to_string());
}

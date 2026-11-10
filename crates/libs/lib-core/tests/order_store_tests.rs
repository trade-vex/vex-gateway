//! `OrderStore`: the API responses for the read model's `trading.orders` rows.

use lib_core::cache::{order_store::OrderStore, scaling_cache::ScalingCache};
use lib_core::dto::trading::Side;
use lib_core::model::orders::Model as OrderModel;
use rust_decimal::Decimal;
use sea_orm::DatabaseConnection;
use std::sync::Arc;

/// A store whose scaling cache knows no market, so that it answers the stored integers.
fn unscaled_store() -> OrderStore {
    OrderStore::new(Arc::new(ScalingCache::new(Arc::new(
        DatabaseConnection::default(),
    ))))
}

fn order(side: &str, status: &str) -> OrderModel {
    OrderModel {
        order_order_id: 1,
        order_user_id: 100,
        order_price: 50_000,
        order_size: 1_000,
        order_filled_size: 400,
        order_side: side.to_string(),
        order_time_in_force: "TIF_GTC".to_string(),
        order_status: status.to_string(),
        event_ts: 1_700_000_000_000_000_000,
        market_id: 65538,
    }
}

#[test]
fn orders_with_an_unknown_stored_side_are_skipped_not_shown_as_bids() {
    let store = unscaled_store();

    let bid = store
        .to_response(order("SIDE_BID", "STATUS_PLACED"))
        .expect("SIDE_BID is core's bid");
    assert_eq!(bid.side, Side::Bid);
    let ask = store
        .to_history_response(order("SIDE_ASK", "STATUS_FILLED"))
        .expect("SIDE_ASK is core's ask");
    assert_eq!(ask.side, Side::Ask);

    // Anything else is left out: guessing would show the order on the wrong side of the book.
    for side in ["Bid", "buy", "sell", "SIDE_UNSPECIFIED", ""] {
        assert!(
            store.to_response(order(side, "STATUS_PLACED")).is_none(),
            "{side:?} shown"
        );
        assert!(
            store
                .to_history_response(order(side, "STATUS_FILLED"))
                .is_none(),
            "{side:?} shown"
        );
    }
}

#[test]
fn open_orders_show_what_remains_and_history_what_was_filled() {
    let store = unscaled_store();

    let open = store
        .to_response(order("SIDE_BID", "STATUS_PARTIALLY_FILLED"))
        .unwrap();
    assert_eq!(open.size, Decimal::from(600));
    assert_eq!(open.status, "PartiallyFilled");
    assert_eq!(open.time_in_force, "TIF_GTC");
    assert_eq!(open.timestamp, 1_700_000_000_000_000_000);

    let history = store
        .to_history_response(order("SIDE_BID", "STATUS_CANCELLED"))
        .unwrap();
    assert_eq!(history.size, Decimal::from(1_000));
    assert_eq!(history.filled, Decimal::from(400));
    assert_eq!(history.status, "Cancelled");
}

/// vex-core's `u64` values reach Postgres with the same bits in `BIGINT` columns, so those above
/// `i64::MAX` are stored negative: the responses decode them with `as u64`.
#[test]
fn u64_values_above_i64_max_are_decoded_from_their_bits() {
    let store = unscaled_store();
    let big = |value: u64| value as i64;
    let row = OrderModel {
        order_order_id: big((1 << 63) + 12345),
        order_user_id: big(u64::MAX),
        order_price: big(u64::MAX - 1),
        order_size: big(u64::MAX),
        order_filled_size: big(1 << 63),
        market_id: i64::from(u32::MAX),
        ..order("SIDE_ASK", "STATUS_PARTIALLY_FILLED")
    };

    let open = store.to_response(row.clone()).unwrap();
    assert_eq!(open.order_id, (1 << 63) + 12345);
    assert_eq!(open.user_id, u64::MAX);
    assert_eq!(open.market_id, u32::MAX);
    assert_eq!(open.price, Decimal::from(u64::MAX - 1));
    assert_eq!(open.size, Decimal::from(u64::MAX - (1 << 63)));

    let history = store.to_history_response(row).unwrap();
    assert_eq!(history.order_id, (1 << 63) + 12345);
    assert_eq!(history.user_id, u64::MAX);
    assert_eq!(history.size, Decimal::from(u64::MAX));
    assert_eq!(history.filled, Decimal::from(1_u64 << 63));

    // Ids travel as strings, which keep every u64 exact.
    let json = serde_json::to_value(&open).unwrap();
    assert_eq!(json["order_id"], ((1_u64 << 63) + 12345).to_string());
    assert_eq!(json["user_id"], u64::MAX.to_string());
}

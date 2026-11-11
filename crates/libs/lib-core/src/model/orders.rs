use sea_orm::FromQueryResult;

/// A row of `trading.orders`: an order after the latest command that changed it, which Kafka
/// Connect writes from vex-core's `OrderEvent`s (the `Order` fields carry the `order_` prefix of
/// Connect's Flatten). The gateway only reads it, so this is a query result, not an entity that
/// could be written.
///
/// vex-core's `u64` values are stored with the same bits in these `BIGINT` columns: decode them
/// with `as u64` (`as u32` for `market_id`). Side, time in force and status are vex-core's
/// protobuf enum value names (`SIDE_*`, `TIF_*`, `STATUS_*`).
#[derive(Debug, Clone, PartialEq, FromQueryResult)]
pub struct Model {
    pub order_order_id: i64,
    pub order_user_id: i64,
    pub order_price: i64,
    pub order_size: i64,
    pub order_filled_size: i64,
    pub order_side: String,
    pub order_time_in_force: String,
    pub order_status: String,
    /// The command's engine timestamp, in nanoseconds since the Unix epoch.
    pub event_ts: i64,
    pub market_id: i64,
}

use sea_orm::FromQueryResult;

/// A row of `trading.balances`: a user's balance of one asset after the latest command that
/// changed it, which Kafka Connect writes from vex-core's `BalanceEvent`s. The gateway only reads
/// it, so this is a query result, not an entity that could be written.
///
/// vex-core's `u64` values are stored with the same bits in these `BIGINT` columns: decode them
/// with `as u64` (`as u16` for `asset_id`).
#[derive(Debug, Clone, PartialEq, FromQueryResult)]
pub struct Model {
    pub user_id: i64,
    pub asset_id: i64,
    pub available: i64,
    pub locked: i64,
    pub total: i64,
    /// The command's engine timestamp, in nanoseconds since the Unix epoch.
    pub event_ts: i64,
}

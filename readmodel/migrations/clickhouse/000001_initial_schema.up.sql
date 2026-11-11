-- The read model's initial ClickHouse schema (trade-vex/vex-core#189):
-- - one raw table per vex-core topic, which Kafka Connect's ClickHouse sink fills
-- - the markets and assets reference tables, which readmodel/seeds fills
-- - per-minute balances and prices, which materialized views keep
-- - the views the gateway reads, each through FINAL, with engine integers scaled to decimals
-- - the roles that keep the writer and the reader apart, and the views' owner
--
-- golang-migrate applies it, with x-multi-statement=true, to the database that its URL names and
-- the deploy created. Every name below is unqualified, so it lands in that database. The tool cuts
-- the file at every semicolon, so no comment may contain one.
--
-- Every proto integer is a UInt64, uint32 included: Connect carries both as INT64, which the sink
-- writes to a UInt64 with all 8 bytes, while it refuses a 64-bit field for a narrower column.
-- Enums arrive as their value names. Columns are the proto fields after Connect's Flatten
-- (delimiter "_") with the event's timestamp renamed to event_ts, vex-core's command time in
-- nanoseconds, as in Postgres.

-- ClickHouse 26.3 or later: the oldest LTS the read model is tested on. Every older release is past
-- its end of support.
SELECT throwIf(
    (toUInt32(splitByChar('.', version())[1]), toUInt32(splitByChar('.', version())[2])) < (26, 3),
    'the read model needs ClickHouse 26.3 or later'
);

-- Roles and the views' owner
--
-- Users and roles belong to the server, so another database on it may have created them. The
-- deploy creates the users, with their secrets: the logins, each granted one of the two roles,
-- and readmodel_owner (RUNBOOK "Read Model"). readmodel_owner owns the views: they read with its
-- privileges, which are granted at the end. Nobody can log in as it, from any host, whatever
-- authentication the deploy gave it: this fails when the deploy has not created it.
CREATE ROLE IF NOT EXISTS readmodel_connect;
CREATE ROLE IF NOT EXISTS readmodel_gateway;
ALTER USER readmodel_owner HOST NONE;

-- Raw tables: one row per (key, event_ts)
--
-- A ReplacingMergeTree keyed by the event's key and event_ts keeps one copy of each event, the
-- last one written, once its parts merge: reads go through FINAL, which collapses the copies that
-- are not merged yet. The sink skips MATERIALIZED columns, so ts never needs a value from it.
-- Copies of an event share event_ts, hence ts and the partition, as merging needs.

-- topic balances
CREATE TABLE balance_events (
    user_id   UInt64,
    asset_id  UInt64,
    available UInt64,
    locked    UInt64,
    total     UInt64,
    event_ts  UInt64,
    ts        DateTime64(9, 'UTC') MATERIALIZED fromUnixTimestamp64Nano(toInt64(event_ts))
) ENGINE = ReplacingMergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (user_id, asset_id, event_ts);

-- topic orders
CREATE TABLE order_events (
    order_order_id      UInt64,
    order_user_id       UInt64,
    order_price         UInt64,
    order_size          UInt64,
    order_side          LowCardinality(String),
    order_time_in_force LowCardinality(String),
    order_status        LowCardinality(String),
    event_ts            UInt64,
    order_filled_size   UInt64,
    market_id           UInt64,
    ts                  DateTime64(9, 'UTC') MATERIALIZED fromUnixTimestamp64Nano(toInt64(event_ts))
) ENGINE = ReplacingMergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (order_order_id, event_ts);

-- topic orderbook: its levels are JSON strings
CREATE TABLE orderbook_events (
    market_id UInt64,
    bids      String,
    asks      String,
    event_ts  UInt64,
    ts        DateTime64(9, 'UTC') MATERIALIZED fromUnixTimestamp64Nano(toInt64(event_ts))
) ENGINE = ReplacingMergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (market_id, event_ts);

-- topic cancels
CREATE TABLE cancel_events (
    order_id  UInt64,
    market_id UInt64,
    user_id   UInt64,
    event_ts  UInt64,
    ts        DateTime64(9, 'UTC') MATERIALIZED fromUnixTimestamp64Nano(toInt64(event_ts))
) ENGINE = ReplacingMergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (order_id, event_ts);

-- topic trades
CREATE TABLE trade_events (
    maker_user_id  UInt64,
    taker_user_id  UInt64,
    market_id      UInt64,
    price          UInt64,
    size           UInt64,
    maker_order_id UInt64,
    taker_order_id UInt64,
    taker_side     LowCardinality(String),
    event_ts       UInt64,
    ts             DateTime64(9, 'UTC') MATERIALIZED fromUnixTimestamp64Nano(toInt64(event_ts))
) ENGINE = ReplacingMergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (taker_order_id, maker_order_id, event_ts);

-- topic deposits
CREATE TABLE deposit_events (
    user_id  UInt64,
    asset_id UInt64,
    amount   UInt64,
    event_ts UInt64,
    ts       DateTime64(9, 'UTC') MATERIALIZED fromUnixTimestamp64Nano(toInt64(event_ts))
) ENGINE = ReplacingMergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (user_id, event_ts);

-- topic withdrawals
CREATE TABLE withdrawal_events AS deposit_events;

-- Reference tables: vex-core's symbol configuration, written only by readmodel/seeds
--
-- The gateway's ScalingCache scales with the same rows in Postgres. A row stays when its market
-- or asset leaves the configuration, since history still needs its scale. Read through FINAL: a
-- seed run writes every row again.
CREATE TABLE assets (
    asset_id     UInt16,
    native_scale UInt64
) ENGINE = ReplacingMergeTree
ORDER BY asset_id;

CREATE TABLE markets (
    market_id     UInt32,
    base_asset    UInt16,
    quote_asset   UInt16,
    quote_scale_k UInt64
) ENGINE = ReplacingMergeTree
ORDER BY market_id;

-- Per minute
--
-- Each materialized view writes a row per key and minute for every block the sink inserts, with
-- a version that orders events. The ReplacingMergeTree keeps the row with the greatest version,
-- so a copy of an event, which has the version of the original, cannot count twice. They hold
-- engine integers: the views below scale them as they are read, so the seeds may arrive later.

-- The last balance of each user's asset in each minute.
CREATE TABLE balances_last_per_min (
    user_id  UInt64,
    asset_id UInt64,
    minute   DateTime('UTC'),
    total    UInt64,
    event_ts UInt64
) ENGINE = ReplacingMergeTree(event_ts)
ORDER BY (user_id, asset_id, minute);

CREATE MATERIALIZED VIEW balances_last_per_min_mv TO balances_last_per_min
DEFINER = readmodel_owner SQL SECURITY DEFINER AS
SELECT b.user_id AS user_id, b.asset_id AS asset_id, toStartOfMinute(b.ts) AS minute,
       argMax(b.total, b.event_ts) AS total, max(b.event_ts) AS event_ts
FROM balance_events AS b
GROUP BY user_id, asset_id, minute;

-- The last trade price of each market in each minute. The fills of one command share its
-- event_ts, so the version also orders them as the taker met them: a bid taker fills from the
-- lowest ask price up, an ask taker from the highest bid price down. The last fill of the last
-- command wins, whatever order the sink writes them in.
CREATE TABLE market_prices_last_per_min (
    market_id UInt64,
    minute    DateTime('UTC'),
    price     UInt64,
    version   UInt128
) ENGINE = ReplacingMergeTree(version)
ORDER BY (market_id, minute);

CREATE MATERIALIZED VIEW market_prices_last_per_min_mv TO market_prices_last_per_min
DEFINER = readmodel_owner SQL SECURITY DEFINER AS
SELECT market_id, minute, argMax(price, fill_version) AS price, max(fill_version) AS version
FROM (
    SELECT market_id, toStartOfMinute(ts) AS minute, price,
           bitShiftLeft(toUInt128(event_ts), 64)
               + if(taker_side = 'SIDE_ASK', bitNot(price), price) AS fill_version
    FROM trade_events
)
GROUP BY market_id, minute;

-- The gateway's views
--
-- Each reads its tables through FINAL and scales as the gateway's ScalingCache does: a balance or
-- an amount by its asset's native_scale, a price by its market's quote_scale_k, a size by the
-- native_scale of its market's base asset. Decimal(38, 18) holds every u64 with 18 decimals. A
-- row whose market or asset has no reference row is left out: the seeds write every configured
-- one on each deploy. The gateway may read these views and nothing else.

-- The last balance of each user's asset in each minute, in units of the asset.
CREATE VIEW balances_per_minute
DEFINER = readmodel_owner SQL SECURITY DEFINER AS
SELECT b.user_id AS user_id, a.asset_id AS asset_id, b.minute AS minute,
       toDecimal128(b.total, 18) / a.native_scale AS total
FROM balances_last_per_min AS b FINAL
INNER JOIN assets AS a FINAL ON a.asset_id = b.asset_id;

-- The last trade price of each market in each minute, as the price of its base asset in units of
-- its quote asset.
CREATE VIEW asset_prices_per_minute
DEFINER = readmodel_owner SQL SECURITY DEFINER AS
SELECT m.base_asset AS asset_id, m.quote_asset AS quote_asset_id, p.minute AS minute,
       toDecimal128(p.price, 18) / m.quote_scale_k AS price
FROM market_prices_last_per_min AS p FINAL
INNER JOIN markets AS m FINAL ON m.market_id = p.market_id;

-- Deposits, in units of the asset.
CREATE VIEW deposits
DEFINER = readmodel_owner SQL SECURITY DEFINER AS
SELECT d.user_id AS user_id, a.asset_id AS asset_id, d.ts AS ts,
       toDecimal128(d.amount, 18) / a.native_scale AS amount
FROM deposit_events AS d FINAL
INNER JOIN assets AS a FINAL ON a.asset_id = d.asset_id;

-- Withdrawals, in units of the asset.
CREATE VIEW withdrawals
DEFINER = readmodel_owner SQL SECURITY DEFINER AS
SELECT w.user_id AS user_id, a.asset_id AS asset_id, w.ts AS ts,
       toDecimal128(w.amount, 18) / a.native_scale AS amount
FROM withdrawal_events AS w FINAL
INNER JOIN assets AS a FINAL ON a.asset_id = w.asset_id;

-- Each fill's volume: its price times its size, in units of the market's quote asset. The product
-- is taken over the engine integers in Decimal256, then scaled.
CREATE VIEW trade_volumes
DEFINER = readmodel_owner SQL SECURITY DEFINER AS
SELECT t.ts AS ts, t.maker_user_id AS maker_user_id, t.taker_user_id AS taker_user_id,
       CAST(toDecimal256(t.price, 18) * t.size / (toUInt128(m.quote_scale_k) * a.native_scale)
            AS Decimal(38, 18)) AS volume
FROM trade_events AS t FINAL
INNER JOIN markets AS m FINAL ON m.market_id = t.market_id
INNER JOIN assets AS a FINAL ON a.asset_id = m.base_asset;

-- Privileges

-- Kafka Connect's sink inserts into the raw tables, and needs nothing more: that privilege also
-- lets it describe them.
GRANT INSERT ON balance_events TO readmodel_connect;
GRANT INSERT ON order_events TO readmodel_connect;
GRANT INSERT ON orderbook_events TO readmodel_connect;
GRANT INSERT ON cancel_events TO readmodel_connect;
GRANT INSERT ON trade_events TO readmodel_connect;
GRANT INSERT ON deposit_events TO readmodel_connect;
GRANT INSERT ON withdrawal_events TO readmodel_connect;

-- The gateway reads the views, so every read it makes goes through FINAL and is scaled.
GRANT SELECT ON balances_per_minute TO readmodel_gateway;
GRANT SELECT ON asset_prices_per_minute TO readmodel_gateway;
GRANT SELECT ON deposits TO readmodel_gateway;
GRANT SELECT ON withdrawals TO readmodel_gateway;
GRANT SELECT ON trade_volumes TO readmodel_gateway;

-- The views and materialized views run as their owner: it reads the tables and writes the
-- per-minute ones. A sink insert whose materialized view lacks a privilege fails as a whole.
GRANT SELECT ON balance_events TO readmodel_owner;
GRANT SELECT ON trade_events TO readmodel_owner;
GRANT SELECT ON deposit_events TO readmodel_owner;
GRANT SELECT ON withdrawal_events TO readmodel_owner;
GRANT SELECT ON assets TO readmodel_owner;
GRANT SELECT ON markets TO readmodel_owner;
GRANT SELECT, INSERT ON balances_last_per_min TO readmodel_owner;
GRANT SELECT, INSERT ON market_prices_last_per_min TO readmodel_owner;

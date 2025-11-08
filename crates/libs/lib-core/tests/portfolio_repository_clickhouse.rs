use std::str::FromStr;
use std::time::Duration;

use chrono::Utc;
use clickhouse::Client as ClickHouseClient;
use lib_core::repository::repository::{ClickHousePortfolioRepository, PortfolioRepository};
use lib_core::{PnlResolution, PortfolioMetrics};
use rust_decimal::Decimal;
use tokio::time::sleep;
use uuid::Uuid;

const DEFAULT_URL: &str = "http://localhost:8123";
const DEFAULT_USER: &str = "default";
const QUOTE_ASSET_ID: u16 = 1;
const BASE_ASSET_ID: u16 = 2;

fn market_id(base: u16, quote: u16) -> u32 {
    ((base as u32) << 16) | quote as u32
}

async fn ensure_clickhouse_available(client: &ClickHouseClient) -> Option<()> {
    client.query("SELECT 1").fetch_one::<u8>().await.ok()?;
    Some(())
}

async fn apply_schema(
    client: &ClickHouseClient,
    database: &str,
) -> Result<(), clickhouse::error::Error> {
    let statements = vec![
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.market_prices
            (
                ts        DateTime64(3) CODEC(Delta, ZSTD),
                market_id UInt32,
                price     Decimal(38, 12)
            )
            ENGINE = MergeTree
            PARTITION BY toDate(ts)
            ORDER BY (market_id, ts)
            TTL ts + INTERVAL 180 DAY
            SETTINGS index_granularity = 8192
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.balance_events
            (
                ts        DateTime64(3) CODEC(Delta, ZSTD),
                user_id   UInt64,
                asset_id  UInt16,
                available Decimal(38, 18),
                locked    Decimal(38, 18),
                total     Decimal(38, 18)
            )
            ENGINE = MergeTree
            PARTITION BY toDate(ts)
            ORDER BY (user_id, asset_id, ts)
            TTL ts + INTERVAL 365 DAY
            SETTINGS index_granularity = 8192
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.deposit_events
            (
                ts       DateTime64(3) CODEC(Delta, ZSTD),
                user_id  UInt64,
                asset_id UInt16,
                amount   Decimal(38, 18)
            )
            ENGINE = MergeTree
            PARTITION BY toDate(ts)
            ORDER BY (user_id, ts, asset_id)
            TTL ts + INTERVAL 365 DAY
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.withdrawal_events
            (
                ts       DateTime64(3) CODEC(Delta, ZSTD),
                user_id  UInt64,
                asset_id UInt16,
                amount   Decimal(38, 18)
            )
            ENGINE = MergeTree
            PARTITION BY toDate(ts)
            ORDER BY (user_id, ts, asset_id)
            TTL ts + INTERVAL 365 DAY
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.trade_events
            (
                ts             DateTime64(3) CODEC(Delta, ZSTD),
                market_id      UInt32,
                price          Decimal(38, 12),
                size_base      Decimal(38, 18),
                maker_user_id  UInt64,
                taker_user_id  UInt64,
                maker_order_id UInt64,
                taker_order_id UInt64
            )
            ENGINE = MergeTree
            PARTITION BY toDate(ts)
            ORDER BY (market_id, ts)
            TTL ts + INTERVAL 365 DAY
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.balances_last_per_min
            (
                minute   DateTime,
                user_id  UInt64,
                asset_id UInt16,
                total    Decimal(38, 18),
                ver      DateTime64(3)
            )
            ENGINE = ReplacingMergeTree(ver)
            ORDER BY (user_id, asset_id, minute)
            TTL minute + INTERVAL 730 DAY
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS {db}.mv_balances_last_per_min
            TO {db}.balances_last_per_min
            AS
            SELECT
                toStartOfMinute(ts) AS minute,
                user_id,
                asset_id,
                argMax(total, ts) AS total,
                max(ts) AS ver
            FROM {db}.balance_events
            GROUP BY minute, user_id, asset_id
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.market_prices_last_per_min
            (
                minute    DateTime,
                market_id UInt32,
                price     Decimal(38, 12),
                ver       DateTime64(3)
            )
            ENGINE = ReplacingMergeTree(ver)
            ORDER BY (market_id, minute)
            TTL minute + INTERVAL 365 DAY
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS {db}.mv_market_prices_last_per_min
            TO {db}.market_prices_last_per_min
            AS
            SELECT
                toStartOfMinute(ts) AS minute,
                market_id,
                argMax(price, ts) AS price,
                max(ts) AS ver
            FROM {db}.market_prices
            GROUP BY minute, market_id
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.equity_per_minute
            (
                minute       DateTime,
                user_id      UInt64,
                equity_quote Decimal(38, 12),
                ver          DateTime64(3)
            )
            ENGINE = ReplacingMergeTree(ver)
            ORDER BY (user_id, minute)
            TTL minute + INTERVAL 365 DAY
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.markets
            (
                market_id      UInt32,
                base_asset_id  UInt16,
                quote_asset_id UInt16,
                is_active      UInt8 DEFAULT 1
            )
            ENGINE = MergeTree
            ORDER BY (market_id)
            SETTINGS index_granularity = 8192
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.equity_quote_config
            (
                quote_asset_id UInt16,
                updated_at     DateTime64(3)
            )
            ENGINE = TinyLog
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.asset_prices_last_per_min
            (
                minute         DateTime,
                asset_id       UInt16,
                quote_asset_id UInt16,
                price          Decimal(38, 12),
                ver            DateTime64(3)
            )
            ENGINE = ReplacingMergeTree(ver)
            ORDER BY (asset_id, quote_asset_id, minute)
            TTL minute + INTERVAL 365 DAY
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS {db}.mv_asset_prices_last_per_min
            TO {db}.asset_prices_last_per_min
            AS
            WITH cfg AS (SELECT quote_asset_id FROM {db}.equity_quote_config LIMIT 1)
            SELECT
                mp.minute,
                m.base_asset_id AS asset_id,
                m.quote_asset_id AS quote_asset_id,
                mp.price AS price,
                toDateTime64(mp.minute, 3) AS ver
            FROM {db}.market_prices_last_per_min mp
            INNER JOIN {db}.markets m ON m.market_id = mp.market_id AND m.is_active = 1
            INNER JOIN cfg ON m.quote_asset_id = cfg.quote_asset_id
            UNION ALL
            SELECT
                mp.minute,
                m.quote_asset_id AS asset_id,
                m.base_asset_id AS quote_asset_id,
                CAST(1 AS Decimal(38, 12)) / nullIf(mp.price, CAST(0 AS Decimal(38, 12))) AS price,
                toDateTime64(mp.minute, 3) AS ver
            FROM {db}.market_prices_last_per_min mp
            INNER JOIN {db}.markets m ON m.market_id = mp.market_id AND m.is_active = 1
            INNER JOIN (SELECT quote_asset_id FROM {db}.equity_quote_config LIMIT 1) cfg
                ON m.base_asset_id = cfg.quote_asset_id
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS {db}.mv_equity_per_minute
            TO {db}.equity_per_minute
            AS
            WITH cfg AS (SELECT quote_asset_id FROM {db}.equity_quote_config LIMIT 1)
            SELECT
                minute,
                user_id,
                sum(total * coalesce(price, CAST(0 AS Decimal(38, 12)))) AS equity_quote,
                max(toDateTime64(minute, 3)) AS ver
            FROM (
                SELECT
                    b.minute AS minute,
                    b.user_id AS user_id,
                    b.asset_id,
                    b.total,
                    if(
                        b.asset_id = cfg.quote_asset_id,
                        CAST(1 AS Decimal(38, 12)),
                        ap.price
                    ) AS price
                FROM {db}.balances_last_per_min b
                CROSS JOIN cfg
                LEFT ASOF JOIN {db}.asset_prices_last_per_min ap
                    ON ap.asset_id = b.asset_id
                   AND ap.quote_asset_id = cfg.quote_asset_id
                   AND ap.minute <= b.minute
            ) AS joined
            GROUP BY minute, user_id
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE TABLE IF NOT EXISTS {db}.user_cashflow_per_minute
            (
                minute         DateTime,
                user_id        UInt64,
                cashflow_quote Decimal(38, 12)
            )
            ENGINE = SummingMergeTree
            ORDER BY (user_id, minute)
            TTL minute + INTERVAL 365 DAY
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS {db}.mv_deposit_cashflow_per_minute
            TO {db}.user_cashflow_per_minute
            AS
            WITH cfg AS (
                SELECT quote_asset_id FROM {db}.equity_quote_config LIMIT 1
            )
            SELECT
                toStartOfMinute(d.ts) AS minute,
                d.user_id,
                d.amount * if(
                    d.asset_id = cfg.quote_asset_id,
                    CAST(1 AS Decimal(38, 12)),
                    coalesce(ap.price, CAST(0 AS Decimal(38, 12)))
                ) AS cashflow_quote
            FROM {db}.deposit_events d
            CROSS JOIN cfg
            LEFT ASOF JOIN {db}.asset_prices_last_per_min ap
                ON ap.asset_id = d.asset_id
               AND ap.quote_asset_id = cfg.quote_asset_id
               AND ap.minute <= toStartOfMinute(d.ts)
            "#,
            db = database
        ),
        format!(
            r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS {db}.mv_withdrawal_cashflow_per_minute
            TO {db}.user_cashflow_per_minute
            AS
            WITH cfg AS (
                SELECT quote_asset_id FROM {db}.equity_quote_config LIMIT 1
            )
            SELECT
                toStartOfMinute(w.ts) AS minute,
                w.user_id,
                (w.amount * if(
                    w.asset_id = cfg.quote_asset_id,
                    CAST(1 AS Decimal(38, 12)),
                    coalesce(ap.price, CAST(0 AS Decimal(38, 12)))
                )) * (-1) AS cashflow_quote
            FROM {db}.withdrawal_events w
            CROSS JOIN cfg
            LEFT ASOF JOIN {db}.asset_prices_last_per_min ap
                ON ap.asset_id = w.asset_id
               AND ap.quote_asset_id = cfg.quote_asset_id
               AND ap.minute <= toStartOfMinute(w.ts)
            "#,
            db = database
        ),
    ];

    for statement in statements {
        client.query(&statement).execute().await?;
    }

    Ok(())
}

async fn seed_data(
    client: &ClickHouseClient,
    database: &str,
) -> Result<(), clickhouse::error::Error> {
    const DAY: i64 = 24 * 60 * 60;
    const HOUR: i64 = 60 * 60;

    let now_ts = Utc::now().timestamp();
    let aligned_now = now_ts - (now_ts % 60);
    let user_id = 42_u64;
    let mkt_id = market_id(BASE_ASSET_ID, QUOTE_ASSET_ID);

    let baseline = aligned_now - 7 * DAY;
    let three_days_ago = aligned_now - 3 * DAY;
    let one_day_ago = aligned_now - DAY;
    let now_minus_hour = aligned_now - HOUR;
    let now_minus_two_hours = aligned_now - 2 * HOUR;

    let statements = vec![
        format!(
            r#"
            INSERT INTO {db}.equity_quote_config (quote_asset_id, updated_at) VALUES
                ({quote_asset}, toDateTime64({now}, 3))
            "#,
            db = database,
            quote_asset = QUOTE_ASSET_ID,
            now = aligned_now
        ),
        format!(
            r#"
            INSERT INTO {db}.markets (market_id, base_asset_id, quote_asset_id, is_active) VALUES
                ({market_id}, {base_asset}, {quote_asset}, 1)
            "#,
            db = database,
            market_id = mkt_id,
            base_asset = BASE_ASSET_ID,
            quote_asset = QUOTE_ASSET_ID
        ),
        format!(
            r#"
            INSERT INTO {db}.market_prices (ts, market_id, price) VALUES
                (toDateTime64({two_hours}, 3), {market_id}, toDecimal128(50000, 12)),
                (toDateTime64({one_hour}, 3), {market_id}, toDecimal128(48000, 12)),
                (toDateTime64({now}, 3), {market_id}, toDecimal128(47000, 12))
            "#,
            db = database,
            market_id = mkt_id,
            two_hours = now_minus_two_hours,
            one_hour = now_minus_hour,
            now = aligned_now
        ),
        format!(
            r#"
            INSERT INTO {db}.balance_events (ts, user_id, asset_id, available, locked, total) VALUES
                (toDateTime64({baseline}, 3), {user}, {asset}, toDecimal128(1000, 18), toDecimal128(0, 18), toDecimal128(1000, 18)),
                (toDateTime64({three_days}, 3), {user}, {asset}, toDecimal128(1200, 18), toDecimal128(0, 18), toDecimal128(1200, 18)),
                (toDateTime64({one_day}, 3), {user}, {asset}, toDecimal128(1400, 18), toDecimal128(0, 18), toDecimal128(1400, 18)),
                (toDateTime64({now}, 3), {user}, {asset}, toDecimal128(1600, 18), toDecimal128(0, 18), toDecimal128(1600, 18))
            "#,
            db = database,
            user = user_id,
            asset = QUOTE_ASSET_ID,
            baseline = baseline,
            three_days = three_days_ago,
            one_day = one_day_ago,
            now = aligned_now
        ),
        format!(
            r#"
            INSERT INTO {db}.deposit_events (ts, user_id, asset_id, amount) VALUES
                (toDateTime64({three_days}, 3), {user}, {asset}, toDecimal128(200, 18))
            "#,
            db = database,
            user = user_id,
            asset = QUOTE_ASSET_ID,
            three_days = three_days_ago
        ),
        format!(
            r#"
            INSERT INTO {db}.withdrawal_events (ts, user_id, asset_id, amount) VALUES
                (toDateTime64({one_hour}, 3), {user}, {asset}, toDecimal128(50, 18))
            "#,
            db = database,
            user = user_id,
            asset = QUOTE_ASSET_ID,
            one_hour = now_minus_hour
        ),
        format!(
            r#"
            INSERT INTO {db}.trade_events (ts, market_id, price, size_base, maker_user_id, taker_user_id, maker_order_id, taker_order_id) VALUES
                (toDateTime64({one_hour}, 3), {market_id}, toDecimal128(100, 12), toDecimal128(2, 18), {user}, 999, 1, 2),
                (toDateTime64({two_hours}, 3), {market_id}, toDecimal128(50, 12), toDecimal128(1, 18), 999, {user}, 3, 4)
            "#,
            db = database,
            market_id = mkt_id,
            user = user_id,
            one_hour = now_minus_hour,
            two_hours = now_minus_two_hours
        ),
    ];

    for statement in statements {
        client.query(&statement).execute().await?;
    }

    // Give materialized views a moment to process.
    client.query("SYSTEM FLUSH LOGS").execute().await?;
    sleep(Duration::from_millis(200)).await;

    Ok(())
}

fn assert_metrics(metrics: &PortfolioMetrics) {
    let expected_balance = Decimal::from_str("1600").unwrap();
    let expected_balance_24h_ago = Decimal::from_str("1400").unwrap();
    let expected_volume_24h = Decimal::from_str("250").unwrap();
    let expected_pnl_7d = Decimal::from_str("450").unwrap();

    assert_eq!(metrics.balance, expected_balance);
    assert_eq!(metrics.balance_24h_ago, expected_balance_24h_ago);
    assert_eq!(metrics.volume_24h, expected_volume_24h);
    assert_eq!(metrics.pnl_7d, expected_pnl_7d);
    assert!(
        metrics
            .pnl_series
            .iter()
            .any(|point| point.pnl == expected_pnl_7d),
        "expected final pnl value in series"
    );
}

#[tokio::test]
async fn clickhouse_portfolio_repository_integration() {
    let url = std::env::var("TEST_CLICKHOUSE_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    let user = std::env::var("TEST_CLICKHOUSE_USER").unwrap_or_else(|_| DEFAULT_USER.to_string());
    let password = std::env::var("TEST_CLICKHOUSE_PASSWORD").unwrap_or_default();

    let base_client = ClickHouseClient::default()
        .with_url(url.clone())
        .with_user(user.clone())
        .with_password(password.clone());

    if ensure_clickhouse_available(&base_client).await.is_none() {
        eprintln!("Skipping ClickHouse integration test: unable to connect to {}", url);
        return;
    }

    let database_name = format!("test_portfolio_repo_{}", Uuid::new_v4().simple());
    let create_db_stmt = format!("CREATE DATABASE IF NOT EXISTS {}", database_name);
    if let Err(err) = base_client.query(&create_db_stmt).execute().await {
        panic!("failed to create test database {database_name}: {err}");
    }

    let db_client = ClickHouseClient::default()
        .with_url(url.clone())
        .with_user(user.clone())
        .with_password(password.clone())
        .with_database(&database_name);

    apply_schema(&db_client, &database_name)
        .await
        .expect("failed to apply schema");
    seed_data(&db_client, &database_name)
        .await
        .expect("failed to seed test data");

    let repo = ClickHousePortfolioRepository::new_with_schema(db_client, &database_name);
    let user_id = 42;
    let now_ts = Utc::now().timestamp();

    let metrics = repo
        .get_portfolio(user_id, now_ts, PnlResolution::Week)
        .await
        .expect("portfolio query failed");

    assert_metrics(&metrics);

    let drop_db_stmt = format!("DROP DATABASE IF EXISTS {}", database_name);
    if let Err(err) = base_client.query(&drop_db_stmt).execute().await {
        eprintln!(
            "warning: failed to drop ClickHouse test database {database}: {err}",
            database = database_name
        );
    }
}


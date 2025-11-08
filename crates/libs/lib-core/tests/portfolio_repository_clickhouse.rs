use std::str::FromStr;
use std::time::Duration;

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, Utc};
use clickhouse::Client as ClickHouseClient;
use lib_core::repository::repository::{ClickHousePortfolioRepository, PortfolioRepository};
use lib_core::{PnlResolution, PortfolioMetrics};
use rust_decimal::Decimal;
use tokio::time::sleep;
use uuid::Uuid;
use tracing::info;
use std::sync::Once;

const DEFAULT_URL: &str = "http://localhost:8123";
const DEFAULT_USER: &str = "default";
const QUOTE_ASSET_ID: u16 = 1;
const BASE_ASSET_ID: u16 = 2;

fn market_id(base: u16, quote: u16) -> u32 {
    ((base as u32) << 16) | quote as u32
}

#[derive(Debug, Clone)]
struct SeedContext {
    now: i64,
    equity_points: Vec<(i64, Decimal)>,
    cashflow_points: Vec<(i64, Decimal)>,
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
) -> Result<SeedContext, clickhouse::error::Error> {
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

    Ok(SeedContext {
        now: aligned_now,
        equity_points: vec![
            (baseline, Decimal::from_str("1000").unwrap()),
            (three_days_ago, Decimal::from_str("1200").unwrap()),
            (one_day_ago, Decimal::from_str("1400").unwrap()),
            (aligned_now, Decimal::from_str("1600").unwrap()),
        ],
        cashflow_points: vec![
            (three_days_ago, Decimal::from_str("200").unwrap()),
            (now_minus_hour, Decimal::from_str("-50").unwrap()),
        ],
    })
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

static INIT_TRACING: Once = Once::new();

fn init_tracing() {
    INIT_TRACING.call_once(|| {
        if tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init()
            .is_err()
        {
            // ignore double init during concurrent tests
        }
    });
}

fn window_start(now: i64, resolution: PnlResolution) -> i64 {
    let span = match resolution {
        PnlResolution::Day => 24 * 60 * 60,
        PnlResolution::Week => 7 * 24 * 60 * 60,
        PnlResolution::Month => 30 * 24 * 60 * 60,
        PnlResolution::Year => 365 * 24 * 60 * 60,
    };
    now - span
}

fn bucket_floor(resolution: PnlResolution, ts: i64) -> i64 {
    use chrono::Timelike;

    let Some(dt) = chrono::DateTime::<Utc>::from_timestamp(ts, 0) else {
        return ts;
    };
    let date = dt.date_naive();

    match resolution {
        PnlResolution::Day => dt
            .with_minute(0)
            .and_then(|d| d.with_second(0))
            .and_then(|d| d.with_nanosecond(0))
            .unwrap_or(dt)
            .timestamp(),
        PnlResolution::Week => {
            let weekday = date.weekday().num_days_from_monday() as i64;
            let start_date = date - ChronoDuration::days(weekday);
            start_date
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        }
        PnlResolution::Month => {
            let floored = NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date);
            floored
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        }
        PnlResolution::Year => {
            let floored = NaiveDate::from_ymd_opt(date.year(), 1, 1).unwrap_or(date);
            floored
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        }
    }
}

fn bucket_key(resolution: PnlResolution, ts: i64) -> i64 {
    use chrono::Timelike;

    let Some(dt) = chrono::DateTime::<Utc>::from_timestamp(ts, 0) else {
        return ts;
    };
    match resolution {
        PnlResolution::Day => dt
            .with_minute(0)
            .and_then(|d| d.with_second(0))
            .and_then(|d| d.with_nanosecond(0))
            .unwrap_or(dt)
            .timestamp(),
        PnlResolution::Week | PnlResolution::Month => dt
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp(),
        PnlResolution::Year => {
            let date = dt.date_naive();
            NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
                .unwrap_or(date)
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        }
    }
}

fn expected_series(
    resolution: PnlResolution,
    now: i64,
    ctx: &SeedContext,
) -> Vec<(i64, Decimal)> {
    use std::collections::{BTreeMap, BTreeSet};

    let start = window_start(now, resolution);

    let baseline = ctx
        .equity_points
        .iter()
        .filter(|(ts, _)| *ts <= start)
        .max_by_key(|(ts, _)| *ts)
        .map(|(_, value)| *value)
        .unwrap_or(Decimal::ZERO);

    let mut equity_map = BTreeMap::new();
    for &(ts, value) in &ctx.equity_points {
        if ts >= start && ts <= now {
            equity_map.insert(bucket_key(resolution, ts), value);
        }
    }

    let mut cashflow_map = BTreeMap::new();
    for &(ts, value) in &ctx.cashflow_points {
        if ts > start && ts <= now {
            let bucket = bucket_key(resolution, ts);
            cashflow_map
                .entry(bucket)
                .and_modify(|sum| *sum += value)
                .or_insert(value);
        }
    }

    let mut cumulative_cashflow = Decimal::ZERO;
    let mut cashflow_cumulative_map = BTreeMap::new();
    for (ts, value) in &cashflow_map {
        cumulative_cashflow += *value;
        cashflow_cumulative_map.insert(*ts, cumulative_cashflow);
    }

    let mut bucket_keys = BTreeSet::new();
    bucket_keys.extend(equity_map.keys().copied());
    bucket_keys.extend(cashflow_map.keys().copied());
    bucket_keys.insert(bucket_floor(resolution, start));
    bucket_keys.insert(bucket_floor(resolution, now));
    bucket_keys.insert(start);
    bucket_keys.insert(now);

    let mut last_equity = baseline;
    let mut result = Vec::with_capacity(bucket_keys.len());

    for ts in bucket_keys {
        if let Some(value) = equity_map.get(&ts) {
            last_equity = *value;
        }
        let cashflow_up_to_ts = cashflow_cumulative_map
            .range(..=ts)
            .next_back()
            .map(|(_, value)| *value)
            .unwrap_or(Decimal::ZERO);
        let pnl = last_equity - baseline - cashflow_up_to_ts;
        result.push((ts, pnl));
    }

    result.sort_by_key(|(ts, _)| *ts);
    result
}

#[tokio::test]
async fn clickhouse_portfolio_repository_integration() {
    init_tracing();

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
    let seed_ctx = seed_data(&db_client, &database_name)
        .await
        .expect("failed to seed test data");

    let repo = ClickHousePortfolioRepository::new_with_schema(db_client, &database_name);
    let user_id = 42;
    let now_ts = seed_ctx.now;

    // Weekly summary assertions
    let metrics_week = repo
        .get_portfolio(user_id, now_ts, PnlResolution::Week)
        .await
        .expect("portfolio query failed");

    info!(?metrics_week, "weekly portfolio metrics");
    assert_metrics(&metrics_week);

    let expected_day_pnl = Decimal::from_str("250").unwrap();
    let expected_week_series = expected_series(PnlResolution::Week, now_ts, &seed_ctx);
    let actual_week_series: Vec<(i64, Decimal)> = metrics_week
        .pnl_series
        .iter()
        .map(|point| (point.timestamp, point.pnl))
        .collect();
    assert_eq!(
        actual_week_series,
        expected_week_series,
        "weekly pnl series must match expected accounting pnl"
    );
    let expected_month_pnl = Decimal::from_str("1450").unwrap();

    // Daily series should produce hourly buckets (> 1 point) and align with accounting PnL
    let metrics_day = repo
        .get_portfolio(user_id, now_ts, PnlResolution::Day)
        .await
        .expect("day portfolio query failed");
    let expected_day_series = expected_series(PnlResolution::Day, now_ts, &seed_ctx);
    info!(points = metrics_day.pnl_series.len(), "daily pnl series points");
    assert!(
        metrics_day.pnl_series.len() >= 2,
        "expect at least two hourly pnl points for 1d resolution"
    );
    let actual_day_series: Vec<(i64, Decimal)> = metrics_day
        .pnl_series
        .iter()
        .map(|point| (point.timestamp, point.pnl))
        .collect();
    assert_eq!(
        actual_day_series,
        expected_day_series,
        "daily pnl series must match expected accounting pnl"
    );
    assert_eq!(
        metrics_day.pnl_series.last().unwrap().pnl,
        expected_day_pnl,
        "daily last point should match expected daily accounting pnl"
    );

    // Month resolution should include baseline and final points
    let metrics_month = repo
        .get_portfolio(user_id, now_ts, PnlResolution::Month)
        .await
        .expect("month portfolio query failed");
    let expected_month_series = expected_series(PnlResolution::Month, now_ts, &seed_ctx);
    info!(
        baseline_pnl = ?metrics_month.pnl_series.first().map(|p| p.pnl),
        final_pnl = ?metrics_month.pnl_series.last().map(|p| p.pnl),
        "monthly pnl endpoints"
    );
    let actual_month_series: Vec<(i64, Decimal)> = metrics_month
        .pnl_series
        .iter()
        .map(|point| (point.timestamp, point.pnl))
        .collect();
    assert_eq!(
        actual_month_series,
        expected_month_series,
        "monthly pnl series must match expected accounting pnl"
    );
    assert_eq!(
        metrics_month.pnl_series.last().unwrap().pnl,
        expected_month_pnl,
        "monthly final point should equal expected monthly accounting pnl"
    );
    assert_eq!(
        metrics_month.pnl_series.first().unwrap().pnl,
        Decimal::ZERO,
        "first monthly point should be zero baseline pnl"
    );

    let drop_db_stmt = format!("DROP DATABASE IF EXISTS {}", database_name);
    if let Err(err) = base_client.query(&drop_db_stmt).execute().await {
        eprintln!(
            "warning: failed to drop ClickHouse test database {database}: {err}",
            database = database_name
        );
    }
}

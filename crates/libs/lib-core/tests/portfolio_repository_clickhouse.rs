//! `ClickHousePortfolioRepository` against the read model's ClickHouse schema, as a deployment
//! has it: `readmodel/migrations/clickhouse` and the development seeds applied to a fresh
//! database, which the repository reads as a login that holds only the gateway's role.
//!
//! Needs ClickHouse at `TEST_CLICKHOUSE_URL` (default `http://localhost:8123`) as a user that may
//! create databases, users and roles (`TEST_CLICKHOUSE_USER` and `TEST_CLICKHOUSE_PASSWORD`,
//! default `default` and empty); the scaling test also needs Postgres at `DATABASE_URL`. Without
//! them the tests are skipped, except in CI, where they fail.

use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use clickhouse::Client;
use lib_core::cache::scaling_cache::ScalingCache;
use lib_core::repository::repository::{ClickHousePortfolioRepository, PortfolioRepository};
use lib_core::PnlResolution;
use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use tokio::sync::OnceCell;

/// The tests' ClickHouse database, and the login that reads it with the gateway's role.
const DATABASE: &str = "portfolio_repository_tests";
const GATEWAY_USER: &str = "portfolio_repository_tests_gateway";
const GATEWAY_PASSWORD: &str = "test-only";

const HOUR: i64 = 60 * 60;
const DAY: i64 = 24 * HOUR;
/// USD: the quote asset the repository values portfolios in, with native scale 10^6.
const USD: u64 = 1;
const USD_SCALE: u64 = 1_000_000;
/// BTC/USD (quote asset 1 << 16 | base asset 2): BTC's native scale is 10^8, quote_scale_k 1.
const BTC_USD: u64 = 65538;
const BTC_SCALE: u64 = 100_000_000;

fn readmodel() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../readmodel"))
}

/// Skips the calling test when `service` does not answer at `url`, except in CI, where it fails.
fn unavailable(service: &str, url: &str) {
    assert!(
        std::env::var_os("CI").is_none(),
        "{service} does not answer at {url}"
    );
    eprintln!("skipping: {service} does not answer at {url}");
}

/// The statements of a migration or seed file, cut at each semicolon as golang-migrate's
/// multi-statement mode cuts them; a piece holding only comments is not a statement.
fn statements(sql: &str) -> impl Iterator<Item = &str> {
    sql.split(';').filter(|piece| {
        piece
            .lines()
            .map(str::trim)
            .any(|line| !line.is_empty() && !line.starts_with("--"))
    })
}

struct ClickHouse {
    /// The test's own user, in `DATABASE`: it writes the events, as Connect's sink would.
    admin: Client,
    /// The gateway's login.
    gateway: Client,
}

impl ClickHouse {
    async fn execute(&self, sql: &str) {
        if let Err(error) = self.admin.query(sql).execute().await {
            panic!("{sql}: {error}");
        }
    }
}

/// The tests' ClickHouse database, which the first caller creates; None, so that the test is
/// skipped, when no ClickHouse server answers.
async fn clickhouse() -> Option<ClickHouse> {
    static CREATED: OnceCell<()> = OnceCell::const_new();
    let url =
        std::env::var("TEST_CLICKHOUSE_URL").unwrap_or_else(|_| "http://localhost:8123".into());
    let user = std::env::var("TEST_CLICKHOUSE_USER").unwrap_or_else(|_| "default".into());
    let password = std::env::var("TEST_CLICKHOUSE_PASSWORD").unwrap_or_default();
    let server = Client::default()
        .with_url(&url)
        .with_user(&user)
        .with_password(&password);
    if server.query("SELECT 1").fetch_one::<u8>().await.is_err() {
        unavailable("ClickHouse", &url);
        return None;
    }
    CREATED.get_or_init(|| create_database(&server)).await;
    Some(ClickHouse {
        admin: server.with_database(DATABASE),
        gateway: Client::default()
            .with_url(&url)
            .with_user(GATEWAY_USER)
            .with_password(GATEWAY_PASSWORD)
            .with_database(DATABASE),
    })
}

/// Creates `DATABASE` afresh from the migrations and the development seeds, the views' owner and
/// the gateway's login, as a deployment does.
async fn create_database(server: &Client) {
    let run = |client: &Client, sql: String| {
        let query = client.query(&sql);
        async move {
            if let Err(error) = query.execute().await {
                panic!("{sql}: {error}");
            }
        }
    };
    for sql in [
        format!("DROP DATABASE IF EXISTS {DATABASE} SYNC"),
        format!("CREATE DATABASE {DATABASE}"),
        format!("DROP USER IF EXISTS {GATEWAY_USER}"),
        format!("CREATE USER {GATEWAY_USER} IDENTIFIED BY '{GATEWAY_PASSWORD}'"),
        "CREATE USER IF NOT EXISTS readmodel_owner IDENTIFIED BY 'test-only'".to_string(),
    ] {
        run(server, sql).await;
    }
    let database = server.clone().with_database(DATABASE);
    let mut scripts: Vec<_> = fs::read_dir(readmodel().join("migrations/clickhouse"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".up.sql"))
        .collect();
    scripts.sort();
    scripts.push(readmodel().join("seeds/development.clickhouse.sql"));
    for script in scripts {
        for statement in statements(&fs::read_to_string(&script).unwrap()) {
            run(&database, statement.to_string()).await;
        }
    }
    run(server, format!("GRANT readmodel_gateway TO {GATEWAY_USER}")).await;
}

/// The Postgres database ScalingCache reads, created afresh by the first caller from the
/// migrations and the development seeds: `<DATABASE_URL's database>_portfolio_repository_tests`.
/// None, so that the test is skipped, when no Postgres server answers.
async fn postgres() -> Option<DatabaseConnection> {
    static URL: OnceCell<String> = OnceCell::const_new();
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres123@localhost:5432/trading_db".into());
    let Ok(Ok(server)) =
        tokio::time::timeout(Duration::from_secs(5), Database::connect(&url)).await
    else {
        unavailable("Postgres", &url);
        return None;
    };
    let database = URL
        .get_or_init(|| async {
            let mut database = url::Url::parse(&url).expect("DATABASE_URL is a URL");
            let name = format!(
                "{}_portfolio_repository_tests",
                database.path().trim_start_matches('/')
            );
            for sql in [
                format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#),
                format!(r#"CREATE DATABASE "{name}""#),
            ] {
                server.execute_unprepared(&sql).await.unwrap();
            }
            database.set_path(&name);
            let db = Database::connect(database.as_str()).await.unwrap();
            let mut scripts: Vec<_> = fs::read_dir(readmodel().join("migrations/postgres"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.to_string_lossy().ends_with(".up.sql"))
                .collect();
            scripts.sort();
            scripts.push(readmodel().join("seeds/development.sql"));
            for script in scripts {
                if let Err(error) = db
                    .execute_unprepared(&fs::read_to_string(&script).unwrap())
                    .await
                {
                    panic!("{}: {error}", script.display());
                }
            }
            database.into()
        })
        .await;
    Some(Database::connect(database.as_str()).await.unwrap())
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

/// vex-core's events of one user, written twice each, as a re-publication after a restart
/// writes them: every metric counts each event once.
#[tokio::test]
async fn portfolio_counts_each_event_once() {
    let Some(ch) = clickhouse().await else {
        return;
    };
    let user = 42;
    let now = chrono::Utc::now().timestamp() / 60 * 60;
    let at = |seconds: i64| (seconds as u64) * 1_000_000_000;
    // USD balances, deposits and withdrawals: the quote asset has price 1, so equity is the
    // balance. A deposit falls between the two withdrawals of the last day, so the cash flows
    // come in time order only if the whole UNION ALL of deposits and withdrawals is sorted, not
    // its withdrawals alone, which then come together.
    let balances = [
        (now - 7 * DAY, 1000),
        (now - 3 * DAY, 1200),
        (now - DAY, 1400),
        (now - 8 * HOUR, 1500),
        (now, 1600),
    ];
    let deposits = [(now - 3 * DAY, 200), (now - 11 * HOUR / 2, 100)];
    let withdrawals = [(now - 21 * HOUR / 2, 50), (now - 5 * HOUR / 2, 20)];
    for _copy in 0..2 {
        for (ts, total) in balances {
            let total = total * USD_SCALE;
            ch.execute(&format!(
                "INSERT INTO balance_events (user_id, asset_id, available, locked, total, event_ts) \
                 VALUES ({user}, {USD}, {total}, 0, {total}, {})",
                at(ts)
            ))
            .await;
        }
        for (ts, amount) in deposits {
            ch.execute(&format!(
                "INSERT INTO deposit_events (user_id, asset_id, amount, event_ts) \
                 VALUES ({user}, {USD}, {}, {})",
                amount * USD_SCALE,
                at(ts)
            ))
            .await;
        }
        for (ts, amount) in withdrawals {
            ch.execute(&format!(
                "INSERT INTO withdrawal_events (user_id, asset_id, amount, event_ts) \
                 VALUES ({user}, {USD}, {}, {})",
                amount * USD_SCALE,
                at(ts)
            ))
            .await;
        }
        // 2 BTC at 100 USD as the maker, 1 BTC at 50 USD as the taker: a volume of 250 USD.
        ch.execute(&format!(
            "INSERT INTO trade_events (maker_user_id, taker_user_id, market_id, price, size, \
             maker_order_id, taker_order_id, taker_side, event_ts) VALUES \
             ({user}, 999, {BTC_USD}, 100, {}, 1, 2, 'SIDE_BID', {}), \
             (999, {user}, {BTC_USD}, 50, {}, 3, 4, 'SIDE_ASK', {})",
            2 * BTC_SCALE,
            at(now - HOUR),
            BTC_SCALE,
            at(now - 2 * HOUR)
        ))
        .await;
    }
    let repository = ClickHousePortfolioRepository::new_with_schema(ch.gateway, DATABASE);

    let week = repository
        .get_portfolio(user, now, PnlResolution::Week)
        .await
        .unwrap();
    assert_eq!(week.balance, decimal("1600"));
    assert_eq!(week.balance_24h_ago, decimal("1400"));
    assert_eq!(week.volume_24h, decimal("250"));
    // (1600 - 1000) - (200 + 100 - 50 - 20)
    assert_eq!(week.pnl_7d, decimal("370"));
    assert_eq!(week.max_drawdown_1m, Decimal::ZERO);

    // Hourly from now - 24 h: equity 1400, 1500 from now - 8 h and 1600 at now, against the
    // window's start, less the cash flow since then: -50 from now - 10.5 h, +100 from now - 5.5 h
    // and -20 from now - 2.5 h.
    let day = repository
        .get_portfolio(user, now, PnlResolution::Day)
        .await
        .unwrap();
    let expected: Vec<_> = (0..=24)
        .map(|hour| {
            let pnl = match hour {
                0..=13 => 0,
                14..=15 => 50,
                16..=18 => 150,
                19..=21 => 50,
                22..=23 => 70,
                _ => 170,
            };
            (now - DAY + hour * HOUR, Decimal::from(pnl))
        })
        .collect();
    let series: Vec<_> = day
        .pnl_series
        .iter()
        .map(|point| (point.timestamp, point.pnl))
        .collect();
    assert_eq!(series, expected);

    let chart = repository
        .get_balance_chart(user, now, PnlResolution::Day)
        .await
        .unwrap();
    let expected: Vec<_> = (0..=24)
        .map(|hour| {
            let balance = match hour {
                0..=15 => 1400,
                16..=23 => 1500,
                _ => 1600,
            };
            (now - DAY + hour * HOUR, Decimal::from(balance))
        })
        .collect();
    let chart: Vec<_> = chart
        .iter()
        .map(|point| (point.timestamp, point.balance))
        .collect();
    assert_eq!(chart, expected);
}

/// For every seeded asset and market, the views scale engine integers as ScalingCache does with
/// the Postgres rows the same seeds wrote.
#[tokio::test]
async fn views_scale_as_scaling_cache_does() {
    let Some(ch) = clickhouse().await else {
        return;
    };
    let Some(postgres) = postgres().await else {
        return;
    };
    let assets: Vec<i16> = postgres
        .query_all(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT asset_id FROM assets ORDER BY asset_id",
        ))
        .await
        .unwrap()
        .iter()
        .map(|row| row.try_get("", "asset_id").unwrap())
        .collect();
    let markets: Vec<(i32, i16, i16)> = postgres
        .query_all(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT market_id, base_asset, quote_asset FROM markets ORDER BY market_id",
        ))
        .await
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.try_get("", "market_id").unwrap(),
                row.try_get("", "base_asset").unwrap(),
                row.try_get("", "quote_asset").unwrap(),
            )
        })
        .collect();
    assert!(!assets.is_empty() && !markets.is_empty());
    let cache = ScalingCache::new(Arc::new(postgres));
    cache.load_all().await.unwrap();

    // 2025-01-01T00:00:00Z, a minute of its own. The integers have 18 significant digits, so
    // every scaled value is exact in Decimal(38, 18) and in rust_decimal.
    let minute = 1_735_689_600_i64;
    let event_ts = minute as u64 * 1_000_000_000;
    let amount: u64 = 123_456_789_012_345_678;
    let (price, size): (u64, u64) = (5_000_123, 1_234_567_890);
    let asset_user = |asset: i16| 7_000 + asset as u64;
    let market_user = |market: usize| 8_000 + market as u64;
    for &asset in &assets {
        let user = asset_user(asset);
        for table in ["deposit_events", "withdrawal_events"] {
            ch.execute(&format!(
                "INSERT INTO {table} (user_id, asset_id, amount, event_ts) \
                 VALUES ({user}, {asset}, {amount}, {event_ts})"
            ))
            .await;
        }
        ch.execute(&format!(
            "INSERT INTO balance_events (user_id, asset_id, available, locked, total, event_ts) \
             VALUES ({user}, {asset}, {amount}, 0, {amount}, {event_ts})"
        ))
        .await;
    }
    for (index, &(market, _, _)) in markets.iter().enumerate() {
        let user = market_user(index);
        ch.execute(&format!(
            "INSERT INTO trade_events (maker_user_id, taker_user_id, market_id, price, size, \
             maker_order_id, taker_order_id, taker_side, event_ts) VALUES \
             ({user}, {user}, {market}, {price}, {size}, {user}, {user}, 'SIDE_BID', {event_ts})"
        ))
        .await;
    }

    let read = |sql: String, user: u64| {
        let query = ch.gateway.query(&sql).bind(user);
        async move {
            let rows: Vec<String> = query.fetch_all().await.unwrap();
            assert_eq!(rows.len(), 1, "{sql} for {user}: {rows:?}");
            decimal(&rows[0])
        }
    };
    for &asset in &assets {
        let expected = cache.scale_balance(asset as u16, Decimal::from(amount));
        let user = asset_user(asset);
        for (view, column) in [
            ("balances_per_minute", "total"),
            ("deposits", "amount"),
            ("withdrawals", "amount"),
        ] {
            let sql = format!("SELECT toString({column}) FROM {DATABASE}.{view} WHERE user_id = ?");
            assert_eq!(read(sql, user).await, expected, "{view} of asset {asset}");
        }
    }
    for (index, &(market, base, quote)) in markets.iter().enumerate() {
        let scaled_price = cache.scale_price(market as u32, Decimal::from(price));
        let sql = format!(
            "SELECT toString(price) FROM {DATABASE}.asset_prices_per_minute \
             WHERE asset_id = ? AND quote_asset_id = {quote} AND minute = toDateTime({minute})"
        );
        assert_eq!(
            read(sql, base as u64).await,
            scaled_price,
            "price of market {market}"
        );
        let sql = format!(
            "SELECT toString(volume) FROM {DATABASE}.trade_volumes WHERE maker_user_id = ?"
        );
        assert_eq!(
            read(sql, market_user(index)).await,
            scaled_price * cache.scale_volume(market as u32, Decimal::from(size)),
            "volume in market {market}"
        );
    }
}

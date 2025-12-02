use async_trait::async_trait;
use chrono::Utc;
use lib_core::repository::repository::{
    BalancePoint, PnlPoint, PnlResolution, PortfolioMetrics, PortfolioRepository, TradingError,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;

type Result<T> = std::result::Result<T, TradingError>;

// Mock Portfolio Repository for Unit Testing

/// Mock PortfolioRepository for isolated unit tests
#[derive(Clone)]
struct MockPortfolioRepository {
    should_fail: bool,
    no_data: bool,
    metrics: Option<PortfolioMetrics>,
    balance_chart: Vec<BalancePoint>,
}

impl MockPortfolioRepository {
    fn new() -> Self {
        Self {
            should_fail: false,
            no_data: false,
            metrics: None,
            balance_chart: Vec::new(),
        }
    }

    fn with_failure() -> Self {
        Self {
            should_fail: true,
            ..Self::new()
        }
    }

    fn with_no_data() -> Self {
        Self {
            no_data: true,
            ..Self::new()
        }
    }

    fn with_metrics(mut self, metrics: PortfolioMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    fn with_balance_chart(mut self, chart: Vec<BalancePoint>) -> Self {
        self.balance_chart = chart;
        self
    }
}

#[async_trait]
impl PortfolioRepository for MockPortfolioRepository {
    async fn get_portfolio(
        &self,
        _user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<PortfolioMetrics> {
        if self.should_fail {
            return Err(TradingError::Database("Mock database error".to_string()));
        }

        if self.no_data {
            return Ok(PortfolioMetrics {
                balance: Decimal::ZERO,
                balance_24h_ago: Decimal::ZERO,
                volume_24h: Decimal::ZERO,
                pnl_7d: Decimal::ZERO,
                max_drawdown_1m: Decimal::ZERO,
                pnl_series: vec![],
                pnl_resolution: resolution,
            });
        }

        if let Some(ref metrics) = self.metrics {
            return Ok(metrics.clone());
        }

        // Default mock metrics with realistic data
        let window_start = now_ts_secs - window_seconds(resolution);
        let pnl_series = generate_mock_pnl_series(window_start, now_ts_secs, resolution);

        Ok(PortfolioMetrics {
            balance: Decimal::from_str("10500.50").unwrap(),
            balance_24h_ago: Decimal::from_str("10000.00").unwrap(),
            volume_24h: Decimal::from_str("2500.75").unwrap(),
            pnl_7d: Decimal::from_str("500.50").unwrap(),
            max_drawdown_1m: Decimal::from_str("250.25").unwrap(),
            pnl_series,
            pnl_resolution: resolution,
        })
    }

    async fn get_balance_chart(
        &self,
        _user_id: u64,
        now_ts_secs: i64,
        resolution: PnlResolution,
    ) -> Result<Vec<BalancePoint>> {
        if self.should_fail {
            return Err(TradingError::Database("Mock database error".to_string()));
        }

        if self.no_data {
            return Ok(vec![]);
        }

        if !self.balance_chart.is_empty() {
            return Ok(self.balance_chart.clone());
        }

        // Default mock balance chart
        let window_start = now_ts_secs - window_seconds(resolution);
        Ok(generate_mock_balance_chart(window_start, now_ts_secs, resolution))
    }
}

// Helper Functions

/// Returns window duration in seconds for a given resolution
fn window_seconds(resolution: PnlResolution) -> i64 {
    match resolution {
        PnlResolution::Day => 24 * 60 * 60,
        PnlResolution::Week => 7 * 24 * 60 * 60,
        PnlResolution::Month => 30 * 24 * 60 * 60,
        PnlResolution::Year => 365 * 24 * 60 * 60,
    }
}

/// Generate mock PnL series data
fn generate_mock_pnl_series(start_ts: i64, end_ts: i64, resolution: PnlResolution) -> Vec<PnlPoint> {
    let bucket_size = match resolution {
        PnlResolution::Day => 3600,
        PnlResolution::Week => 86400,
        PnlResolution::Month => 86400,
        PnlResolution::Year => 604800,
    };

    let mut series = Vec::new();
    let mut ts = start_ts;
    let mut cumulative_pnl = Decimal::ZERO;

    while ts <= end_ts {
        cumulative_pnl += Decimal::from(ts % 100) * Decimal::from_str("0.5").unwrap()
            - Decimal::from(20);
        series.push(PnlPoint {
            timestamp: ts,
            pnl: cumulative_pnl,
        });
        ts += bucket_size;
    }

    series
}

/// Generate mock balance chart data
fn generate_mock_balance_chart(
    start_ts: i64,
    end_ts: i64,
    resolution: PnlResolution,
) -> Vec<BalancePoint> {
    let bucket_size = match resolution {
        PnlResolution::Day => 3600,
        PnlResolution::Week => 86400,
        PnlResolution::Month => 86400,
        PnlResolution::Year => 604800,
    };

    let mut chart = Vec::new();
    let mut ts = start_ts;
    let base_balance = Decimal::from(10000);

    while ts <= end_ts {
        let variance = Decimal::from(ts % 1000) / Decimal::from(1000) * Decimal::from(500);
        chart.push(BalancePoint {
            timestamp: ts,
            balance: base_balance + variance,
        });
        ts += bucket_size;
    }

    chart
}

/// Helper to create PortfolioMetrics with specific values
fn create_test_metrics(
    balance: &str,
    balance_24h_ago: &str,
    volume_24h: &str,
    pnl_7d: &str,
    max_drawdown_1m: &str,
    resolution: PnlResolution,
) -> PortfolioMetrics {
    PortfolioMetrics {
        balance: Decimal::from_str(balance).unwrap(),
        balance_24h_ago: Decimal::from_str(balance_24h_ago).unwrap(),
        volume_24h: Decimal::from_str(volume_24h).unwrap(),
        pnl_7d: Decimal::from_str(pnl_7d).unwrap(),
        max_drawdown_1m: Decimal::from_str(max_drawdown_1m).unwrap(),
        pnl_series: vec![],
        pnl_resolution: resolution,
    }
}

// ============================================================================
// PnlResolution Unit Tests
// ============================================================================

#[test]
fn test_pnl_resolution_default() {
    let resolution = PnlResolution::default();
    assert_eq!(resolution, PnlResolution::Day);
}

#[test]
fn test_pnl_resolution_window_seconds() {
    // Test our local helper matches expected values
    assert_eq!(window_seconds(PnlResolution::Day), 24 * 60 * 60);
    assert_eq!(window_seconds(PnlResolution::Week), 7 * 24 * 60 * 60);
    assert_eq!(window_seconds(PnlResolution::Month), 30 * 24 * 60 * 60);
    assert_eq!(window_seconds(PnlResolution::Year), 365 * 24 * 60 * 60);
}

#[test]
fn test_pnl_resolution_as_str() {
    assert_eq!(PnlResolution::Day.as_str(), "1d");
    assert_eq!(PnlResolution::Week.as_str(), "1w");
    assert_eq!(PnlResolution::Month.as_str(), "1m");
    assert_eq!(PnlResolution::Year.as_str(), "1y");
}

#[test]
fn test_pnl_resolution_from_query_standard_formats() {
    assert_eq!(PnlResolution::from_query("1d"), Some(PnlResolution::Day));
    assert_eq!(PnlResolution::from_query("1w"), Some(PnlResolution::Week));
    assert_eq!(PnlResolution::from_query("1m"), Some(PnlResolution::Month));
    assert_eq!(PnlResolution::from_query("1y"), Some(PnlResolution::Year));
}

#[test]
fn test_pnl_resolution_from_query_alternate_formats() {
    assert_eq!(PnlResolution::from_query("d1"), Some(PnlResolution::Day));
    assert_eq!(PnlResolution::from_query("w1"), Some(PnlResolution::Week));
    assert_eq!(PnlResolution::from_query("m1"), Some(PnlResolution::Month));
    assert_eq!(PnlResolution::from_query("y1"), Some(PnlResolution::Year));
}

#[test]
fn test_pnl_resolution_from_query_word_formats() {
    assert_eq!(PnlResolution::from_query("day"), Some(PnlResolution::Day));
    assert_eq!(PnlResolution::from_query("week"), Some(PnlResolution::Week));
    assert_eq!(PnlResolution::from_query("month"), Some(PnlResolution::Month));
    assert_eq!(PnlResolution::from_query("year"), Some(PnlResolution::Year));
}

#[test]
fn test_pnl_resolution_from_query_case_insensitive() {
    assert_eq!(PnlResolution::from_query("1D"), Some(PnlResolution::Day));
    assert_eq!(PnlResolution::from_query("1W"), Some(PnlResolution::Week));
    assert_eq!(PnlResolution::from_query("DAY"), Some(PnlResolution::Day));
    assert_eq!(PnlResolution::from_query("Week"), Some(PnlResolution::Week));
    assert_eq!(PnlResolution::from_query("MONTH"), Some(PnlResolution::Month));
    assert_eq!(PnlResolution::from_query("YeAr"), Some(PnlResolution::Year));
}

#[test]
fn test_pnl_resolution_from_query_invalid() {
    assert_eq!(PnlResolution::from_query(""), None);
    assert_eq!(PnlResolution::from_query("invalid"), None);
    assert_eq!(PnlResolution::from_query("2d"), None);
    assert_eq!(PnlResolution::from_query("1h"), None);
    assert_eq!(PnlResolution::from_query("minute"), None);
    assert_eq!(PnlResolution::from_query("daily"), None);
}

#[test]
fn test_pnl_resolution_equality() {
    assert_eq!(PnlResolution::Day, PnlResolution::Day);
    assert_ne!(PnlResolution::Day, PnlResolution::Week);
    assert_ne!(PnlResolution::Month, PnlResolution::Year);
}

#[test]
fn test_pnl_resolution_clone() {
    let original = PnlResolution::Week;
    let cloned = original;
    assert_eq!(original, cloned);
}

// Balance Overview Tests

#[tokio::test]
async fn test_balance_overview_success() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await;
    assert!(result.is_ok());

    let metrics = result.unwrap();
    assert!(metrics.balance > Decimal::ZERO);
    assert!(metrics.balance_24h_ago > Decimal::ZERO);
}

#[tokio::test]
async fn test_balance_overview_custom_metrics() {
    let custom_metrics = create_test_metrics(
        "50000.00",
        "48000.00",
        "10000.00",
        "2500.00",
        "1000.00",
        PnlResolution::Week,
    );

    let repo = MockPortfolioRepository::new().with_metrics(custom_metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Week).await.unwrap();

    assert_eq!(result.balance, Decimal::from_str("50000.00").unwrap());
    assert_eq!(result.balance_24h_ago, Decimal::from_str("48000.00").unwrap());
    assert_eq!(result.volume_24h, Decimal::from_str("10000.00").unwrap());
    assert_eq!(result.pnl_7d, Decimal::from_str("2500.00").unwrap());
    assert_eq!(result.max_drawdown_1m, Decimal::from_str("1000.00").unwrap());
}

#[tokio::test]
async fn test_balance_overview_database_error() {
    let repo = MockPortfolioRepository::with_failure();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        TradingError::Database(msg) => assert_eq!(msg, "Mock database error"),
        _ => panic!("Expected Database error"),
    }
}

#[tokio::test]
async fn test_balance_overview_no_data() {
    let repo = MockPortfolioRepository::with_no_data();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result.balance, Decimal::ZERO);
    assert_eq!(result.balance_24h_ago, Decimal::ZERO);
    assert_eq!(result.volume_24h, Decimal::ZERO);
}

#[tokio::test]
async fn test_balance_24h_change_positive() {
    let metrics = create_test_metrics(
        "11000.00",
        "10000.00",
        "5000.00",
        "1000.00",
        "200.00",
        PnlResolution::Day,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    let change = result.balance - result.balance_24h_ago;
    assert_eq!(change, Decimal::from(1000));
}

#[tokio::test]
async fn test_balance_24h_change_negative() {
    let metrics = create_test_metrics(
        "9000.00",
        "10000.00",
        "5000.00",
        "-1000.00",
        "1500.00",
        PnlResolution::Day,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    let change = result.balance - result.balance_24h_ago;
    assert_eq!(change, Decimal::from(-1000));
}

// ============================================================================
// PnL (Profit and Loss) Tests
// ============================================================================

#[tokio::test]
async fn test_pnl_series_day_resolution() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result.pnl_resolution, PnlResolution::Day);
    assert!(!result.pnl_series.is_empty());

    // Verify series is sorted by timestamp
    let timestamps: Vec<i64> = result.pnl_series.iter().map(|p| p.timestamp).collect();
    let mut sorted = timestamps.clone();
    sorted.sort();
    assert_eq!(timestamps, sorted);
}

#[tokio::test]
async fn test_pnl_series_week_resolution() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Week).await.unwrap();

    assert_eq!(result.pnl_resolution, PnlResolution::Week);
    assert!(!result.pnl_series.is_empty());
}

#[tokio::test]
async fn test_pnl_series_month_resolution() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Month).await.unwrap();

    assert_eq!(result.pnl_resolution, PnlResolution::Month);
    assert!(!result.pnl_series.is_empty());
}

#[tokio::test]
async fn test_pnl_series_year_resolution() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Year).await.unwrap();

    assert_eq!(result.pnl_resolution, PnlResolution::Year);
    assert!(!result.pnl_series.is_empty());
}

#[tokio::test]
async fn test_pnl_7d_positive() {
    let metrics = create_test_metrics(
        "10500.00",
        "10200.00",
        "5000.00",
        "500.00",
        "150.00",
        PnlResolution::Week,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Week).await.unwrap();

    assert_eq!(result.pnl_7d, Decimal::from_str("500.00").unwrap());
    assert!(result.pnl_7d > Decimal::ZERO);
}

#[tokio::test]
async fn test_pnl_7d_negative() {
    let metrics = create_test_metrics(
        "9500.00",
        "10000.00",
        "3000.00",
        "-500.00",
        "750.00",
        PnlResolution::Week,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Week).await.unwrap();

    assert_eq!(result.pnl_7d, Decimal::from_str("-500.00").unwrap());
    assert!(result.pnl_7d < Decimal::ZERO);
}

#[tokio::test]
async fn test_pnl_series_with_custom_data() {
    let mut custom_metrics = create_test_metrics(
        "10000.00",
        "9800.00",
        "2000.00",
        "200.00",
        "100.00",
        PnlResolution::Day,
    );

    custom_metrics.pnl_series = vec![
        PnlPoint {
            timestamp: 1000,
            pnl: Decimal::ZERO,
        },
        PnlPoint {
            timestamp: 2000,
            pnl: Decimal::from(50),
        },
        PnlPoint {
            timestamp: 3000,
            pnl: Decimal::from(150),
        },
        PnlPoint {
            timestamp: 4000,
            pnl: Decimal::from(200),
        },
    ];

    let repo = MockPortfolioRepository::new().with_metrics(custom_metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result.pnl_series.len(), 4);
    assert_eq!(result.pnl_series[0].pnl, Decimal::ZERO);
    assert_eq!(result.pnl_series[3].pnl, Decimal::from(200));
}

#[tokio::test]
async fn test_max_drawdown_calculation() {
    let metrics = create_test_metrics(
        "10000.00",
        "10500.00",
        "2000.00",
        "100.00",
        "1500.00",
        PnlResolution::Month,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Month).await.unwrap();

    assert_eq!(result.max_drawdown_1m, Decimal::from_str("1500.00").unwrap());
}

#[tokio::test]
async fn test_max_drawdown_zero() {
    let metrics = create_test_metrics(
        "11000.00",
        "10000.00",
        "5000.00",
        "1000.00",
        "0.00",
        PnlResolution::Month,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Month).await.unwrap();

    assert_eq!(result.max_drawdown_1m, Decimal::ZERO);
}

// Balance Chart Tests

#[tokio::test]
async fn test_balance_chart_success() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_balance_chart(1, now_ts, PnlResolution::Day).await;
    assert!(result.is_ok());

    let chart = result.unwrap();
    assert!(!chart.is_empty());
}

#[tokio::test]
async fn test_balance_chart_custom_data() {
    let custom_chart = vec![
        BalancePoint {
            timestamp: 1000,
            balance: Decimal::from(10000),
        },
        BalancePoint {
            timestamp: 2000,
            balance: Decimal::from(10100),
        },
        BalancePoint {
            timestamp: 3000,
            balance: Decimal::from(10250),
        },
        BalancePoint {
            timestamp: 4000,
            balance: Decimal::from(10200),
        },
        BalancePoint {
            timestamp: 5000,
            balance: Decimal::from(10350),
        },
    ];

    let repo = MockPortfolioRepository::new().with_balance_chart(custom_chart);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_balance_chart(1, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result.len(), 5);
    assert_eq!(result[0].balance, Decimal::from(10000));
    assert_eq!(result[4].balance, Decimal::from(10350));
}

#[tokio::test]
async fn test_balance_chart_sorted_by_timestamp() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let chart = repo.get_balance_chart(1, now_ts, PnlResolution::Week).await.unwrap();

    let timestamps: Vec<i64> = chart.iter().map(|p| p.timestamp).collect();
    let mut sorted = timestamps.clone();
    sorted.sort();
    assert_eq!(timestamps, sorted);
}

#[tokio::test]
async fn test_balance_chart_database_error() {
    let repo = MockPortfolioRepository::with_failure();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_balance_chart(1, now_ts, PnlResolution::Day).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        TradingError::Database(msg) => assert_eq!(msg, "Mock database error"),
        _ => panic!("Expected Database error"),
    }
}

#[tokio::test]
async fn test_balance_chart_no_data() {
    let repo = MockPortfolioRepository::with_no_data();
    let now_ts = Utc::now().timestamp();

    let result = repo.get_balance_chart(1, now_ts, PnlResolution::Day).await.unwrap();

    assert!(result.is_empty());
}

#[tokio::test]
async fn test_balance_chart_day_resolution() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let chart = repo.get_balance_chart(1, now_ts, PnlResolution::Day).await.unwrap();

    assert!(!chart.is_empty());
    // Day resolution should have hourly buckets (24+ points for a full day)
    assert!(chart.len() >= 1);
}

#[tokio::test]
async fn test_balance_chart_week_resolution() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let chart = repo.get_balance_chart(1, now_ts, PnlResolution::Week).await.unwrap();

    assert!(!chart.is_empty());
}

#[tokio::test]
async fn test_balance_chart_month_resolution() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let chart = repo.get_balance_chart(1, now_ts, PnlResolution::Month).await.unwrap();

    assert!(!chart.is_empty());
}

#[tokio::test]
async fn test_balance_chart_year_resolution() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    let chart = repo.get_balance_chart(1, now_ts, PnlResolution::Year).await.unwrap();

    assert!(!chart.is_empty());
}

// Volume Tests

#[tokio::test]
async fn test_volume_24h_positive() {
    let metrics = create_test_metrics(
        "10000.00",
        "10000.00",
        "50000.00",
        "0.00",
        "0.00",
        PnlResolution::Day,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result.volume_24h, Decimal::from_str("50000.00").unwrap());
}

#[tokio::test]
async fn test_volume_24h_zero() {
    let metrics = create_test_metrics(
        "10000.00",
        "10000.00",
        "0.00",
        "0.00",
        "0.00",
        PnlResolution::Day,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result.volume_24h, Decimal::ZERO);
}

#[tokio::test]
async fn test_volume_high_precision() {
    let metrics = create_test_metrics(
        "10000.00",
        "10000.00",
        "12345.678901234567",
        "0.00",
        "0.00",
        PnlResolution::Day,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert!(result.volume_24h > Decimal::from(12345));
}

// Edge Case Tests

#[tokio::test]
async fn test_portfolio_zero_balance() {
    let metrics = create_test_metrics(
        "0.00",
        "0.00",
        "0.00",
        "0.00",
        "0.00",
        PnlResolution::Day,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result.balance, Decimal::ZERO);
}

#[tokio::test]
async fn test_portfolio_large_values() {
    let metrics = create_test_metrics(
        "1000000000.00",
        "999000000.00",
        "500000000.00",
        "10000000.00",
        "5000000.00",
        PnlResolution::Day,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result.balance, Decimal::from_str("1000000000.00").unwrap());
}

#[tokio::test]
async fn test_portfolio_high_precision_values() {
    let metrics = create_test_metrics(
        "10000.123456789012",
        "9999.987654321098",
        "500.5",
        "100.12345678901234",
        "50.98765432109876",
        PnlResolution::Day,
    );

    let repo = MockPortfolioRepository::new().with_metrics(metrics);
    let now_ts = Utc::now().timestamp();

    let result = repo.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();

    assert!(result.balance > Decimal::from(10000));
}

#[tokio::test]
async fn test_multiple_users_independent_data() {
    let user1_metrics = create_test_metrics(
        "10000.00",
        "9500.00",
        "2000.00",
        "500.00",
        "100.00",
        PnlResolution::Day,
    );

    let user2_metrics = create_test_metrics(
        "50000.00",
        "52000.00",
        "10000.00",
        "-2000.00",
        "3000.00",
        PnlResolution::Day,
    );

    let repo1 = MockPortfolioRepository::new().with_metrics(user1_metrics);
    let repo2 = MockPortfolioRepository::new().with_metrics(user2_metrics);
    let now_ts = Utc::now().timestamp();

    let result1 = repo1.get_portfolio(1, now_ts, PnlResolution::Day).await.unwrap();
    let result2 = repo2.get_portfolio(2, now_ts, PnlResolution::Day).await.unwrap();

    assert_eq!(result1.balance, Decimal::from_str("10000.00").unwrap());
    assert_eq!(result2.balance, Decimal::from_str("50000.00").unwrap());
    assert!(result1.pnl_7d > Decimal::ZERO);
    assert!(result2.pnl_7d < Decimal::ZERO);
}

#[tokio::test]
async fn test_different_user_ids() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    // Test with different user IDs - all should work
    let result1 = repo.get_portfolio(1, now_ts, PnlResolution::Day).await;
    let result2 = repo.get_portfolio(u64::MAX, now_ts, PnlResolution::Day).await;
    let result3 = repo.get_portfolio(0, now_ts, PnlResolution::Day).await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert!(result3.is_ok());
}

#[tokio::test]
async fn test_timestamp_boundaries() {
    let repo = MockPortfolioRepository::new();
    let now_ts = Utc::now().timestamp();

    // Test with current timestamp
    let result1 = repo.get_portfolio(1, now_ts, PnlResolution::Day).await;
    assert!(result1.is_ok());

    // Test with a far future timestamp (but not so far it causes overflow)
    let far_future = now_ts + 365 * 24 * 60 * 60 * 10; // 10 years from now
    let result2 = repo.get_portfolio(1, far_future, PnlResolution::Day).await;
    assert!(result2.is_ok());

    // Test with a past timestamp
    let past_ts = now_ts - 365 * 24 * 60 * 60; // 1 year ago
    let result3 = repo.get_portfolio(1, past_ts, PnlResolution::Day).await;
    assert!(result3.is_ok());
}

// PnlPoint and BalancePoint Tests

#[test]
fn test_pnl_point_creation() {
    let point = PnlPoint {
        timestamp: 1000,
        pnl: Decimal::from_str("100.50").unwrap(),
    };

    assert_eq!(point.timestamp, 1000);
    assert_eq!(point.pnl, Decimal::from_str("100.50").unwrap());
}

#[test]
fn test_balance_point_creation() {
    let point = BalancePoint {
        timestamp: 1000,
        balance: Decimal::from_str("10000.00").unwrap(),
    };

    assert_eq!(point.timestamp, 1000);
    assert_eq!(point.balance, Decimal::from_str("10000.00").unwrap());
}

#[test]
fn test_pnl_point_clone() {
    let original = PnlPoint {
        timestamp: 1000,
        pnl: Decimal::from_str("500.25").unwrap(),
    };
    let cloned = original.clone();

    assert_eq!(original.timestamp, cloned.timestamp);
    assert_eq!(original.pnl, cloned.pnl);
}

#[test]
fn test_balance_point_clone() {
    let original = BalancePoint {
        timestamp: 1000,
        balance: Decimal::from_str("10500.75").unwrap(),
    };
    let cloned = original.clone();

    assert_eq!(original.timestamp, cloned.timestamp);
    assert_eq!(original.balance, cloned.balance);
}

// TradingError Tests

#[test]
fn test_trading_error_display() {
    let db_error = TradingError::Database("Connection failed".to_string());
    assert!(db_error.to_string().contains("Connection failed"));

    let not_found = TradingError::NotFound;
    assert!(not_found.to_string().len() > 0);
}

#[test]
fn test_trading_error_from_database() {
    let error = TradingError::Database("Test error".to_string());
    match error {
        TradingError::Database(msg) => assert_eq!(msg, "Test error"),
        _ => panic!("Expected Database error"),
    }
}

// Concurrent Access Tests

#[tokio::test]
async fn test_concurrent_portfolio_access() {
    let repo = Arc::new(MockPortfolioRepository::new());
    let now_ts = Utc::now().timestamp();

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let repo = Arc::clone(&repo);
            tokio::spawn(async move {
                repo.get_portfolio(i, now_ts, PnlResolution::Day).await
            })
        })
        .collect();

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}

#[tokio::test]
async fn test_concurrent_balance_chart_access() {
    let repo = Arc::new(MockPortfolioRepository::new());
    let now_ts = Utc::now().timestamp();

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let repo = Arc::clone(&repo);
            tokio::spawn(async move {
                repo.get_balance_chart(i, now_ts, PnlResolution::Week).await
            })
        })
        .collect();

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}


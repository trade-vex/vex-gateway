//! The gateway's Prometheus counters: one registry for the process, which only the internal
//! metrics listener serves (`metrics.bind`), never the public one.
//!
//! Every series is reported from the first scrape, at zero until its first count, so a rate over
//! one that has not moved yet reads 0, not "no data". Each series' counter is looked up once, when
//! the registry is created: counting is an atomic add, with no lock and no label hashing on the
//! request path.

use prometheus::{IntCounter, IntCounterVec, Opts, Registry, TextEncoder};
use std::sync::LazyLock;

/// The `Content-Type` of what [`render`] returns: the Prometheus text format.
pub const CONTENT_TYPE: &str = prometheus::TEXT_FORMAT;

/// How an order or a cancel the gateway tried to send to the matching engine ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineOutcome {
    /// The engine accepted it: 200.
    Accepted,
    /// The engine rejected it: 422.
    Rejected,
    /// The engine was busy and executed nothing: 503 with `Retry-After`.
    Busy,
    /// It never reached the engine: none is connected (503), or this gateway's publication failed
    /// (500).
    Unavailable,
    /// It was sent, but no answer came back: the wait timed out (408), or the engine link was lost
    /// (an order's 408, a cancel's 503).
    Unknown,
}

impl EngineOutcome {
    /// In declaration order, which [`orders`] and [`cancels`] index by.
    const ALL: [Self; 5] = [
        Self::Accepted,
        Self::Rejected,
        Self::Busy,
        Self::Unavailable,
        Self::Unknown,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Busy => "busy",
            Self::Unavailable => "unavailable",
            Self::Unknown => "unknown",
        }
    }
}

/// How a request the gateway refused to authenticate presented its credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    /// A bearer token, or a session cookie whose session holds one, refused with 401.
    Jwt,
    /// An `X-MBX-APIKEY` request the API-key check refused with a 4xx: an unknown, disabled or
    /// expired key, a refused IP or permission, or a bad, stale or replayed signature.
    ApiKey,
    /// A login refused for its email and password: 401.
    Login,
}

impl AuthMethod {
    /// In declaration order, which [`auth_failures`] indexes by.
    const ALL: [Self; 3] = [Self::Jwt, Self::ApiKey, Self::Login];

    fn label(self) -> &'static str {
        match self {
            Self::Jwt => "jwt",
            Self::ApiKey => "api_key",
            Self::Login => "login",
        }
    }
}

/// The rate-limit budget (`[rate_limit]`) a refused request was over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateLimitBudget {
    /// Placing orders: `rate_limit.orders`.
    Orders,
    /// Every other route without a limit of its own, cancelling orders included:
    /// `rate_limit.general`.
    General,
}

impl RateLimitBudget {
    /// In declaration order, which [`rate_limited`] indexes by.
    const ALL: [Self; 2] = [Self::Orders, Self::General];

    /// The budget's name: its `budget` label here, and its part of the rate limiter's Redis keys
    /// and log lines.
    pub fn label(self) -> &'static str {
        match self {
            Self::Orders => "orders",
            Self::General => "general",
        }
    }
}

struct Metrics {
    registry: Registry,
    orders: [IntCounter; EngineOutcome::ALL.len()],
    cancels: [IntCounter; EngineOutcome::ALL.len()],
    auth_failures: [IntCounter; AuthMethod::ALL.len()],
    rate_limited: [IntCounter; RateLimitBudget::ALL.len()],
}

static METRICS: LazyLock<Metrics> = LazyLock::new(|| {
    let registry = Registry::new();
    let outcomes = EngineOutcome::ALL.map(EngineOutcome::label);
    Metrics {
        orders: family(
            &registry,
            "vex_gateway_orders_total",
            "Orders the gateway tried to send to the matching engine, by outcome",
            "outcome",
            outcomes,
        ),
        cancels: family(
            &registry,
            "vex_gateway_cancels_total",
            "Cancels the gateway tried to send to the matching engine, by outcome",
            "outcome",
            outcomes,
        ),
        auth_failures: family(
            &registry,
            "vex_gateway_auth_failures_total",
            "Requests the gateway refused to authenticate, by credential method",
            "method",
            AuthMethod::ALL.map(AuthMethod::label),
        ),
        rate_limited: family(
            &registry,
            "vex_gateway_rate_limited_total",
            "Requests the gateway refused over a rate-limit budget, by budget",
            "budget",
            RateLimitBudget::ALL.map(RateLimitBudget::label),
        ),
        registry,
    }
});

/// Registers the counter family `name` and creates its series, one per value of `label`.
fn family<const N: usize>(
    registry: &Registry,
    name: &str,
    help: &str,
    label: &str,
    values: [&str; N],
) -> [IntCounter; N] {
    let family =
        IntCounterVec::new(Opts::new(name, help), &[label]).expect("a valid name and label");
    registry
        .register(Box::new(family.clone()))
        .expect("each family is registered once");
    values.map(|value| family.with_label_values(&[value]))
}

/// The count of orders that ended with `outcome`.
pub fn orders(outcome: EngineOutcome) -> &'static IntCounter {
    &METRICS.orders[outcome as usize]
}

/// The count of cancels that ended with `outcome`.
pub fn cancels(outcome: EngineOutcome) -> &'static IntCounter {
    &METRICS.cancels[outcome as usize]
}

/// The count of requests refused authentication that presented `method`'s credentials.
pub fn auth_failures(method: AuthMethod) -> &'static IntCounter {
    &METRICS.auth_failures[method as usize]
}

/// The count of requests refused over `budget`.
pub fn rate_limited(budget: RateLimitBudget) -> &'static IntCounter {
    &METRICS.rate_limited[budget as usize]
}

/// Every counter, in the Prometheus text format ([`CONTENT_TYPE`]).
pub fn render() -> prometheus::Result<String> {
    TextEncoder::new().encode_to_string(&METRICS.registry.gather())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value the text format shows for `series`.
    fn rendered(series: &str) -> u64 {
        let text = render().unwrap();
        text.lines()
            .find_map(|line| line.strip_prefix(series)?.strip_prefix(' '))
            .unwrap_or_else(|| panic!("{series} is not rendered:\n{text}"))
            .parse()
            .unwrap()
    }

    #[test]
    fn every_series_is_rendered_from_startup() {
        let text = render().unwrap();

        for family in [
            "vex_gateway_orders_total",
            "vex_gateway_cancels_total",
            "vex_gateway_auth_failures_total",
            "vex_gateway_rate_limited_total",
        ] {
            assert!(
                text.contains(&format!("# TYPE {family} counter\n")),
                "{text}"
            );
        }
        for outcome in ["accepted", "rejected", "busy", "unavailable", "unknown"] {
            for family in ["vex_gateway_orders_total", "vex_gateway_cancels_total"] {
                rendered(&format!("{family}{{outcome=\"{outcome}\"}}"));
            }
        }
        for method in ["jwt", "api_key", "login"] {
            rendered(&format!(
                "vex_gateway_auth_failures_total{{method=\"{method}\"}}"
            ));
        }
        for budget in ["orders", "general"] {
            rendered(&format!(
                "vex_gateway_rate_limited_total{{budget=\"{budget}\"}}"
            ));
        }
    }

    /// The counters are indexed by declaration order: each value must count under its own label.
    #[test]
    fn each_value_counts_in_its_own_series() {
        for outcome in EngineOutcome::ALL {
            for (counter, family) in [
                (orders(outcome), "vex_gateway_orders_total"),
                (cancels(outcome), "vex_gateway_cancels_total"),
            ] {
                let series = format!("{family}{{outcome=\"{}\"}}", outcome.label());
                let before = rendered(&series);
                counter.inc();
                assert_eq!(rendered(&series), before + 1, "{series}");
            }
        }
        for method in AuthMethod::ALL {
            let series = format!(
                "vex_gateway_auth_failures_total{{method=\"{}\"}}",
                method.label()
            );
            let before = rendered(&series);
            auth_failures(method).inc();
            assert_eq!(rendered(&series), before + 1, "{series}");
        }
        for budget in RateLimitBudget::ALL {
            let series = format!(
                "vex_gateway_rate_limited_total{{budget=\"{}\"}}",
                budget.label()
            );
            let before = rendered(&series);
            rate_limited(budget).inc();
            assert_eq!(rendered(&series), before + 1, "{series}");
        }
    }
}

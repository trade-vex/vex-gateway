//! Response correlation service for matching async responses to requests

use common::OrderCommand;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;
use tracing::{debug, warn};

/// Global counter for generating unique client order IDs
static CLIENT_ORDER_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The engine link is unhealthy once request timeouts have kept occurring for this long with no
/// routed response in between. Requests sent 5 s apart into a stalled engine time out across
/// this span; one burst of concurrent timeouts from a single stall spans ~0 s and stays healthy.
const ENGINE_LINK_FAILURE_SPAN_MS: u64 = 10_000;

/// Timeouts further apart than this are separate incidents, not one that persists: the later one
/// restarts the window. Without it, two lone lost responses hours apart on a quiet gateway would
/// read as a 10 s outage.
const ENGINE_LINK_TIMEOUT_GAP_MS: u64 = 30_000;

/// Generate a globally unique client order ID
#[inline]
pub fn generate_client_order_id() -> u64 {
    let counter = CLIENT_ORDER_COUNTER.fetch_add(1, Ordering::Relaxed);
    generate_client_order_id_at(SystemTime::now(), counter)
}

fn generate_client_order_id_at(now: SystemTime, counter: u64) -> u64 {
    let timestamp = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(counter);

    // Combine: top 48 bits = timestamp, bottom 16 bits = counter
    ((timestamp & 0xFFFF_FFFF_FFFF) << 16) | (counter & 0xFFFF)
}

/// Manages request-response correlation using client_order_id
#[derive(Clone)]
pub struct CorrelationService {
    pending_requests: Arc<DashMap<u64, oneshot::Sender<OrderCommand>>>,
    /// Origin of the millisecond stamps below. A stamp is `ms since origin + 1`; 0 means none.
    clock_origin: std::time::Instant,
    /// First timeout of the current window (since the last routed response or quiet gap).
    first_timeout_ms: Arc<AtomicU64>,
    /// Latest timeout since the last routed response.
    last_timeout_ms: Arc<AtomicU64>,
}

impl CorrelationService {
    pub fn new() -> Self {
        // let shard_count = num_cpus::get() * 4;
        let base_shard_count = num_cpus::get() * 4;
        // DashMap requires shard_count to be a power of two
        let shard_count = base_shard_count.next_power_of_two();
        Self {
            pending_requests: Arc::new(DashMap::with_shard_amount(shard_count)),
            clock_origin: std::time::Instant::now(),
            first_timeout_ms: Arc::new(AtomicU64::new(0)),
            last_timeout_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Register a pending request and return receiver for response
    #[inline]
    pub fn register_request(&self, client_order_id: u64) -> oneshot::Receiver<OrderCommand> {
        let (tx, rx) = oneshot::channel();
        self.pending_requests.insert(client_order_id, tx);

        debug!(
            client_order_id = client_order_id,
            pending = self.pending_requests.len(),
            "Registered pending request"
        );

        rx
    }

    /// Route response to waiting handler
    #[inline]
    pub fn route_response(&self, response: OrderCommand) -> bool {
        let client_order_id = response.client_order_id;
        let order_id = response.order_id;

        if let Some((_, sender)) = self.pending_requests.remove(&client_order_id) {
            match sender.send(response) {
                Ok(()) => {
                    debug!(
                        client_order_id = client_order_id,
                        order_id = order_id,
                        pending = self.pending_requests.len(),
                        "Successfully routed response"
                    );
                    self.first_timeout_ms.store(0, Ordering::Relaxed);
                    self.last_timeout_ms.store(0, Ordering::Relaxed);
                    true
                }
                Err(_) => {
                    warn!(
                        client_order_id = client_order_id,
                        "Response arrived after handler timeout"
                    );
                    false
                }
            }
        } else {
            warn!(
                client_order_id = client_order_id,
                order_id = order_id,
                "Unknown client_order_id - no waiting handler"
            );
            false
        }
    }

    /// Cancel a pending request (on timeout)
    #[inline]
    pub fn cancel_request(&self, client_order_id: u64) -> bool {
        if self.pending_requests.remove(&client_order_id).is_some() {
            debug!(
                client_order_id = client_order_id,
                "Cancelled pending request"
            );
            true
        } else {
            false
        }
    }

    /// Record that a request received no response before its deadline.
    pub fn timeout_request(&self, client_order_id: u64) -> bool {
        self.timeout_request_at(client_order_id, std::time::Instant::now())
    }

    /// [`Self::timeout_request`] for a timeout observed at `at`.
    pub fn timeout_request_at(&self, client_order_id: u64, at: std::time::Instant) -> bool {
        let elapsed_ms = at.saturating_duration_since(self.clock_origin).as_millis();
        let stamp = u64::try_from(elapsed_ms)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        // Move the window start BEFORE publishing `last` (Release), so a health check that sees
        // this `last` (Acquire) never pairs it with the previous window's start.
        let previous = self.last_timeout_ms.load(Ordering::Relaxed);
        if stamp.saturating_sub(previous) > ENGINE_LINK_TIMEOUT_GAP_MS {
            // Nothing timed out recently: a new incident restarts the window.
            self.first_timeout_ms.store(stamp, Ordering::Relaxed);
        } else {
            // Open the window if none is open; otherwise this timeout only extends it.
            let _ = self.first_timeout_ms.compare_exchange(
                0,
                stamp,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
        self.last_timeout_ms.fetch_max(stamp, Ordering::Release);
        self.cancel_request(client_order_id)
    }

    /// False once timeouts have kept occurring for `ENGINE_LINK_FAILURE_SPAN_MS`, none more than
    /// `ENGINE_LINK_TIMEOUT_GAP_MS` after the previous, with no routed response in between. How
    /// many requests timed out does not matter, only for how long.
    pub fn is_engine_link_healthy(&self) -> bool {
        // `last` (Acquire) before `first`: whatever races these loads, `first` is then never older
        // than the window `last` belongs to, so a race reads as healthy, not as a spurious 503.
        let last = self.last_timeout_ms.load(Ordering::Acquire);
        let first = self.first_timeout_ms.load(Ordering::Relaxed);
        first == 0 || last.saturating_sub(first) < ENGINE_LINK_FAILURE_SPAN_MS
    }

    /// Get count of pending requests (for monitoring)
    pub fn pending_count(&self) -> usize {
        self.pending_requests.len()
    }
}

impl Default for CorrelationService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn concurrent_timeouts_in_one_stall_stay_healthy() {
        let service = CorrelationService::new();
        std::thread::scope(|scope| {
            for client_order_id in 1..=3 {
                let service = &service;
                scope.spawn(move || service.timeout_request(client_order_id));
            }
        });

        assert!(service.is_engine_link_healthy());
    }

    #[test]
    fn sustained_timeouts_mark_link_unhealthy() {
        let service = CorrelationService::new();
        let t0 = std::time::Instant::now();

        service.timeout_request_at(1, t0);
        service.timeout_request_at(2, t0 + 5 * SECOND);
        service.timeout_request_at(3, t0 + 9 * SECOND);
        assert!(service.is_engine_link_healthy());

        service.timeout_request_at(4, t0 + 10 * SECOND);
        assert!(!service.is_engine_link_healthy());
    }

    #[test]
    fn timeouts_separated_by_a_quiet_gap_are_separate_incidents() {
        let service = CorrelationService::new();
        let t0 = std::time::Instant::now();
        service.timeout_request_at(1, t0);
        service.timeout_request_at(2, t0 + 31 * SECOND);
        assert!(service.is_engine_link_healthy());

        // Timeouts that then persist are still detected, measured from the new window.
        service.timeout_request_at(3, t0 + 36 * SECOND);
        assert!(service.is_engine_link_healthy());
        service.timeout_request_at(4, t0 + 41 * SECOND);
        assert!(!service.is_engine_link_healthy());
    }

    #[test]
    fn routed_response_recovers_link_and_restarts_the_window() {
        let service = CorrelationService::new();
        let t0 = std::time::Instant::now();
        service.timeout_request_at(1, t0);
        service.timeout_request_at(2, t0 + 10 * SECOND);
        assert!(!service.is_engine_link_healthy());

        let _receiver = service.register_request(3);
        assert!(service.route_response(OrderCommand {
            client_order_id: 3,
            ..OrderCommand::default()
        }));
        assert!(service.is_engine_link_healthy());

        // The window restarts at the next timeout instead of reaching back to t0.
        service.timeout_request_at(4, t0 + 11 * SECOND);
        service.timeout_request_at(5, t0 + 20 * SECOND);
        assert!(service.is_engine_link_healthy());
        service.timeout_request_at(6, t0 + 21 * SECOND);
        assert!(!service.is_engine_link_healthy());
    }

    #[test]
    fn only_routed_response_resets_engine_link_failure() {
        let service = CorrelationService::new();
        let t0 = std::time::Instant::now();
        service.timeout_request_at(1, t0);
        service.timeout_request_at(2, t0 + 10 * SECOND);
        assert!(!service.is_engine_link_healthy());

        assert!(!service.route_response(OrderCommand {
            client_order_id: 4,
            ..OrderCommand::default()
        }));
        assert!(!service.is_engine_link_healthy());

        let receiver = service.register_request(5);
        drop(receiver);
        assert!(!service.route_response(OrderCommand {
            client_order_id: 5,
            ..OrderCommand::default()
        }));
        assert!(!service.is_engine_link_healthy());

        let _receiver = service.register_request(6);
        assert!(service.route_response(OrderCommand {
            client_order_id: 6,
            ..OrderCommand::default()
        }));
        assert!(service.is_engine_link_healthy());
    }

    #[test]
    fn correlation_ids_use_counter_when_clock_is_before_epoch() {
        let before_epoch = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();

        let first = generate_client_order_id_at(before_epoch, 41);
        let second = generate_client_order_id_at(before_epoch, 42);

        assert_ne!(first, second);
        assert_eq!(first, (41 << 16) | 41);
        assert_eq!(second, (42 << 16) | 42);
    }
}

/// Wait for response with timeout
#[inline]
pub async fn wait_for_response(
    receiver: oneshot::Receiver<OrderCommand>,
    timeout: Duration,
) -> Result<OrderCommand, ResponseError> {
    tokio::time::timeout(timeout, receiver)
        .await
        .map_err(|_| ResponseError::Timeout)?
        .map_err(|_| ResponseError::Cancelled)
}

/// Response errors
#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    #[error("Response timeout")]
    Timeout,

    #[error("Response channel cancelled")]
    Cancelled,
}

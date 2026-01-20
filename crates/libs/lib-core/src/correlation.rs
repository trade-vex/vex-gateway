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

/// Generate a globally unique client order ID
///
/// This function is designed to never panic, even if system time goes backwards
/// (e.g., due to VM migration, NTP correction, or container issues).
#[inline]
pub fn generate_client_order_id() -> u64 {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|e| {
            // System clock went backwards - use counter-only fallback
            warn!(
                error = %e,
                "System time went backwards, using counter-only client order ID"
            );
            Duration::from_secs(0)
        })
        .as_nanos() as u64;

    let counter = CLIENT_ORDER_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Combine: top 48 bits = timestamp, bottom 16 bits = counter
    // Even with timestamp=0, the counter ensures uniqueness
    ((timestamp & 0xFFFF_FFFF_FFFF) << 16) | (counter & 0xFFFF)
}

/// Manages request-response correlation using client_order_id
#[derive(Clone)]
pub struct CorrelationService {
    pending_requests: Arc<DashMap<u64, oneshot::Sender<OrderCommand>>>,
}

impl CorrelationService {
    pub fn new() -> Self {
        // let shard_count = num_cpus::get() * 4;
        let base_shard_count = num_cpus::get() * 4;
        // DashMap requires shard_count to be a power of two
        let shard_count = base_shard_count.next_power_of_two();
        Self {
            pending_requests: Arc::new(DashMap::with_shard_amount(shard_count)),
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

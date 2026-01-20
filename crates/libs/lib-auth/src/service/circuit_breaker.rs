//! Circuit breaker implementation for external service calls.
//!
//! This module provides circuit breaker protection for external HTTP calls
//! to OAuth providers and other external services, preventing cascading
//! failures when external services are unavailable.
//!
//! # Circuit States
//!
//! - **Closed**: Normal operation, requests pass through
//! - **Open**: Circuit tripped, requests fail immediately
//! - **Half-Open**: Testing if service recovered
//!
//! # Configuration
//!
//! Default settings:
//! - Failure threshold: 5 consecutive failures to open circuit
//! - Success threshold: 2 consecutive successes to close circuit
//! - Backoff: Exponential from 5s to 60s

use failsafe::backoff::exponential;
use failsafe::failure_policy::consecutive_failures;
use failsafe::futures::CircuitBreaker;
use failsafe::{Config, Error as CircuitError};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Error type for circuit breaker operations.
#[derive(Debug, Clone)]
pub enum CircuitBreakerError<E> {
    /// The underlying operation failed.
    Inner(E),
    /// The circuit is open, request was rejected.
    CircuitOpen,
}

impl<E: std::fmt::Display> std::fmt::Display for CircuitBreakerError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitBreakerError::Inner(e) => write!(f, "{}", e),
            CircuitBreakerError::CircuitOpen => {
                write!(f, "Circuit breaker is open, request rejected")
            }
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for CircuitBreakerError<E> {}

/// Configuration for circuit breaker behavior.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Minimum backoff duration when circuit opens.
    pub min_backoff: Duration,
    /// Maximum backoff duration.
    pub max_backoff: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            min_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(60),
        }
    }
}

/// Concrete circuit breaker type using exponential backoff and consecutive failures policy.
type ConcreteCircuitBreaker = failsafe::StateMachine<
    failsafe::failure_policy::ConsecutiveFailures<failsafe::backoff::Exponential>,
    (),
>;

/// Manages circuit breakers for multiple external services.
///
/// Each service (identified by name) gets its own circuit breaker instance,
/// allowing independent failure tracking per provider.
#[derive(Clone)]
pub struct CircuitBreakerManager {
    breakers: Arc<RwLock<HashMap<String, Arc<ConcreteCircuitBreaker>>>>,
    config: CircuitBreakerConfig,
}

impl CircuitBreakerManager {
    /// Create a new circuit breaker manager with default configuration.
    pub fn new() -> Self {
        Self {
            breakers: Arc::new(RwLock::new(HashMap::new())),
            config: CircuitBreakerConfig::default(),
        }
    }

    /// Create a new circuit breaker manager with custom configuration.
    pub fn with_config(config: CircuitBreakerConfig) -> Self {
        Self {
            breakers: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Get or create a circuit breaker for the given service name.
    async fn get_or_create(&self, service: &str) -> Arc<ConcreteCircuitBreaker> {
        // Try read lock first (fast path)
        {
            let breakers = self.breakers.read().await;
            if let Some(cb) = breakers.get(service) {
                return cb.clone();
            }
        }

        // Need to create a new circuit breaker
        let mut breakers = self.breakers.write().await;

        // Double-check after acquiring write lock
        if let Some(cb) = breakers.get(service) {
            return cb.clone();
        }

        // Create new circuit breaker with configured policy
        let backoff = exponential(self.config.min_backoff, self.config.max_backoff);
        let policy = consecutive_failures(self.config.failure_threshold, backoff);

        let cb: ConcreteCircuitBreaker = Config::new().failure_policy(policy).build();

        let cb = Arc::new(cb);
        breakers.insert(service.to_string(), cb.clone());

        tracing::info!(service = %service, "Created circuit breaker");

        cb
    }

    /// Execute an async operation with circuit breaker protection.
    ///
    /// # Arguments
    ///
    /// * `service` - Name of the service (e.g., "github", "google")
    /// * `future` - The future that performs the external call
    ///
    /// # Returns
    ///
    /// Returns `Ok(T)` on success, `Err(CircuitBreakerError::Inner(E))` on operation failure,
    /// or `Err(CircuitBreakerError::CircuitOpen)` if the circuit is open.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = manager.call("github", async {
    ///     client.get(url).send().await
    /// }).await;
    /// ```
    pub async fn call<Fut, T, E>(
        &self,
        service: &str,
        future: Fut,
    ) -> Result<T, CircuitBreakerError<E>>
    where
        Fut: Future<Output = Result<T, E>>,
    {
        let cb = self.get_or_create(service).await;

        match cb.call(future).await {
            Ok(result) => {
                tracing::trace!(service = %service, "Circuit breaker call succeeded");
                Ok(result)
            }
            Err(CircuitError::Inner(e)) => {
                tracing::warn!(service = %service, "Circuit breaker recorded failure");
                Err(CircuitBreakerError::Inner(e))
            }
            Err(CircuitError::Rejected) => {
                tracing::warn!(service = %service, "Circuit breaker rejected request (open)");
                Err(CircuitBreakerError::CircuitOpen)
            }
        }
    }

    /// Check if the circuit breaker for a service is currently open.
    pub async fn is_open(&self, service: &str) -> bool {
        let breakers = self.breakers.read().await;
        if let Some(cb) = breakers.get(service) {
            !cb.is_call_permitted()
        } else {
            false // No circuit breaker yet means it would be created in closed state
        }
    }
}

impl Default for CircuitBreakerManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_circuit_breaker_manager_creation() {
        let manager = CircuitBreakerManager::new();
        assert!(!manager.is_open("test").await);
    }

    #[tokio::test]
    async fn test_circuit_breaker_success() {
        let manager = CircuitBreakerManager::new();

        let result: Result<i32, CircuitBreakerError<&str>> =
            manager.call("test", async { Ok::<i32, &str>(42) }).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_circuit_breaker_failure() {
        let manager = CircuitBreakerManager::new();

        let result: Result<i32, CircuitBreakerError<&str>> = manager
            .call("test", async { Err::<i32, &str>("error") })
            .await;

        assert!(matches!(result, Err(CircuitBreakerError::Inner("error"))));
    }

    #[tokio::test]
    async fn test_circuit_breaker_opens_after_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            min_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(10),
        };
        let manager = CircuitBreakerManager::with_config(config);

        // Fail 3 times to trip the circuit
        for _ in 0..3 {
            let _: Result<i32, CircuitBreakerError<&str>> = manager
                .call("test", async { Err::<i32, &str>("error") })
                .await;
        }

        // Next call should be rejected
        let result: Result<i32, CircuitBreakerError<&str>> =
            manager.call("test", async { Ok::<i32, &str>(42) }).await;

        assert!(matches!(result, Err(CircuitBreakerError::CircuitOpen)));
    }

    #[tokio::test]
    async fn test_circuit_breaker_independent_services() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            min_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(10),
        };
        let manager = CircuitBreakerManager::with_config(config);

        // Trip circuit for "service_a"
        for _ in 0..2 {
            let _: Result<i32, CircuitBreakerError<&str>> = manager
                .call("service_a", async { Err::<i32, &str>("error") })
                .await;
        }

        // service_a should be open
        let result_a: Result<i32, CircuitBreakerError<&str>> = manager
            .call("service_a", async { Ok::<i32, &str>(42) })
            .await;
        assert!(matches!(result_a, Err(CircuitBreakerError::CircuitOpen)));

        // service_b should still work
        let result_b: Result<i32, CircuitBreakerError<&str>> = manager
            .call("service_b", async { Ok::<i32, &str>(42) })
            .await;
        assert!(result_b.is_ok());
    }

    #[tokio::test]
    async fn test_circuit_breaker_error_display() {
        let inner_error: CircuitBreakerError<&str> = CircuitBreakerError::Inner("test error");
        assert_eq!(inner_error.to_string(), "test error");

        let open_error: CircuitBreakerError<&str> = CircuitBreakerError::CircuitOpen;
        assert!(open_error.to_string().contains("Circuit breaker is open"));
    }

    #[tokio::test]
    async fn test_circuit_breaker_call_count() {
        let call_count = Arc::new(AtomicU32::new(0));
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            min_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(10),
        };
        let manager = CircuitBreakerManager::with_config(config);

        // Fail 2 times to trip the circuit
        for _ in 0..2 {
            let count = call_count.clone();
            let _: Result<i32, CircuitBreakerError<&str>> = manager
                .call("test", async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err::<i32, &str>("error")
                })
                .await;
        }

        // This call should be rejected without invoking the operation
        let count = call_count.clone();
        let _: Result<i32, CircuitBreakerError<&str>> = manager
            .call("test", async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok::<i32, &str>(42)
            })
            .await;

        // Only 2 calls should have been made (the 3rd was rejected)
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }
}

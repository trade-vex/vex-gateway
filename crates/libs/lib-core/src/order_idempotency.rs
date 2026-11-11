use redis::{aio::ConnectionManager, Script};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;

#[cfg(not(test))]
const IDEMPOTENCY_TTL_SECS: u64 = 24 * 60 * 60;
#[cfg(test)]
const IDEMPOTENCY_TTL_SECS: u64 = 1;
// How long a released claim stays visible as such. It only has to outlive the requests already
// waiting on the claim (5 s each); a new request treats the marker as a free key.
const RELEASED_MARKER_TTL_SECS: u64 = 60;
// A waiter polls with capped exponential backoff (25, 50, ... 500 ms): a fast completion is still
// seen within tens of milliseconds, while a stalled claim costs ~15 GETs over a 5 s wait instead
// of ~200 on the Redis connection shared with sessions and caches.
const COMPLETION_POLL_INITIAL: Duration = Duration::from_millis(25);
const COMPLETION_POLL_MAX: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct OrderIdempotencyStore {
    connection: ConnectionManager,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdempotentOrderOutcome {
    pub http_status: u16,
    pub order_id: u64,
    pub client_order_id: u64,
    pub status: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderClaim {
    Acquired,
    InFlight { client_order_id: u64 },
    Completed(IdempotentOrderOutcome),
    PayloadMismatch,
}

/// Why an attempt gave its claim back. Either way the engine executed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReleaseReason {
    /// The engine was too busy to take the order; a retry shortly after is expected to succeed.
    Busy,
    /// The order was not published: no core is connected, or this gateway's publication failed.
    EngineUnavailable,
}

/// What a request waiting on another attempt's claim learns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionWait {
    /// The attempt finished: replay its outcome.
    Completed(IdempotentOrderOutcome),
    /// The attempt gave its claim back because the engine executed nothing, so the key is free
    /// and a retry is published afresh. The reason lets the waiter answer as the attempt did.
    Released(ReleaseReason),
    /// Still in flight at the deadline, or the claim vanished without a release (expiry,
    /// eviction, failover), in which case the order may already have been published.
    Unresolved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum StoredOrderAttempt {
    InFlight {
        fingerprint: String,
        client_order_id: u64,
    },
    Completed {
        fingerprint: String,
        outcome: IdempotentOrderOutcome,
    },
    /// Left by `release` for `RELEASED_MARKER_TTL_SECS` in place of the claim, so a waiter can
    /// tell a release (safe to retry) from a claim that vanished (perhaps published), and why the
    /// engine executed nothing. `claim` treats it as a free key.
    Released(ReleaseReason),
}

#[derive(Debug, Error)]
pub enum OrderIdempotencyError {
    #[error("Redis idempotency operation failed: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("Idempotency state serialization failed: {0}")]
    Serialization(String),
    #[error("Idempotency claim changed while it was being read")]
    ClaimChanged,
}

impl OrderIdempotencyStore {
    pub fn new(connection: ConnectionManager) -> Self {
        Self { connection }
    }

    pub async fn claim(
        &self,
        user_id: u64,
        idempotency_key: &str,
        fingerprint: &str,
        client_order_id: u64,
    ) -> Result<OrderClaim, OrderIdempotencyError> {
        let key = redis_key(user_id, idempotency_key);
        let attempt = StoredOrderAttempt::InFlight {
            fingerprint: fingerprint.to_owned(),
            client_order_id,
        };
        let bytes = serialize(&attempt)?;
        let released_busy = serialize(&StoredOrderAttempt::Released(ReleaseReason::Busy))?;
        let released_unavailable = serialize(&StoredOrderAttempt::Released(
            ReleaseReason::EngineUnavailable,
        ))?;
        // SET NX, except that a release marker, whatever its reason, counts as a free key.
        let script = Script::new(
            r#"
            local current = redis.call('GET', KEYS[1])
            if current == false or current == ARGV[2] or current == ARGV[4] then
                redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[3])
                return 1
            end
            return 0
            "#,
        );

        for _ in 0..2 {
            let mut connection = self.connection.clone();
            let outcome: Result<i32, _> = script
                .key(&key)
                .arg(&bytes)
                .arg(&released_busy)
                .arg(IDEMPOTENCY_TTL_SECS)
                .arg(&released_unavailable)
                .invoke_async(&mut connection)
                .await;

            let acquired = match outcome {
                Ok(acquired) => acquired == 1,
                Err(error) => {
                    // The connection can drop AFTER Redis executed the SET but before the reply
                    // reaches us. Propagating here stranded our own claim for the full 24h TTL:
                    // every retry then saw InFlight, waited, and returned 408, with no handler
                    // left alive to complete it (finding G4-1).
                    //
                    // Read the key back instead. If it holds THIS attempt -- same fingerprint and
                    // client_order_id -- then our SET did land and the claim is ours to continue
                    // with. Any other state (or a failed read-back) propagates as before.
                    // Deliberately NOT treated as "our claim landed".
                    //
                    // A record matching our fingerprint and client_order_id does not prove we
                    // wrote it: client_order_id combines a timestamp with a PROCESS-LOCAL counter
                    // and carries no gateway identity (correlation.rs), so a second gateway
                    // handling the same retry of the same request can mint an identical record.
                    // Claiming Acquired on that evidence would let both publish -- a duplicate
                    // order, which is worse than the stranded claim this recovery was meant to
                    // avoid. Proving ownership needs a per-attempt nonce carried through
                    // complete/release; until then, report the state we find and fail safe. A
                    // release marker is not our claim either.
                    let existing = self.lookup_state(&key).await.ok().flatten();
                    return existing
                        .and_then(|existing| claim_from_state(existing, fingerprint))
                        .ok_or_else(|| error.into());
                }
            };

            if acquired {
                return Ok(OrderClaim::Acquired);
            }

            // Freed between the two reads (expired or released): try again.
            if let Some(claim) = self
                .lookup_state(&key)
                .await?
                .and_then(|existing| claim_from_state(existing, fingerprint))
            {
                return Ok(claim);
            }
        }

        Err(OrderIdempotencyError::ClaimChanged)
    }

    pub async fn complete(
        &self,
        user_id: u64,
        idempotency_key: &str,
        fingerprint: &str,
        outcome: IdempotentOrderOutcome,
    ) -> Result<bool, OrderIdempotencyError> {
        let key = redis_key(user_id, idempotency_key);
        let expected = serialize(&StoredOrderAttempt::InFlight {
            fingerprint: fingerprint.to_owned(),
            client_order_id: outcome.client_order_id,
        })?;
        let completed = serialize(&StoredOrderAttempt::Completed {
            fingerprint: fingerprint.to_owned(),
            outcome,
        })?;
        let script = Script::new(
            r#"
            if redis.call('GET', KEYS[1]) == ARGV[1] then
                redis.call('SET', KEYS[1], ARGV[2], 'EX', ARGV[3])
                return 1
            end
            return 0
            "#,
        );
        let mut connection = self.connection.clone();
        let updated: i32 = script
            .key(key)
            .arg(expected)
            .arg(completed)
            .arg(IDEMPOTENCY_TTL_SECS)
            .invoke_async(&mut connection)
            .await?;
        Ok(updated == 1)
    }

    /// Gives back the claim of an attempt the engine never executed. The claim is replaced, in
    /// one atomic step, by a short-lived release marker that keeps `reason`: requests already
    /// waiting on it learn they may retry and why, and a new request claims the key as if it
    /// were free.
    pub async fn release(
        &self,
        user_id: u64,
        idempotency_key: &str,
        fingerprint: &str,
        client_order_id: u64,
        reason: ReleaseReason,
    ) -> Result<bool, OrderIdempotencyError> {
        let key = redis_key(user_id, idempotency_key);
        let expected = serialize(&StoredOrderAttempt::InFlight {
            fingerprint: fingerprint.to_owned(),
            client_order_id,
        })?;
        let released = serialize(&StoredOrderAttempt::Released(reason))?;
        let script = Script::new(
            r#"
            if redis.call('GET', KEYS[1]) == ARGV[1] then
                redis.call('SET', KEYS[1], ARGV[2], 'EX', ARGV[3])
                return 1
            end
            return 0
            "#,
        );
        let mut connection = self.connection.clone();
        let released: i32 = script
            .key(key)
            .arg(expected)
            .arg(released)
            .arg(RELEASED_MARKER_TTL_SECS)
            .invoke_async(&mut connection)
            .await?;
        Ok(released == 1)
    }

    pub async fn wait_for_completion(
        &self,
        user_id: u64,
        idempotency_key: &str,
        fingerprint: &str,
        timeout: Duration,
    ) -> Result<CompletionWait, OrderIdempotencyError> {
        let key = redis_key(user_id, idempotency_key);
        let deadline = Instant::now() + timeout;
        let mut poll_interval = COMPLETION_POLL_INITIAL;

        loop {
            match self.lookup_state(&key).await? {
                Some(StoredOrderAttempt::Completed {
                    fingerprint: stored_fingerprint,
                    outcome,
                }) if stored_fingerprint == fingerprint => {
                    return Ok(CompletionWait::Completed(outcome))
                }
                Some(StoredOrderAttempt::InFlight {
                    fingerprint: stored_fingerprint,
                    ..
                }) if stored_fingerprint == fingerprint => {}
                Some(StoredOrderAttempt::Released(reason)) => {
                    return Ok(CompletionWait::Released(reason))
                }
                _ => return Ok(CompletionWait::Unresolved),
            }

            let now = Instant::now();
            if now >= deadline {
                return Ok(CompletionWait::Unresolved);
            }
            // Never sleep past the deadline: the last poll happens at it, not up to 500 ms later.
            tokio::time::sleep(poll_interval.min(deadline - now)).await;
            poll_interval = next_poll_interval(poll_interval);
        }
    }

    async fn lookup_state(
        &self,
        key: &str,
    ) -> Result<Option<StoredOrderAttempt>, OrderIdempotencyError> {
        let mut connection = self.connection.clone();
        let bytes: Option<Vec<u8>> = redis::cmd("GET")
            .arg(key)
            .query_async(&mut connection)
            .await?;
        bytes.map(|value| deserialize(&value)).transpose()
    }
}

/// The claim a request gets from the key's current state, or `None` for a release marker, which
/// leaves the key free.
fn claim_from_state(existing: StoredOrderAttempt, fingerprint: &str) -> Option<OrderClaim> {
    match existing {
        StoredOrderAttempt::InFlight {
            fingerprint: stored_fingerprint,
            client_order_id,
        } if stored_fingerprint == fingerprint => Some(OrderClaim::InFlight { client_order_id }),
        StoredOrderAttempt::Completed {
            fingerprint: stored_fingerprint,
            outcome,
        } if stored_fingerprint == fingerprint => Some(OrderClaim::Completed(outcome)),
        StoredOrderAttempt::Released(_) => None,
        _ => Some(OrderClaim::PayloadMismatch),
    }
}

fn next_poll_interval(interval: Duration) -> Duration {
    interval.saturating_mul(2).min(COMPLETION_POLL_MAX)
}

fn redis_key(user_id: u64, idempotency_key: &str) -> String {
    format!("vex:order-idempotency:{user_id}:{idempotency_key}")
}

fn serialize(value: &StoredOrderAttempt) -> Result<Vec<u8>, OrderIdempotencyError> {
    serde_json::to_vec(value)
        .map_err(|error| OrderIdempotencyError::Serialization(error.to_string()))
}

fn deserialize(bytes: &[u8]) -> Result<StoredOrderAttempt, OrderIdempotencyError> {
    serde_json::from_slice(bytes)
        .map_err(|error| OrderIdempotencyError::Serialization(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate_client_order_id;
    use redis::Client;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    /// The store on `REDIS_URL` (default local 6379), or `None` when Redis is unreachable so the
    /// calling test skips instead of failing a plain `cargo test`. CI sets `REDIS_URL` to its
    /// Redis service.
    async fn store() -> Option<OrderIdempotencyStore> {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let client = Client::open(redis_url.as_str()).expect("valid Redis URL");
        match ConnectionManager::new(client).await {
            Ok(connection) => Some(OrderIdempotencyStore::new(connection)),
            Err(error) => {
                eprintln!("Skipping live-Redis test: unable to connect to {redis_url}: {error}");
                None
            }
        }
    }

    fn outcome(client_order_id: u64) -> IdempotentOrderOutcome {
        IdempotentOrderOutcome {
            http_status: 200,
            order_id: 42,
            client_order_id,
            status: "Placed".to_owned(),
            message: "Order processed successfully".to_owned(),
        }
    }

    #[tokio::test]
    async fn same_key_replays_first_outcome_without_second_publish() {
        let Some(store) = store().await else { return };
        let user_id = generate_client_order_id();
        let key = format!("same-key-{}", generate_client_order_id());
        let client_order_id = generate_client_order_id();
        let publishes = AtomicUsize::new(0);

        assert_eq!(
            store
                .claim(user_id, &key, "payload", client_order_id)
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        publishes.fetch_add(1, Ordering::SeqCst);
        assert!(store
            .complete(user_id, &key, "payload", outcome(client_order_id))
            .await
            .unwrap());

        assert_eq!(
            store
                .claim(user_id, &key, "payload", generate_client_order_id())
                .await
                .unwrap(),
            OrderClaim::Completed(outcome(client_order_id))
        );
        assert_eq!(publishes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_retry_does_not_double_publish() {
        let Some(store) = store().await else { return };
        let store = Arc::new(store);
        let user_id = generate_client_order_id();
        let key = format!("concurrent-key-{}", generate_client_order_id());
        let publishes = Arc::new(AtomicUsize::new(0));

        let attempt = |client_order_id| {
            let store = store.clone();
            let key = key.clone();
            let publishes = publishes.clone();
            async move {
                match store
                    .claim(user_id, &key, "payload", client_order_id)
                    .await
                    .unwrap()
                {
                    OrderClaim::Acquired => {
                        publishes.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let result = outcome(client_order_id);
                        store
                            .complete(user_id, &key, "payload", result.clone())
                            .await
                            .unwrap();
                        result
                    }
                    OrderClaim::InFlight { .. } => match store
                        .wait_for_completion(user_id, &key, "payload", Duration::from_millis(500))
                        .await
                        .unwrap()
                    {
                        CompletionWait::Completed(outcome) => outcome,
                        unexpected => panic!("unexpected wait result: {unexpected:?}"),
                    },
                    unexpected => panic!("unexpected claim: {unexpected:?}"),
                }
            }
        };

        let (first, second) = tokio::join!(
            attempt(generate_client_order_id()),
            attempt(generate_client_order_id())
        );
        assert_eq!(first, second);
        assert_eq!(publishes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_key_places_new_order() {
        let Some(store) = store().await else { return };
        let user_id = generate_client_order_id();
        let first_id = generate_client_order_id();
        let second_id = generate_client_order_id();
        let publishes = AtomicUsize::new(0);

        assert_eq!(
            store
                .claim(user_id, "different-a", "payload", first_id)
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        publishes.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            store
                .claim(user_id, "different-b", "payload", second_id)
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        publishes.fetch_add(1, Ordering::SeqCst);
        assert_eq!(publishes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn claim_expires() {
        let Some(store) = store().await else { return };
        let user_id = generate_client_order_id();
        let key = format!("expiry-key-{}", generate_client_order_id());

        assert_eq!(
            store
                .claim(user_id, &key, "payload", generate_client_order_id())
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert_eq!(
            store
                .claim(user_id, &key, "payload", generate_client_order_id())
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
    }

    #[tokio::test]
    async fn unsent_attempt_can_release_its_claim() {
        let Some(store) = store().await else { return };
        for reason in [ReleaseReason::Busy, ReleaseReason::EngineUnavailable] {
            let user_id = generate_client_order_id();
            let key = format!("release-key-{}", generate_client_order_id());
            let first_id = generate_client_order_id();

            assert_eq!(
                store
                    .claim(user_id, &key, "payload", first_id)
                    .await
                    .unwrap(),
                OrderClaim::Acquired
            );
            assert!(store
                .release(user_id, &key, "payload", first_id, reason)
                .await
                .unwrap());
            assert_eq!(
                store
                    .claim(user_id, &key, "payload", generate_client_order_id())
                    .await
                    .unwrap(),
                OrderClaim::Acquired,
                "a {reason:?} release marker must leave the key free"
            );
        }
    }

    #[tokio::test]
    async fn waiter_on_a_released_claim_learns_why_and_the_retry_claims_the_key() {
        let Some(store) = store().await else { return };
        for reason in [ReleaseReason::Busy, ReleaseReason::EngineUnavailable] {
            let user_id = generate_client_order_id();
            let key = format!("released-key-{}", generate_client_order_id());
            let first_id = generate_client_order_id();
            assert_eq!(
                store
                    .claim(user_id, &key, "payload", first_id)
                    .await
                    .unwrap(),
                OrderClaim::Acquired
            );

            // A same-key request is already waiting when the engine turns the attempt away.
            let waiter = {
                let store = store.clone();
                let key = key.clone();
                tokio::spawn(async move {
                    store
                        .wait_for_completion(user_id, &key, "payload", Duration::from_secs(2))
                        .await
                })
            };
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(store
                .release(user_id, &key, "payload", first_id, reason)
                .await
                .unwrap());

            assert_eq!(
                waiter.await.unwrap().unwrap(),
                CompletionWait::Released(reason)
            );
            let mut connection = store.connection.clone();
            let marker_ttl: i64 = redis::cmd("TTL")
                .arg(redis_key(user_id, &key))
                .query_async(&mut connection)
                .await
                .unwrap();
            assert!(
                (1..=RELEASED_MARKER_TTL_SECS as i64).contains(&marker_ttl),
                "release marker TTL {marker_ttl}"
            );
            // The marker never blocks the retry: it claims the key as if it were free.
            assert_eq!(
                store
                    .claim(user_id, &key, "payload", generate_client_order_id())
                    .await
                    .unwrap(),
                OrderClaim::Acquired
            );
        }
    }

    #[tokio::test]
    async fn waiter_on_a_vanished_claim_stays_unresolved() {
        let Some(store) = store().await else { return };
        let user_id = generate_client_order_id();
        let key = format!("vanished-key-{}", generate_client_order_id());
        assert_eq!(
            store
                .claim(user_id, &key, "payload", generate_client_order_id())
                .await
                .unwrap(),
            OrderClaim::Acquired
        );
        // Eviction or failover drops the claim without a release marker: the order may have been
        // published, so the waiter must not be told to retry.
        let mut connection = store.connection.clone();
        let _: () = redis::cmd("DEL")
            .arg(redis_key(user_id, &key))
            .query_async(&mut connection)
            .await
            .unwrap();

        assert_eq!(
            store
                .wait_for_completion(user_id, &key, "payload", Duration::from_secs(1))
                .await
                .unwrap(),
            CompletionWait::Unresolved
        );
    }

    #[test]
    fn completion_poll_backs_off_to_a_cap() {
        let mut interval = COMPLETION_POLL_INITIAL;
        let mut schedule = vec![interval];
        for _ in 0..6 {
            interval = next_poll_interval(interval);
            schedule.push(interval);
        }
        assert_eq!(
            schedule,
            [25, 50, 100, 200, 400, 500, 500].map(Duration::from_millis)
        );
    }

    #[tokio::test]
    async fn waiter_gives_up_at_its_deadline_not_after_a_full_backoff_step() {
        let Some(store) = store().await else { return };
        let user_id = generate_client_order_id();
        let key = format!("deadline-key-{}", generate_client_order_id());
        assert_eq!(
            store
                .claim(user_id, &key, "payload", generate_client_order_id())
                .await
                .unwrap(),
            OrderClaim::Acquired
        );

        // Unclamped, the 25+50+100+200 ms steps would be followed by a full 400 ms sleep,
        // returning at ~775 ms.
        let timeout = Duration::from_millis(400);
        let started = Instant::now();
        let outcome = store
            .wait_for_completion(user_id, &key, "payload", timeout)
            .await
            .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(outcome, CompletionWait::Unresolved);
        assert!(elapsed >= timeout, "returned early after {elapsed:?}");
        assert!(
            elapsed < timeout + Duration::from_millis(250),
            "overshot the deadline: {elapsed:?}"
        );
    }
}

//! The gateway's link to vex-core: the publisher requests send through, and the supervisor that
//! establishes the link in the background and establishes it again after it is lost, such as
//! when the core restarts.

use crate::correlation::CorrelationService;
use arc_swap::ArcSwapOption;
use common::OrderCommand;
use rand::Rng;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use vex_networking::client::{GatewayError, Publisher};

/// How often the supervisor checks an established link: one atomic read of the publication's
/// connection flag, which Aeron clears a few seconds after the core stops reading it.
const LINK_CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// The delay before the first retry, doubled after each further failure up to
/// `RECONNECT_BACKOFF_CAP`. An attempt against a core that is down already waits up to 20 s for
/// it, so the delay mostly paces the attempts the core turns away at once, such as while it
/// still holds the session of the link this gateway lost.
const RECONNECT_BACKOFF_BASE: Duration = Duration::from_millis(500);
const RECONNECT_BACKOFF_CAP: Duration = Duration::from_secs(10);

/// How long the supervisor waits between checks for the requests still offering on a lost
/// publisher. Each holds it for one non-blocking offer.
const RELEASE_CHECK_INTERVAL: Duration = Duration::from_millis(1);

/// The publication end of an established link, shared with the request path.
pub trait LinkPublisher: Send + Sync + 'static {
    /// False once no core reads the publication: the link is lost.
    fn is_connected(&self) -> bool;
}

impl LinkPublisher for Publisher {
    fn is_connected(&self) -> bool {
        self.publication.is_connected()
    }
}

/// The publisher of the current link to vex-core, or none while [`EngineLinkSupervisor`]
/// connects. Requests read it without a lock, and no method hands it out, so no request holds it
/// beyond one call; only the supervisor swaps it.
pub struct EngineLink<P = Publisher> {
    publisher: Arc<ArcSwapOption<P>>,
}

impl<P> Default for EngineLink<P> {
    /// A link that is not connected yet.
    fn default() -> Self {
        Self {
            publisher: Arc::new(ArcSwapOption::empty()),
        }
    }
}

impl<P> Clone for EngineLink<P> {
    fn clone(&self) -> Self {
        Self {
            publisher: Arc::clone(&self.publisher),
        }
    }
}

impl<P: LinkPublisher> EngineLink<P> {
    /// Whether a link is established and the core reads its publication.
    pub fn is_connected(&self) -> bool {
        self.publisher
            .load()
            .as_ref()
            .is_some_and(|publisher| publisher.is_connected())
    }
}

impl EngineLink {
    /// A link that is not connected yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Offers the command once on the current link, never blocking: `NotConnected` while there is
    /// none, as when its core does not read it, otherwise what
    /// [`Publisher::try_send_order_command`] returns.
    pub fn try_send_order_command(&self, command: &OrderCommand) -> Result<(), GatewayError> {
        match &*self.publisher.load() {
            Some(publisher) => publisher.try_send_order_command(command),
            None => Err(GatewayError::NotConnected),
        }
    }
}

/// Opens links to vex-core for [`EngineLinkSupervisor`] and releases the lost ones.
pub trait CoreConnector {
    type Publisher: LinkPublisher;
    /// What the link needs besides its publisher, such as the thread polling the core's
    /// responses.
    type Session;

    /// Handshakes with the core and opens the link's channels. Blocks until they are up or the
    /// attempt fails.
    fn connect(&mut self) -> Result<(Self::Session, Self::Publisher), GatewayError>;

    /// Releases a lost link and stops delivering its responses. No request holds its publisher any
    /// more.
    fn close(&mut self, session: Self::Session, publisher: Self::Publisher);
}

/// Keeps the engine link up, on a thread of its own because connecting blocks. It connects,
/// checks the link every `LINK_CHECK_INTERVAL`, and after a failed attempt or a lost link waits a
/// capped, jittered, exponentially growing delay before the next handshake. A link is lost once
/// the core no longer reads its publication, or once its responses keep timing out (see
/// [`CorrelationService::is_engine_link_healthy`]): a link that no longer answers is as useless
/// as one that is gone. The process serves requests throughout; order requests get
/// `NotConnected` while there is no link.
pub struct EngineLinkSupervisor<C: CoreConnector> {
    connector: C,
    link: EngineLink<C::Publisher>,
    correlation_service: CorrelationService,
    backoff: Backoff,
    established: Option<(C::Session, Arc<C::Publisher>)>,
}

impl<C: CoreConnector> EngineLinkSupervisor<C> {
    /// A supervisor for `link`, which is not connected yet. A lost link fails the requests
    /// waiting in `correlation_service`.
    pub fn new(
        connector: C,
        link: EngineLink<C::Publisher>,
        correlation_service: CorrelationService,
    ) -> Self {
        Self {
            connector,
            link,
            correlation_service,
            backoff: Backoff::default(),
            established: None,
        }
    }

    /// Supervises the link for the life of the process, blocking the calling thread. Returns only
    /// by panicking.
    pub fn run(mut self) {
        loop {
            let wait = self.step();
            std::thread::sleep(wait);
        }
    }

    /// One supervision step: connect when there is no link, otherwise check it and release it if
    /// it is lost. Returns how long to wait before the next step.
    fn step(&mut self) -> Duration {
        let Some((session, publisher)) = self.established.take() else {
            return self.connect();
        };
        let reason = if !publisher.is_connected() {
            "the core no longer reads this gateway's publication"
        } else if !self.correlation_service.is_engine_link_healthy() {
            "responses from the core kept timing out"
        } else {
            self.established = Some((session, publisher));
            return LINK_CHECK_INTERVAL;
        };
        warn!(reason, "Lost the link to vex-core; reconnecting");
        self.release(session, publisher);
        self.backoff.next_delay()
    }

    fn connect(&mut self) -> Duration {
        match self.connector.connect() {
            Ok((session, publisher)) => {
                let publisher = Arc::new(publisher);
                self.link.publisher.store(Some(Arc::clone(&publisher)));
                self.established = Some((session, publisher));
                self.backoff.reset();
                info!("Connected to vex-core");
                LINK_CHECK_INTERVAL
            }
            Err(error) => {
                let retry_in = self.backoff.next_delay();
                warn!(
                    error = %error,
                    retry_in = ?retry_in,
                    "Could not connect to vex-core; retrying"
                );
                retry_in
            }
        }
    }

    /// Takes a lost link out of service: new requests get `NotConnected`, the link is closed once
    /// no request offers on its publisher any more, and then the requests still waiting for a
    /// response on it fail at once. Closing first delivers the responses the link polled before it
    /// stopped.
    fn release(&mut self, session: C::Session, publisher: Arc<C::Publisher>) {
        self.link.publisher.store(None);
        let publisher = sole_owner(publisher);
        self.connector.close(session, publisher);
        self.correlation_service.link_lost();
    }
}

/// Waits until `publisher` is held only here, then takes it. The requests that loaded it before
/// it left the link each hold it for one non-blocking offer, so this returns almost at once, and
/// the publisher is then dropped by the connector on this thread rather than on a request thread.
fn sole_owner<P>(mut publisher: Arc<P>) -> P {
    loop {
        match Arc::try_unwrap(publisher) {
            Ok(publisher) => return publisher,
            Err(shared) => {
                publisher = shared;
                std::thread::sleep(RELEASE_CHECK_INTERVAL);
            }
        }
    }
}

/// Capped exponential backoff with equal jitter: the n-th delay in a row is drawn from [d/2, d]
/// for d = min(`RECONNECT_BACKOFF_BASE` * 2^(n-1), `RECONNECT_BACKOFF_CAP`). The jitter spreads
/// the handshakes of the gateways that lost the same core; the fixed half keeps a retry from
/// following a failure at once.
#[derive(Default)]
struct Backoff {
    failures: u32,
}

impl Backoff {
    fn next_delay(&mut self) -> Duration {
        let ceiling = RECONNECT_BACKOFF_BASE
            .saturating_mul(1 << self.failures.min(16))
            .min(RECONNECT_BACKOFF_CAP);
        self.failures = self.failures.saturating_add(1);
        rand::thread_rng().gen_range(ceiling / 2..=ceiling)
    }

    fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::oneshot::error::TryRecvError;

    const MS: Duration = Duration::from_millis(1);

    /// Reads the connection flag of the fake core's current link.
    struct FakePublisher {
        core_reads: Arc<AtomicBool>,
    }

    impl LinkPublisher for FakePublisher {
        fn is_connected(&self) -> bool {
            self.core_reads.load(Ordering::SeqCst)
        }
    }

    /// Accepts or refuses each handshake as scripted. Sessions are numbered from 1.
    struct FakeConnector {
        handshakes: VecDeque<bool>,
        core_reads: Arc<AtomicBool>,
        sessions: u32,
        closed: Vec<u32>,
    }

    impl CoreConnector for FakeConnector {
        type Publisher = FakePublisher;
        type Session = u32;

        fn connect(&mut self) -> Result<(u32, FakePublisher), GatewayError> {
            if !self
                .handshakes
                .pop_front()
                .expect("an unscripted handshake")
            {
                return Err(GatewayError::Timeout(
                    "Waiting for core handshake response".to_string(),
                ));
            }
            self.sessions += 1;
            self.core_reads.store(true, Ordering::SeqCst);
            let publisher = FakePublisher {
                core_reads: Arc::clone(&self.core_reads),
            };
            Ok((self.sessions, publisher))
        }

        fn close(&mut self, session: u32, _publisher: FakePublisher) {
            self.closed.push(session);
        }
    }

    fn assert_between(delay: Duration, low: Duration, high: Duration) {
        assert!(
            low <= delay && delay <= high,
            "{delay:?} is outside [{low:?}, {high:?}]"
        );
    }

    #[test]
    fn supervisor_backs_off_connects_and_reconnects_after_losing_the_link() {
        let correlation_service = CorrelationService::new();
        let link = EngineLink::<FakePublisher>::default();
        let core_reads = Arc::new(AtomicBool::new(false));
        let connector = FakeConnector {
            handshakes: VecDeque::from([false, false, true, true]),
            core_reads: Arc::clone(&core_reads),
            sessions: 0,
            closed: Vec::new(),
        };
        let mut supervisor =
            EngineLinkSupervisor::new(connector, link.clone(), correlation_service.clone());

        // Disconnected: each refused handshake backs off longer.
        assert!(!link.is_connected());
        assert_between(supervisor.step(), 250 * MS, 500 * MS);
        assert_between(supervisor.step(), 500 * MS, 1000 * MS);
        assert!(!link.is_connected());

        // Handshake accepted: requests can send, and the link is checked from now on.
        assert_eq!(supervisor.step(), LINK_CHECK_INTERVAL);
        assert!(link.is_connected());
        assert_eq!(supervisor.step(), LINK_CHECK_INTERVAL);
        assert!(supervisor.connector.closed.is_empty());

        // Lost: the link leaves service, the request waiting on it fails at once, the session is
        // closed, and the backoff starts over.
        let mut waiting = correlation_service.register_request(7);
        core_reads.store(false, Ordering::SeqCst);
        assert_between(supervisor.step(), 250 * MS, 500 * MS);
        assert!(link.publisher.load().is_none());
        assert_eq!(supervisor.connector.closed, [1]);
        assert!(matches!(waiting.try_recv(), Err(TryRecvError::Closed)));
        assert_eq!(correlation_service.pending_count(), 0);

        // Handshake again: a new session serves the link.
        assert_eq!(supervisor.step(), LINK_CHECK_INTERVAL);
        assert!(link.is_connected());
        assert_eq!(supervisor.connector.sessions, 2);
        assert_eq!(supervisor.connector.closed, [1]);
    }

    #[test]
    fn supervisor_reconnects_a_link_whose_responses_keep_timing_out() {
        let correlation_service = CorrelationService::new();
        let link = EngineLink::<FakePublisher>::default();
        let connector = FakeConnector {
            handshakes: VecDeque::from([true, true]),
            core_reads: Arc::new(AtomicBool::new(false)),
            sessions: 0,
            closed: Vec::new(),
        };
        let mut supervisor =
            EngineLinkSupervisor::new(connector, link.clone(), correlation_service.clone());
        assert_eq!(supervisor.step(), LINK_CHECK_INTERVAL);

        // The core still reads the publication, but no response has come back for 10 s.
        let t0 = std::time::Instant::now();
        correlation_service.timeout_request_at(1, t0);
        correlation_service.timeout_request_at(2, t0 + 10_000 * MS);
        assert!(link.is_connected());

        assert_between(supervisor.step(), 250 * MS, 500 * MS);
        assert!(!link.is_connected());
        assert_eq!(supervisor.connector.closed, [1]);
        // The timeouts were the lost link's, so the next link starts with a clean record.
        assert!(correlation_service.is_engine_link_healthy());

        assert_eq!(supervisor.step(), LINK_CHECK_INTERVAL);
        assert!(link.is_connected());
        assert_eq!(supervisor.step(), LINK_CHECK_INTERVAL);
        assert_eq!(supervisor.connector.closed, [1]);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap_with_jitter_and_starts_over_after_a_reset() {
        let mut backoff = Backoff::default();
        for ceiling_ms in [500, 1000, 2000, 4000, 8000, 10_000, 10_000] {
            assert_between(backoff.next_delay(), ceiling_ms / 2 * MS, ceiling_ms * MS);
        }
        for _ in 0..100 {
            assert_between(backoff.next_delay(), 5000 * MS, 10_000 * MS);
        }

        backoff.reset();

        assert_between(backoff.next_delay(), 250 * MS, 500 * MS);
    }

    #[test]
    fn lost_publisher_is_taken_only_after_the_request_offering_on_it_is_done() {
        let publisher = Arc::new(AtomicBool::new(false));
        let offering = Arc::clone(&publisher);
        let request = std::thread::spawn(move || {
            std::thread::sleep(20 * MS);
            offering.store(true, Ordering::SeqCst);
        });

        let publisher = sole_owner(publisher);

        assert!(publisher.load(Ordering::SeqCst));
        request.join().unwrap();
    }

    #[test]
    fn link_without_a_connection_refuses_to_send() {
        let link = EngineLink::new();

        assert!(!link.is_connected());
        assert!(matches!(
            link.try_send_order_command(&OrderCommand::default()),
            Err(GatewayError::NotConnected)
        ));
    }
}

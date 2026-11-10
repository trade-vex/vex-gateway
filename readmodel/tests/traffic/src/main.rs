//! Drives the read-model test stack's vex-core through every event topic the read model stores:
//! deposits, resting orders, fills, cancels and withdrawals.
//!
//! It runs as the `traffic` service of `readmodel/tests/compose.yml`, with an embedded Aeron
//! media driver in its own container, and exits non-zero as soon as the core answers a command
//! with an unexpected status or not at all. Orders are limit orders only: a replayed market buy
//! can lock a different price than the original run (vex-core#140 item 2).

use common::{OrderCommand, Side, Status, TimeInForce};
use rusteron_media_driver::{AeronDriver, AeronDriverContext};
use std::error::Error;
use std::ffi::CString;
use std::net::ToSocketAddrs;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;
use vex_config::GatewayNetworkingConfig;
use vex_networking::client::{OrderCommandHandler, Publisher, VexGateway};

/// The `core` service and its `VEX_SERVER_PORT` (the control port is the next one).
const CORE_HOST: &str = "core";
const CORE_PORT: u16 = 3521;
const AERON_DIR: &str = "/dev/shm/aeron-readmodel-traffic";
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

const USD: u16 = 1;
const BTC: u16 = 2;
const ETH: u16 = 3;
const BTC_USD: u32 = ((USD as u32) << 16) | BTC as u32;

/// Above `i64::MAX`, so its rows show that user ids keep their bits.
const MAKER: u64 = (1 << 63) | 1;
const TAKER: u64 = 2;
/// Deposits 10^19 ETH units, a balance above `i64::MAX` that the type test reads back.
const WHALE: u64 = 3;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> Result<()> {
    let driver_context = AeronDriverContext::new()?;
    driver_context.set_dir(&CString::new(AERON_DIR)?)?;
    driver_context.set_dir_delete_on_start(true)?;
    driver_context.set_dir_delete_on_shutdown(true)?;
    let (driver_stop, driver) = AeronDriver::launch_embedded(driver_context, false);

    let result = drive();

    driver_stop.store(true, Ordering::SeqCst);
    driver.join().map_err(|_| "the media driver panicked")??;
    result
}

fn drive() -> Result<()> {
    let mut core = Core::connect()?;

    core.execute(
        OrderCommand::deposit_funds(MAKER, 1_000_000_000_000, USD),
        Status::Processed,
    )?;
    core.execute(
        OrderCommand::deposit_funds(TAKER, 1_000_000_000, BTC),
        Status::Processed,
    )?;
    core.execute(
        OrderCommand::deposit_funds(WHALE, 10_000_000_000_000_000_000, ETH),
        Status::Processed,
    )?;

    for round in 0..3 {
        let price = 50_000 + round;
        let bid = core.execute(
            OrderCommand::place_order(
                TimeInForce::Gtc,
                MAKER,
                price,
                100_000,
                Side::Bid,
                BTC_USD,
                0,
            ),
            Status::Placed,
        )?;
        core.execute(
            OrderCommand::place_order(
                TimeInForce::Gtc,
                TAKER,
                price,
                40_000,
                Side::Ask,
                BTC_USD,
                0,
            ),
            Status::Filled,
        )?;
        core.execute(
            OrderCommand::cancel_order(bid.order_id, MAKER, Side::Bid, BTC_USD),
            Status::Cancelled,
        )?;
    }

    core.execute(
        OrderCommand::withdraw_funds(TAKER, 1_000, BTC),
        Status::Processed,
    )?;
    core.execute(
        OrderCommand::withdraw_funds(MAKER, 1_000, USD),
        Status::Processed,
    )?;
    Ok(())
}

struct Core {
    publisher: Publisher,
    /// Owns the Aeron client the publisher sends through.
    _gateway: VexGateway,
    responses: Receiver<OrderCommand>,
    last_client_order_id: u64,
}

impl Core {
    fn connect() -> Result<Self> {
        // The Aeron C client does not resolve container names.
        let address = (CORE_HOST, CORE_PORT)
            .to_socket_addrs()?
            .find(|address| address.is_ipv4())
            .ok_or("the core's address does not resolve")?;
        let mut config = GatewayNetworkingConfig::development_defaults();
        config.context_dir = AERON_DIR.to_string();
        config.core_address = address.ip().to_string();
        config.core_port = CORE_PORT;
        config.core_control_port = CORE_PORT + 1;

        let mut gateway = VexGateway::new(config)?;
        let (sender, responses) = mpsc::channel();
        let publisher = gateway.start(OrderCommandHandler::new(gateway.gateway_id(), sender))?;
        Ok(Self {
            publisher,
            _gateway: gateway,
            responses,
            last_client_order_id: 0,
        })
    }

    /// Sends `command` and waits for its response, which must have status `expected`.
    fn execute(&mut self, mut command: OrderCommand, expected: Status) -> Result<OrderCommand> {
        self.last_client_order_id += 1;
        command.client_order_id = self.last_client_order_id;
        self.publisher.send_order_command(&command)?;
        loop {
            let response = self.responses.recv_timeout(RESPONSE_TIMEOUT).map_err(|_| {
                format!(
                    "no response to {:?} for user {}",
                    command.command, command.user_id
                )
            })?;
            if response.client_order_id != command.client_order_id {
                println!(
                    "ignoring a response to client order {}",
                    response.client_order_id
                );
                continue;
            }
            if response.status != expected {
                return Err(format!(
                    "{:?} for user {} answered {:?}, expected {expected:?}",
                    command.command, command.user_id, response.status
                )
                .into());
            }
            println!(
                "{:?} user={} order={} -> {:?}",
                command.command, command.user_id, response.order_id, response.status
            );
            return Ok(response);
        }
    }
}

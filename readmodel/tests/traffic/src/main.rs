//! Drives the read-model test stack's vex-core, as the `traffic` service of
//! `readmodel/tests/compose.yml`, with an embedded Aeron media driver in its own container:
//!
//! - `readmodel-traffic coverage`: every event topic the read model stores: deposits, resting
//!   orders, fills, cancels and withdrawals.
//! - `readmodel-traffic ledger <gateway id>`: one more round of the acceptance ledger, read from
//!   a line of stdin (`{}` for none yet) and written to stdout as a line: the latest event, per
//!   key of the latest-state topics, that the core's answers say it published (`Ledger`). The
//!   gateway then stays connected until stdin closes.
//!
//! It exits non-zero as soon as the core answers a command with an unexpected status or not at
//! all, and reports each command on stderr. Orders are limit orders only: a replayed market buy
//! can lock a different price than the original run (vex-core#140 item 2).

use common::{
    CoreMarketSpecification, L2SIZE, OrderCommand, Side, Status, TimeInForce, base_asset,
    quote_asset,
};
use rusteron_media_driver::{AeronDriver, AeronDriverContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::CString;
use std::net::ToSocketAddrs;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use vex_config::{Environment, GatewayNetworkingConfig, VexConfig};
use vex_networking::client::{OrderCommandHandler, Publisher, VexGateway};

/// The core's address: it shares the network namespace of the `aeron-archive` service. The
/// control port is the next one after `VEX_SERVER_PORT`.
const CORE_HOST: &str = "aeron-archive";
const CORE_PORT: u16 = 3521;
const AERON_DIR: &str = "/dev/shm/aeron-readmodel-traffic";
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a connection waits for the core, which accepts gateways only once it has replayed
/// its journal and re-published what it must.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

const USD: u16 = 1;
const BTC: u16 = 2;
const ETH: u16 = 3;
const BTC_USD: u32 = ((USD as u32) << 16) | BTC as u32;
const ETH_USD: u32 = ((USD as u32) << 16) | ETH as u32;

// coverage
/// Above `i64::MAX`, so its rows show that user ids keep their bits.
const MAKER: u64 = (1 << 63) | 1;
const TAKER: u64 = 2;
/// Deposits 10^19 ETH units, a balance above `i64::MAX` that the type test reads back.
const WHALE: u64 = 3;
/// The gateway id of the coverage run.
const COVERAGE_GATEWAY: u8 = 1;

// ledger: two traders whose resting ETH/USD orders never cross, cancels and withdrawals. The
// ledger's book of ETH/USD holds its own orders only (`Ledger::book`), so no other traffic of
// the stack may rest an order there: coverage trades BTC/USD, and only deposits ETH.
const BIDDER: u64 = 101;
const ASKER: u64 = 102;
/// Order pairs a round places: enough that some of their events are still queued in the core
/// when it is killed behind paused brokers.
const PAIRS_PER_ROUND: u64 = 10;
/// 0.01 ETH.
const ORDER_SIZE: u64 = 10_000_000_000_000_000;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> Result<()> {
    let driver_context = AeronDriverContext::new()?;
    driver_context.set_dir(&CString::new(AERON_DIR)?)?;
    driver_context.set_dir_delete_on_start(true)?;
    driver_context.set_dir_delete_on_shutdown(true)?;
    let (driver_stop, driver) = AeronDriver::launch_embedded(driver_context, false);

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["coverage"] => coverage(),
        ["ledger", gateway_id] => ledger_round(gateway_id.parse()?),
        _ => {
            Err("usage: readmodel-traffic coverage | readmodel-traffic ledger <gateway id>".into())
        }
    };

    driver_stop.store(true, Ordering::SeqCst);
    driver.join().map_err(|_| "the media driver panicked")??;
    result
}

fn coverage() -> Result<()> {
    let mut core = Core::connect(COVERAGE_GATEWAY)?;

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

/// Continues the ledger on a line of stdin by one round, and writes it to stdout as a line: the
/// first round funds the traders; every round places `PAIRS_PER_ROUND` bids and asks that never
/// cross, cancels its first bid and ask, and withdraws. Then stays connected until stdin closes,
/// so that the core can be stopped with a gateway connected, as production gateways always are.
fn ledger_round(gateway_id: u8) -> Result<()> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let mut ledger: Ledger = serde_json::from_str(&line)?;
    let market = VexConfig::new(Environment::Development)
        .symbols
        .symbols
        .get(&ETH_USD)
        .cloned()
        .ok_or("ETH/USD is not a development market")?;
    let mut core = Core::connect(gateway_id)?;

    if ledger.pairs == 0 {
        for (user, amount, asset) in [
            (BIDDER, 1_000_000_000_000, USD),
            (ASKER, 5_000_000_000_000_000_000, ETH),
        ] {
            let response = core.execute(
                OrderCommand::deposit_funds(user, amount, asset),
                Status::Processed,
            )?;
            ledger.funds(&response, |available| available + amount);
        }
    }

    let mut placed = Vec::new();
    for _ in 0..PAIRS_PER_ROUND {
        ledger.pairs += 1;
        for (user, side, price) in [
            (BIDDER, Side::Bid, 39_000 - ledger.pairs),
            (ASKER, Side::Ask, 41_000 + ledger.pairs),
        ] {
            let response = core.execute(
                OrderCommand::place_order(
                    TimeInForce::Gtc,
                    user,
                    price,
                    ORDER_SIZE,
                    side,
                    ETH_USD,
                    0,
                ),
                Status::Placed,
            )?;
            ledger.placed(&response, &market)?;
            placed.push(response);
        }
    }
    for order in &placed[..2] {
        let response = core.execute(
            OrderCommand::cancel_order(order.order_id, order.user_id, order.side, ETH_USD),
            Status::Cancelled,
        )?;
        ledger.cancelled(&response, &market)?;
    }
    let amount = 1_000_000;
    let response = core.execute(
        OrderCommand::withdraw_funds(BIDDER, amount, USD),
        Status::Processed,
    )?;
    ledger.funds(&response, |available| available - amount);

    println!("{}", serde_json::to_string(&ledger)?);
    std::io::copy(&mut std::io::stdin(), &mut std::io::sink())?;
    drop(core);
    Ok(())
}

/// What the core's answers say it published last for each key of the latest-state topics, by
/// topic and Kafka key, in proto3 JSON: the read model's latest-state rows, and Kafka's latest
/// event per key, must equal it.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Ledger {
    /// Order pairs placed so far, which also numbers their prices.
    pairs: u64,
    balances: BTreeMap<String, BalanceEvent>,
    orders: BTreeMap<String, OrderEvent>,
    orderbook: BTreeMap<String, OrderbookEvent>,
}

/// trading.proto's events, in proto3 JSON.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BalanceEvent {
    user_id: u64,
    asset_id: u32,
    available: u64,
    locked: u64,
    total: u64,
    timestamp: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrderEvent {
    order: Order,
    market_id: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Order {
    order_id: u64,
    user_id: u64,
    price: u64,
    size: u64,
    filled_size: u64,
    side: String,
    time_in_force: String,
    status: String,
    timestamp: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrderbookEvent {
    market_id: u32,
    bids: String,
    asks: String,
    timestamp: u64,
}

/// One order-book level, as the core writes the levels of an `OrderbookEvent` in JSON.
#[derive(Serialize)]
struct Level {
    price: u64,
    size: u64,
}

impl Ledger {
    /// The command stamped `timestamp` published `user`'s balance of `asset`, with what
    /// `change` makes of its (available, locked).
    fn balance(
        &mut self,
        user: u64,
        asset: u16,
        timestamp: u64,
        change: impl FnOnce(&mut u64, &mut u64),
    ) {
        let balance = self
            .balances
            .entry(format!("{user}:{asset}"))
            .or_insert_with(|| BalanceEvent {
                user_id: user,
                asset_id: asset.into(),
                ..BalanceEvent::default()
            });
        change(&mut balance.available, &mut balance.locked);
        balance.total = balance.available + balance.locked;
        balance.timestamp = timestamp;
    }

    /// A processed deposit or withdrawal, whose answer names the asset as its market: the
    /// available balance becomes `change` of it.
    fn funds(&mut self, response: &OrderCommand, change: impl FnOnce(u64) -> u64) {
        let asset = response.market_id as u16;
        self.balance(
            response.user_id,
            asset,
            response.timestamp,
            |available, _| *available = change(*available),
        );
    }

    /// What a resting order locks: a bid its quote cost, an ask its size of the base asset.
    /// Returns the locked asset, the market's other asset and the amount.
    fn lock(
        order: &Order,
        market_id: u32,
        market: &CoreMarketSpecification,
    ) -> Result<(u16, u16, u64)> {
        let (base, quote) = (base_asset(market_id), quote_asset(market_id));
        Ok(if order.side == "SIDE_BID" {
            let cost = market
                .calculate_quote_cost(order.price, order.size - order.filled_size)
                .ok_or("an order's quote cost overflows")?;
            (quote, base, cost)
        } else {
            (base, quote, order.size - order.filled_size)
        })
    }

    /// A placed limit order, which rests unfilled: its owner's balances of both assets (one
    /// with the order's lock), the order and the order book.
    fn placed(&mut self, response: &OrderCommand, market: &CoreMarketSpecification) -> Result<()> {
        let order = Order {
            order_id: response.order_id,
            user_id: response.user_id,
            price: response.price,
            size: response.size,
            filled_size: 0,
            side: side_name(response.side).to_string(),
            time_in_force: "TIF_GTC".to_string(),
            status: "STATUS_PLACED".to_string(),
            timestamp: response.timestamp,
        };
        let (locked_asset, other_asset, lock) = Self::lock(&order, response.market_id, market)?;
        self.balance(
            order.user_id,
            locked_asset,
            order.timestamp,
            |available, locked| {
                *available -= lock;
                *locked += lock;
            },
        );
        self.balance(order.user_id, other_asset, order.timestamp, |_, _| {});
        self.orders.insert(
            order.order_id.to_string(),
            OrderEvent {
                order,
                market_id: response.market_id,
            },
        );
        self.book(response.market_id, response.timestamp);
        Ok(())
    }

    /// A cancelled resting order: its lock returns to its owner, and the book loses it.
    fn cancelled(
        &mut self,
        response: &OrderCommand,
        market: &CoreMarketSpecification,
    ) -> Result<()> {
        let event = self
            .orders
            .get_mut(&response.order_id.to_string())
            .ok_or("a cancelled order the ledger did not place")?;
        event.order.status = "STATUS_CANCELLED".to_string();
        event.order.timestamp = response.timestamp;
        let (user, timestamp) = (event.order.user_id, event.order.timestamp);
        let (locked_asset, other_asset, lock) = Self::lock(&event.order, event.market_id, market)?;
        self.balance(user, locked_asset, timestamp, |available, locked| {
            *available += lock;
            *locked -= lock;
        });
        self.balance(user, other_asset, timestamp, |_, _| {});
        self.book(response.market_id, timestamp);
        Ok(())
    }

    /// The market's book after the command stamped `timestamp`, as the core snapshots it: at
    /// most `L2SIZE` levels a side, best first, each the resting size at its price. It is built
    /// from the ledger's orders alone: the ledger owns the market (see `BIDDER`).
    fn book(&mut self, market_id: u32, timestamp: u64) {
        let (mut bids, mut asks) = (BTreeMap::new(), BTreeMap::new());
        for event in self.orders.values() {
            let order = &event.order;
            if event.market_id == market_id && order.status == "STATUS_PLACED" {
                let levels = if order.side == "SIDE_BID" {
                    &mut bids
                } else {
                    &mut asks
                };
                *levels.entry(order.price).or_insert(0) += order.size - order.filled_size;
            }
        }
        let levels = |levels: Vec<(&u64, &u64)>| {
            let levels: Vec<_> = levels
                .into_iter()
                .take(L2SIZE)
                .map(|(&price, &size)| Level { price, size })
                .collect();
            serde_json::to_string(&levels).expect("levels serialize")
        };
        self.orderbook.insert(
            market_id.to_string(),
            OrderbookEvent {
                market_id,
                bids: levels(bids.iter().rev().collect()),
                asks: levels(asks.iter().collect()),
                timestamp,
            },
        );
    }
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Bid => "SIDE_BID",
        Side::Ask => "SIDE_ASK",
    }
}

struct Core {
    publisher: Publisher,
    /// Owns the Aeron client the publisher sends through.
    _gateway: VexGateway,
    responses: Receiver<OrderCommand>,
    last_client_order_id: u64,
}

impl Core {
    /// Connects as gateway `gateway_id`, trying again until the core accepts it.
    fn connect(gateway_id: u8) -> Result<Self> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            match Self::try_connect(gateway_id) {
                Ok(core) => return Ok(core),
                Err(error) if Instant::now() < deadline => {
                    eprintln!("the core does not accept gateway {gateway_id} yet: {error}");
                    std::thread::sleep(Duration::from_secs(1));
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn try_connect(gateway_id: u8) -> Result<Self> {
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
        config.gateway_id = gateway_id;
        config.connection_timeout_seconds = 10;

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
                eprintln!(
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
            eprintln!(
                "{:?} user={} order={} -> {:?}",
                command.command, command.user_id, response.order_id, response.status
            );
            return Ok(response);
        }
    }
}

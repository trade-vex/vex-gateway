#!/usr/bin/env python3
"""The read model's tests, against a real stack (trade-vex/vex-core#189).

Checks that apply-connectors.sh retries a request Connect refuses while it rebalances, a bounded
number of times, and no other, and that one put() of a JDBC connector blocks for less than the
sink consumers' max.poll.interval.ms. Then builds and starts readmodel/tests/compose.yml with
vex-core at the rev Cargo.lock pins, journaling to its Aeron Archive, with its topics as that rev's
deploy/kafka/topics.toml declares them, and with Postgres and ClickHouse serving TLS under a
throwaway CA, and checks:
- the Postgres migrations refuse a database in which roles other than its owner may create in
  schema public, apply to an empty database, and a second run is a no-op;
- the ClickHouse migrations refuse a server where the deploy has not created the views' owner,
  apply to an empty database, on a server that refuses users without a password, and to a second
  one on the same server, and a second run is a no-op;
- the seeds apply to both, twice;
- schema.sql: the keep_newer guard on every trading table, the guard itself, trading.u64 and the
  roles' privileges;
- ClickHouse's privileges: Connect may only insert into the raw tables, the gateway may only read
  the views, every view and materialized view runs as readmodel_owner, who cannot log in, even
  with its password;
- every field of every trading.proto event, after Flatten, has a column of the right type in its
  connectors' Postgres tables and in its ClickHouse raw table, which is a ReplacingMergeTree keyed
  by the event's key and event_ts, and every connector follows the read model's rules;
- end to end, with traffic through the real core and Postgres out for 90 s, first hung with a
  statement of every connector in flight, then stopped: no connector task fails, every connector
  cuts off its statement, no later than check_put_bound assumes, and its login that gets no answer,
  and retries its refused logins, every topic lands in its tables, a latest-state row holds its
  key's greatest event_ts, every event lands in its ClickHouse table once, and the per-minute
  tables hold each minute's last balance and price, a command's last fill whatever order its
  fills arrive in, u64 values above i64::MAX keep their bits in both, replaying an out-of-order
  and duplicated suffix changes no table and no view, although ClickHouse stores the copies until
  its parts merge, every latest-state row is the engine's state, and every table in both holds
  what Kafka holds, after a restart of the core with a gateway connected, which replays its
  journal, after a Kafka outage while orders flow, and after a kill -9 of the core with events
  queued behind paused brokers (check_acceptance), Connect reaches Postgres over TLS only, the
  ClickHouse connector rides out ClickHouse being down, a connector refuses a server certificate
  its CA did not sign or that names another host and resumes by itself once the right one is
  back, and a field without a column stops its Postgres connector, skipping nothing, until the
  migration adds the column and a restart resumes it, while ClickHouse's sink drops the field
  silently.

Needs git, openssl, Docker with Compose v2, and network access to GitHub. Stops and removes the
stack at the end, also on failure. In GitHub Actions, with the Actions runtime exposed to it
(ci.yml), it keeps the images' layers in the Actions cache (compose.cache.yml).

    python3 readmodel/tests/run.py
"""

import contextlib
import http.server
import itertools
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import urllib.request
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
READMODEL = ROOT / "readmodel"
TESTS = READMODEL / "tests"
CORE_REPOSITORY = "https://github.com/trade-vex/vex-core"

# vex-core's event topics and the trading.proto message each carries (vex-core
# processors/src/events.rs). The topics themselves, with their partitions and settings, come from
# the pinned vex-core's deploy/kafka/topics.toml (apply_topics).
TOPIC_MESSAGES = {
    "balances": "BalanceEvent",
    "orders": "OrderEvent",
    "orderbook": "OrderbookEvent",
    "cancels": "CancelOrderEvent",
    "trades": "TradeEvent",
    "deposits": "DepositEvent",
    "withdrawals": "WithdrawEvent",
}
# trading.proto's other messages: Order is nested in OrderEvent; order-book levels travel as JSON.
NOT_EVENTS = {"Order", "OrderbookLevel"}
BROKERS = ("redpanda-0", "redpanda-1", "redpanda-2")
# The column type of each proto field type (design doc §3.4), in Postgres and in ClickHouse. A
# type missing here has none yet.
COLUMN_TYPES = {
    "TYPE_UINT64": "bigint",
    "TYPE_UINT32": "bigint",
    "TYPE_ENUM": "text",
    "TYPE_STRING": "text",
}
CLICKHOUSE_TYPES = {
    "TYPE_UINT64": "UInt64",
    "TYPE_UINT32": "UInt64",
    "TYPE_ENUM": "LowCardinality(String)",
    "TYPE_STRING": "String",
}
# Each topic's ClickHouse raw table (readmodel/migrations/clickhouse).
CLICKHOUSE_TABLES = {
    "balances": "balance_events",
    "orders": "order_events",
    "orderbook": "orderbook_events",
    "cancels": "cancel_events",
    "trades": "trade_events",
    "deposits": "deposit_events",
    "withdrawals": "withdrawal_events",
}
# The ClickHouse views, which the gateway reads, and the per-minute tables behind them.
CLICKHOUSE_VIEWS = {
    "balances_per_minute",
    "asset_prices_per_minute",
    "deposits",
    "withdrawals",
    "trade_volumes",
}
PER_MINUTE_TABLES = {"balances_last_per_min", "market_prices_last_per_min"}
OWNER = "readmodel_owner"

SECRETS = "/etc/vex-connect/postgres.properties"
POSTGRES_CA = "/etc/vex-connect/postgres-ca.pem"
# Settings every Postgres connector has, with these values.
CONNECTOR_RULES = {
    "connector.class": "io.confluent.connect.jdbc.JdbcSinkConnector",
    # TLS that verifies the server's certificate and host name; a bound on each login and on each
    # wait for an answer, which pgjdbc leaves unbounded by default; and no other URL parameter:
    # reWriteBatchedInserts would fail every batch that holds a key twice. socketTimeout is far
    # above the longest statement: a put() carries at most max.poll.records (500) records, whose
    # upserts through keep_newer took at most 46 ms in this stack.
    "connection.url": f"jdbc:postgresql://${{file:{SECRETS}:server}}/${{file:{SECRETS}:database}}"
    f"?sslmode=verify-full&sslrootcert={POSTGRES_CA}&loginTimeout=10&socketTimeout=15",
    "connection.user": f"${{file:{SECRETS}:user}}",
    "connection.password": f"${{file:{SECRETS}:password}}",
    # A connection is tried 7 times, 30 s apart: a task rides out a Postgres outage of up to
    # 3 minutes inside one put(), which blocks for less than its consumer's max.poll.interval.ms
    # (check_put_bound).
    "connection.attempts": "7",
    "connection.backoff.ms": "30000",
    "insert.mode": "upsert",
    "pk.mode": "record_value",
    "auto.create": "false",
    "auto.evolve": "false",
    "transforms": "flatten,event_ts",
    "transforms.flatten.type": "org.apache.kafka.connect.transforms.Flatten$Value",
    "transforms.flatten.delimiter": "_",
    "transforms.event_ts.type": "org.apache.kafka.connect.transforms.ReplaceField$Value",
    "errors.tolerance": "none",
    # A statement that fails is tried again, 60 times 10 s apart.
    "max.retries": "60",
    "retry.backoff.ms": "10000",
}
# Settings that differ between Postgres connectors. Anything else (a dead-letter queue, say) is
# refused.
CONNECTOR_SETTINGS = {
    "tasks.max",
    "topics",
    "table.name.format",
    "pk.fields",
    "transforms.event_ts.renames",
}

CLICKHOUSE_SECRETS = "/etc/vex-connect/clickhouse.properties"
CLICKHOUSE_CA = "/etc/vex-connect/clickhouse-ca.pem"
# The settings of the ClickHouse connector, which writes every topic, with these values.
CLICKHOUSE_RULES = {
    "connector.class": "com.clickhouse.kafka.connect.ClickHouseSinkConnector",
    "topics": ",".join(CLICKHOUSE_TABLES),
    "topic2TableMap": ",".join(f"{topic}={table}" for topic, table in CLICKHOUSE_TABLES.items()),
    **{
        setting: f"${{file:{CLICKHOUSE_SECRETS}:{setting}}}"
        for setting in ("hostname", "port", "database", "username", "password")
    },
    # HTTPS that verifies the server's certificate against the mounted CA, and its host name:
    # the sink's clients do both by default, given the CA (sslrootcert), and nothing here turns
    # either off.
    "ssl": "true",
    "jdbcConnectionProperties": f"?sslrootcert={CLICKHOUSE_CA}",
    # The sink's exactly-once mode deduplicates by Kafka offset, but vex-core's re-published copies
    # arrive at new offsets: the ReplacingMergeTrees collapse both kinds.
    "exactlyOnce": "false",
    "errors.tolerance": "none",
    # A ClickHouse that cannot be reached is retried, as a refused or reset connection is too, and
    # a certificate that does not verify, which the sink reports as a failed connection: the task
    # writes once it is back, like the JDBC connectors after a Postgres outage.
    "retryOnSocketException": "true",
    "transforms": "flatten,event_ts,order_event_ts",
    "transforms.flatten.type": "org.apache.kafka.connect.transforms.Flatten$Value",
    "transforms.flatten.delimiter": "_",
    # ReplaceField reads each field of its output from the record by its name before the
    # rename, so it fails a record that already has the target name: the rename of orders'
    # timestamp applies to the orders topic only.
    "transforms.event_ts.type": "org.apache.kafka.connect.transforms.ReplaceField$Value",
    "transforms.event_ts.renames": "timestamp:event_ts",
    "transforms.order_event_ts.type": "org.apache.kafka.connect.transforms.ReplaceField$Value",
    "transforms.order_event_ts.renames": "order_timestamp:event_ts",
    "transforms.order_event_ts.predicate": "orders",
    "predicates": "orders",
    "predicates.orders.type": "org.apache.kafka.connect.transforms.predicates.TopicNameMatches",
    "predicates.orders.pattern": "orders",
}
CLICKHOUSE_SETTINGS = {"tasks.max"}

# What readmodel/tests/traffic sends: user 2^63 + 1 trades USD and BTC, user 3 deposits 10^19 ETH.
MAKER = 2**63 + 1
WHALE = 3
ETH = 3
WHALE_ETH = 10**19
CATCH_UP_SECONDS = 180
# How long one rpk call may run before it is killed (Stack.rpk). Each ends within seconds, but one
# can hang: a consume begun as the brokers came back from a pause waited 14 minutes, while a fresh
# one ended at once.
RPK_SECONDS = 30
# How long Postgres is out after the traffic, counted from when it hangs: within what the
# connectors ride out.
OUTAGE_SECONDS = 90
# How long the JDBC sink waits for the check of a cached connection (CachedConnectionProvider).
VALIDITY_CHECK_SECONDS = 5
# How long the core gets to stop: its pipeline drain, and its 20 s delivery grace.
STOP_SECONDS = 60


def main():
    rev = pinned_core_rev()
    with tempfile.TemporaryDirectory(prefix="vex-readmodel-") as temp:
        core = Path(temp) / "vex-core"
        step(f"check out vex-core {rev}")
        run(["git", "init", "-q", str(core)])
        run(["git", "-C", str(core), "fetch", "-q", "--depth=1", CORE_REPOSITORY, rev])
        run(["git", "-C", str(core), "-c", "advice.detachedHead=false", "checkout", "-q", rev])
        # The core's image is built from this tree; without .git, an unchanged rev hits the cache.
        shutil.rmtree(core / ".git")
        step("generate the stack's CA and server certificates")
        tls = Path(temp) / "tls"
        make_certificates(tls)

        stack = Stack(core, rev, tls)
        try:
            test(stack, core)
        except BaseException:
            print(stack.compose("logs", "--no-color", "--tail=100", check=False))
            raise
        finally:
            step("remove the stack")
            stack.compose("down", "--volumes", "--remove-orphans", check=False)
    print("All read model tests passed.")


def test(stack, core):
    check_apply_retries()
    check_put_bound()
    step("build the images")
    stack.compose(
        "--profile", "tools", "build", "connect", "aeron-archive", "core", "traffic", capture=False
    )
    step("start Redpanda, Postgres and ClickHouse")
    stack.compose("up", "--detach", "--wait", *BROKERS, "postgres", "clickhouse")
    apply_topics(stack, core)

    check_migrations(stack)
    check_clickhouse_migrations(stack)
    check_seeds(stack)
    step("schema.sql: triggers, guard, trading.u64, roles")
    stack.psql((TESTS / "schema.sql").read_text())
    check_clickhouse_privileges(stack)

    connectors = {
        path.stem: json.loads(path.read_text())
        for path in sorted((READMODEL / "connect" / "connectors").glob("*.json"))
    }
    postgres = postgres_connectors(connectors)
    clickhouse = {name: config for name, config in connectors.items() if name not in postgres}
    expect(list(clickhouse) == ["clickhouse"], f"connectors other than Postgres': {list(clickhouse)}")
    keys = topic_keys(postgres)
    descriptor = proto_descriptor(stack)
    check_coverage(stack, postgres, descriptor)
    check_clickhouse_coverage(stack, clickhouse["clickhouse"], keys, descriptor)
    start_connect(stack, connectors)
    drive_traffic_through_an_outage(stack, connectors, postgres)

    check_tables_hold_the_topics(stack, connectors, descriptor)
    check_last_fill_price(stack, connectors)
    check_u64_round_trip(stack, connectors)
    check_replayed_suffix_changes_nothing(stack, connectors)
    check_acceptance(stack, connectors, descriptor)
    check_tls(stack, connectors)
    check_clickhouse_outage_and_tls(stack, connectors)
    check_skew(stack, connectors, clickhouse, core)


def make_certificates(directory):
    """Writes into `directory` a throwaway CA (ca.crt) and, each with its key, the certificates it
    signs for the host names the connectors reach Postgres and ClickHouse by (postgres.crt,
    clickhouse.crt), and two that must not verify: one another CA of the same name signs
    (untrusted.crt), and one the CA signs for another host name (wrong-name.crt)."""

    def certificate(name, host, ca=None):
        key, request, extensions, crt = (
            directory / f"{name}.{suffix}" for suffix in ("key", "csr", "ext", "crt")
        )
        extensions.write_text(
            f"subjectAltName=DNS:{host}\n"
            if ca
            else "basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign\n"
        )
        run([
            "openssl", "req", "-new", "-nodes", "-newkey", "ec",
            "-pkeyopt", "ec_paramgen_curve:P-256", "-subj", f"/CN={host}",
            "-keyout", key, "-out", request,
        ])
        signer = (
            ["-CA", directory / f"{ca}.crt", "-CAkey", directory / f"{ca}.key"]
            if ca
            else ["-signkey", key]
        )
        run([
            "openssl", "x509", "-req", "-days", "1", "-in", request, "-extfile", extensions,
            *signer, "-out", crt,
        ])

    directory.mkdir()
    certificate("ca", "vex-readmodel test CA")
    certificate("rogue-ca", "vex-readmodel test CA")
    certificate("postgres", "postgres", ca="ca")
    certificate("clickhouse", "clickhouse", ca="ca")
    certificate("clickhouse-untrusted", "clickhouse", ca="rogue-ca")
    certificate("untrusted", "postgres", ca="rogue-ca")
    certificate("wrong-name", "impostor", ca="ca")


def apply_topics(stack, core):
    """Creates vex-core's topics and sets its subjects' compatibility as the pinned core's
    deploy/kafka/topics.toml declares them, as its `cargo xtask kafka apply` does in every
    environment: the one source of truth for them, since the brokers create no topic. Keeps each
    event topic's partition count in stack.partitions."""
    step("create vex-core's topics from its deploy/kafka/topics.toml")
    declared = tomllib.loads((core / "deploy" / "kafka" / "topics.toml").read_text())
    events, delivered = declared["event_topics"], declared["delivered_topic"]
    # A setting this function does not know of would be left out silently.
    expect(
        (set(declared), set(events), set(delivered))
        == (
            {"replication_factor", "event_topics", "delivered_topic"},
            {"partitions", "config", "subject_compatibility"},
            {"name", "partitions", "config"},
        ),
        f"topics.toml declares more than run.py applies: {declared}",
    )
    expect(
        set(events["partitions"]) == set(TOPIC_MESSAGES),
        f"vex-core publishes to {sorted(events['partitions'])}: map each to its message in "
        "TOPIC_MESSAGES",
    )
    topics = [(topic, count, events["config"]) for topic, count in events["partitions"].items()]
    topics.append((delivered["name"], delivered["partitions"], delivered["config"]))
    for topic, count, config in topics:
        stack.rpk(
            "topic", "create", topic, f"--partitions={count}",
            f"--replicas={declared['replication_factor']}",
            *(f"--topic-config={key}={value}" for key, value in config.items()),
        )
    stack.rpk(
        "registry", "compatibility-level", "set", *(f"{topic}-value" for topic in TOPIC_MESSAGES),
        f"--level={events['subject_compatibility']}",
    )
    stack.partitions = events["partitions"]


def check_apply_retries():
    step("apply-connectors.sh tries a request Connect refuses while it rebalances 6 times at most")
    names = sorted(path.stem for path in (READMODEL / "connect" / "connectors").glob("*.json"))
    put = ("PUT", f"/connectors/{names[0]}/config")
    delete = ("DELETE", "/connectors/pg-orphan")
    code, output, requests = apply_against_fake_connect({put: [409, 409], delete: [409]}, names)
    expect(code == 0 and "deleted pg-orphan" in output, output)
    expect(requests.count(put) == 3 and requests.count(delete) == 2, requests)
    code, output, requests = apply_against_fake_connect({put: [409] * 6}, names)
    expect(code != 0 and requests == [put] * 6, f"exit {code}, requests {requests}\n{output}")
    code, output, requests = apply_against_fake_connect({put: [500]}, names)
    expect(code != 0 and requests == [put], f"exit {code}, requests {requests}\n{output}")


def url_parameters():
    """The parameters of every connector's JDBC URL, by name."""
    query = CONNECTOR_RULES["connection.url"].split("?", 1)[1]
    return dict(parameter.split("=", 1) for parameter in query.split("&"))


def unanswered(timeout):
    """How long a wait for an answer from Postgres with read timeout `timeout` lasts when none
    comes. Once the read times out, pgjdbc closes the connection, and the JVM closes a TLS 1.3
    socket by reading what the server still sends, again for up to its read timeout (SSLSocketImpl;
    in the Connect image, a statement with socketTimeout=5 failed after 10.01 s over TLS 1.3 and
    after 5.02 s over TLS 1.2 or without TLS)."""
    return 2 * timeout


def check_put_bound():
    step("one put() blocks for less than the sink consumers' max.poll.interval.ms")
    url = url_parameters()
    login, socket = (int(url.get(name, 0)) for name in ("loginTimeout", "socketTimeout"))
    expect(login > 0 and socket > 0, "without loginTimeout and socketTimeout, a put() never ends")
    attempts = int(CONNECTOR_RULES["connection.attempts"])
    backoff = int(CONNECTOR_RULES["connection.backoff.ms"]) / 1000
    # With Postgres unreachable or hung, a put() checks its cached connection, then tries to
    # connect, each login cut off after loginTimeout (pgjdbc logs in on a thread of its own and
    # stops waiting for it then). A new JDBC dialect's first connection runs one query of its own:
    # a put() runs either that query or the check, never both, but both are counted. Then the
    # first statement that gets no answer closes the connection, so those after it fail at once.
    worst = (
        unanswered(VALIDITY_CHECK_SECONDS)
        + attempts * login
        + (attempts - 1) * backoff
        + 2 * unanswered(socket)
    )
    worker = dict(
        line.split("=", 1)
        for line in (READMODEL / "connect" / "worker.properties").read_text().splitlines()
        if line and not line.startswith("#")
    )
    interval = int(worker.get("consumer.max.poll.interval.ms", 0)) / 1000
    expect(
        worst < interval,
        f"one put() can block for {worst:g} s, not less than the sink consumers' "
        f"max.poll.interval.ms ({interval:g} s): the group would drop a task that rides out an "
        "outage",
    )


def apply_against_fake_connect(failures, names):
    """Runs apply-connectors.sh against a stand-in for Connect's REST API, which runs `names` and
    pg-orphan and answers each (method, path) with its statuses in `failures` first, then with
    success. Returns the script's exit code and output, and the requests it made."""
    requests = []

    class Connect(http.server.BaseHTTPRequestHandler):
        def answer(self):
            requests.append((self.command, self.path))
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
            pending = failures.get((self.command, self.path))
            status = pending.pop(0) if pending else 200
            body = json.dumps(names + ["pg-orphan"] if status == 200 else {"error_code": status})
            self.send_response(status)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body.encode())

        do_GET = do_PUT = do_DELETE = answer

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Connect)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    apply = READMODEL / "connect" / "apply-connectors.sh"
    try:
        result = subprocess.run(
            [apply, f"http://127.0.0.1:{server.server_port}"],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
    finally:
        server.shutdown()
        server.server_close()
    return result.returncode, result.stdout, requests


def check_migrations(stack):
    step("the migrations refuse a database in which not only its owner may create in public")
    # As in a database upgraded from PostgreSQL 14, which let every role create in public.
    stack.psql("CREATE DATABASE refused")
    stack.psql("GRANT CREATE ON SCHEMA public TO PUBLIC", database="refused")
    output = stack.migrate("refused", check=False, stderr=subprocess.STDOUT)
    expect("schema public lets PUBLIC create objects in it" in output, output)
    trading = "SELECT count(*) FROM pg_namespace WHERE nspname = 'trading'"
    expect(stack.psql(trading, database="refused") == "0", "the refused migration created trading")
    stack.psql("DROP DATABASE refused")

    step("the migrations apply to an empty database")
    migrations = (READMODEL / "migrations" / "postgres").glob("*.up.sql")
    latest = max(int(path.name.split("_")[0]) for path in migrations)
    stack.migrate()
    schema = stack.dump_schema()
    version = stack.psql("SELECT version, dirty FROM schema_migrations")
    expect(version == f"{latest}|f", f"version|dirty is {version}, not {latest}|f")

    step("a second run is a no-op")
    output = stack.migrate(stderr=subprocess.STDOUT)
    expect(output.rstrip().endswith("no change"), output)
    expect(stack.dump_schema() == schema, "the second run changed the schema")


def check_clickhouse_migrations(stack):
    step("the ClickHouse migrations refuse a server where the deploy has not created their owner")
    stack.clickhouse("CREATE DATABASE no_owner")
    output = stack.migrate_clickhouse("no_owner", check=False, stderr=subprocess.STDOUT)
    expect(f"There is no user `{OWNER}`" in output, output)
    stack.clickhouse("DROP DATABASE no_owner SYNC")
    # What a deployment does (RUNBOOK "Read Model"), with the test password but, unlike it, from
    # any host: the migrations make it HOST NONE. The server refuses users without a password
    # (clickhouse.xml), as managed ones do.
    stack.clickhouse(f"CREATE USER {OWNER} IDENTIFIED BY 'test-only'")
    expect(stack.clickhouse("SELECT 1", user=OWNER) == "1", f"{OWNER} cannot log in yet")

    step("the ClickHouse migrations apply to an empty database, and a second run is a no-op")
    migrations = (READMODEL / "migrations" / "clickhouse").glob("*.up.sql")
    latest = max(int(path.name.split("_")[0]) for path in migrations)
    stack.migrate_clickhouse()
    schema = stack.clickhouse_schema()
    version = stack.clickhouse(
        "SELECT version, dirty FROM schema_migrations ORDER BY sequence DESC LIMIT 1"
    )
    expect(version == f"{latest}\t0", f"version, dirty is {version!r}, not {latest}, 0")
    output = stack.migrate_clickhouse(stderr=subprocess.STDOUT)
    expect(output.rstrip().endswith("no change"), output)
    expect(stack.clickhouse_schema() == schema, "the second run changed the schema")

    step("they apply to a second database on the same server, whose roles and owner exist")
    stack.clickhouse("CREATE DATABASE second")
    stack.migrate_clickhouse("second")
    stack.clickhouse("DROP DATABASE second SYNC")


def check_seeds(stack):
    step("the seeds apply, and again with no change")
    seeds = (READMODEL / "seeds" / "development.sql").read_text()
    rows = "SELECT * FROM assets ORDER BY asset_id; SELECT * FROM markets ORDER BY market_id"
    stack.psql(seeds)
    seeded = stack.psql(rows)
    stack.psql(seeds)
    expect(seeded and stack.psql(rows) == seeded, seeded)

    seeds = (READMODEL / "seeds" / "development.clickhouse.sql").read_text()
    rows = "SELECT * FROM assets FINAL ORDER BY ALL; SELECT * FROM markets FINAL ORDER BY ALL"
    stack.clickhouse(seeds)
    seeded = stack.clickhouse(rows)
    stack.clickhouse(seeds)
    expect(seeded and stack.clickhouse(rows) == seeded, seeded)


def check_clickhouse_privileges(stack):
    step("ClickHouse: Connect only inserts into the raw tables, the gateway only reads the views")
    # What a deployment does for the logins (RUNBOOK "Read Model"), with the test password.
    for login, role in [
        ("readmodel_connect_login", "readmodel_connect"),
        ("readmodel_gateway_login", "readmodel_gateway"),
    ]:
        stack.clickhouse(f"CREATE USER {login} IDENTIFIED BY 'test-only'; GRANT {role} TO {login}")
    grants = defaultdict(set)
    for grant in stack.clickhouse_rows(
        "SELECT role_name, access_type, table FROM system.grants "
        "WHERE (database = 'readmodel' OR database IS NULL) "
        "AND role_name IN ('readmodel_connect', 'readmodel_gateway')"
    ):
        grants[grant["role_name"]].add((grant["access_type"], grant["table"]))
    raw = {("INSERT", table) for table in CLICKHOUSE_TABLES.values()}
    expect(grants["readmodel_connect"] == raw, f"readmodel_connect: {grants['readmodel_connect']}")
    views = {("SELECT", view) for view in CLICKHOUSE_VIEWS}
    expect(grants["readmodel_gateway"] == views, f"readmodel_gateway: {grants['readmodel_gateway']}")

    tables = stack.clickhouse_rows(
        "SELECT name, engine, create_table_query FROM system.tables WHERE database = 'readmodel'"
    )
    expect(
        {table["name"] for table in tables if table["engine"] == "View"} == CLICKHOUSE_VIEWS,
        "every view is the gateway's, and the gateway reads every view",
    )
    for table in tables:
        if table["engine"] in ("View", "MaterializedView"):
            # Without the clause, a view runs with the rights of whoever reads it.
            expect(
                f"DEFINER = {OWNER} SQL SECURITY DEFINER" in table["create_table_query"],
                f"{table['name']} does not run as {OWNER}: {table['create_table_query']}",
            )

    step("the gateway's login reads the views and is denied every table")
    for table in tables:
        name = table["name"]
        read = stack.clickhouse(
            f"SELECT * FROM {name} LIMIT 0", user="readmodel_gateway_login", check=False
        )
        if name in CLICKHOUSE_VIEWS:
            expect(read == "", f"the gateway cannot read {name}: {read}")
        else:
            expect("ACCESS_DENIED" in read, f"the gateway may read {name}: {read!r}")
        write = stack.clickhouse(
            f"INSERT INTO {name} SELECT * FROM {name} LIMIT 0",
            user="readmodel_gateway_login",
            check=False,
        )
        expect("ACCESS_DENIED" in write, f"the gateway may write {name}: {write!r}")
    login = stack.clickhouse("SELECT 1", user=OWNER, check=False)
    expect("AUTHENTICATION_FAILED" in login, f"{OWNER} can log in with its password: {login!r}")


def proto_descriptor(stack):
    """trading.proto at the pinned rev, compiled by buf: its messages and enums by name."""
    image = json.loads(stack.compose("run", "--rm", "buf"))
    (proto,) = [file for file in image["file"] if file["name"] == "trading.proto"]
    return {
        "messages": {message["name"]: message for message in proto["messageType"]},
        "enums": {enum["name"]: enum for enum in proto.get("enumType", [])},
    }


def postgres_connectors(connectors):
    """The JDBC connectors, which write Postgres: every connector but the ClickHouse one."""
    return {
        name: config
        for name, config in connectors.items()
        if config["connector.class"] == CONNECTOR_RULES["connector.class"]
    }


def topic_keys(postgres):
    """Each topic's key columns: the primary key of its Postgres tables without event_ts."""
    keys = {}
    for config in postgres.values():
        key = [column for column in config["pk.fields"].split(",") if column != "event_ts"]
        topic = config["topics"]
        expect(keys.setdefault(topic, key) == key, f"{topic}: keys {keys[topic]} and {key}")
    return keys


def topic_columns(topic, config, descriptor):
    """{column: path of proto fields} of a table that a connector with `config` writes `topic`
    to: the topic's message after Flatten (delimiter "_") and the connector's renames."""
    renames = dict(
        pair.split(":")
        for setting, value in config.items()
        if setting.endswith(".renames")
        for pair in value.split(",")
    )

    def flatten(message, prefix):
        for field in descriptor["messages"][message]["field"]:
            expect(
                field["label"] != "LABEL_REPEATED",
                f"{message}.{field['name']}: Flatten cannot store a repeated field",
            )
            path = prefix + (field,)
            if field["type"] == "TYPE_MESSAGE":
                yield from flatten(field["typeName"].rsplit(".", 1)[1], path)
            else:
                name = "_".join(part["name"] for part in path)
                yield renames.get(name, name), path

    return dict(flatten(TOPIC_MESSAGES[topic], ()))


def check_coverage(stack, connectors, descriptor):
    step("every field of every trading.proto event has a column in its tables")
    unknown = set(descriptor["messages"]) - set(TOPIC_MESSAGES.values()) - NOT_EVENTS
    expect(not unknown, f"trading.proto messages no topic carries: {unknown}; map them above")
    topics = {config["topics"] for config in connectors.values()}
    expect(topics == set(TOPIC_MESSAGES), f"the connectors read {topics}")
    tables = stack.psql(
        "SELECT 'trading.' || tablename FROM pg_tables WHERE schemaname = 'trading'"
    ).split()
    expect(
        sorted(config["table.name.format"] for config in connectors.values()) == sorted(tables),
        "every trading table needs exactly one connector",
    )
    for name, config in connectors.items():
        for setting, value in CONNECTOR_RULES.items():
            expect(config.get(setting) == value, f"{name}: {setting} must be {value!r}")
        unexpected = set(config) - set(CONNECTOR_RULES) - CONNECTOR_SETTINGS
        expect(not unexpected, f"{name}: unexpected settings {unexpected}")

        schema, table = config["table.name.format"].split(".")
        needed = {
            column: COLUMN_TYPES.get(path[-1]["type"], path[-1]["type"])
            for column, path in topic_columns(config["topics"], config, descriptor).items()
        }
        columns = dict(
            line.split("|")
            for line in stack.psql(
                "SELECT column_name, data_type FROM information_schema.columns "
                f"WHERE table_schema = '{schema}' AND table_name = '{table}'"
            ).splitlines()
        )
        expect(
            columns == needed,
            f"{name}: {schema}.{table} has {columns}, its {config['topics']} events need {needed}",
        )
        primary_key = stack.psql(
            "SELECT attname FROM pg_index "
            "JOIN pg_attribute ON attrelid = indrelid AND attnum = ANY (indkey) "
            f"WHERE indrelid = '{schema}.{table}'::regclass AND indisprimary"
        ).split()
        expect(
            sorted(primary_key) == sorted(config["pk.fields"].split(",")),
            f"{name}: pk.fields is not the primary key {primary_key}",
        )


def check_clickhouse_coverage(stack, config, keys, descriptor):
    step("every field of every trading.proto event has a column in its ClickHouse raw table")
    for setting, value in CLICKHOUSE_RULES.items():
        expect(config.get(setting) == value, f"clickhouse: {setting} must be {value!r}")
    unexpected = set(config) - set(CLICKHOUSE_RULES) - CLICKHOUSE_SETTINGS
    expect(not unexpected, f"clickhouse: unexpected settings {unexpected}")
    expect(set(CLICKHOUSE_TABLES) == set(TOPIC_MESSAGES), "every topic needs a ClickHouse table")
    for topic, table in CLICKHOUSE_TABLES.items():
        # The sink writes a table's columns and drops any field without one, silently: this is
        # the only check that every field has its column.
        needed = {
            column: CLICKHOUSE_TYPES.get(path[-1]["type"], path[-1]["type"])
            for column, path in topic_columns(topic, config, descriptor).items()
        }
        columns = stack.clickhouse_rows(
            "SELECT name, type, default_kind, default_expression FROM system.columns "
            f"WHERE database = 'readmodel' AND table = '{table}'"
        )
        written = {c["name"]: c["type"] for c in columns if c["default_kind"] != "MATERIALIZED"}
        expect(written == needed, f"{table} has {written}, the {topic} events need {needed}")
        materialized = {
            c["name"]: (c["type"], c["default_expression"])
            for c in columns
            if c["default_kind"] == "MATERIALIZED"
        }
        ts = ("DateTime64(9, 'UTC')", "fromUnixTimestamp64Nano(toInt64(event_ts))")
        expect(materialized == {"ts": ts}, f"{table}: MATERIALIZED columns {materialized}")
        # One row per event once merged, whichever copy came last, and copies share a partition.
        (engine,) = stack.clickhouse_rows(
            "SELECT engine, sorting_key, partition_key FROM system.tables "
            f"WHERE database = 'readmodel' AND name = '{table}'"
        )
        expected = {
            "engine": "ReplacingMergeTree",
            "sorting_key": ", ".join(keys[topic] + ["event_ts"]),
            "partition_key": "toYYYYMM(ts)",
        }
        expect(engine == expected, f"{table} is {engine}, not {expected}")


def start_connect(stack, connectors):
    step("Connect runs exactly the connectors in readmodel/connect/connectors")
    # What a deployment does for Connect's login (RUNBOOK "Read Model"), with postgres.properties'
    # test password.
    stack.psql(
        "CREATE ROLE readmodel_connect_login LOGIN PASSWORD 'test-only' IN ROLE readmodel_connect"
    )
    stack.compose("up", "--detach", "--wait", "connect")
    apply = [str(READMODEL / "connect" / "apply-connectors.sh"), stack.connect_url()]
    stack.connect("PUT", "/connectors/pg-orphan/config", connectors["pg-orderbook"])
    output = run(apply)
    expect("deleted pg-orphan" in output, output)
    running = sorted(stack.connect("GET", "/connectors"))
    expect(running == sorted(connectors), f"Connect runs {running}")
    output = run(apply)
    expect("deleted" not in output, output)


def drive_traffic_through_an_outage(stack, connectors, postgres):
    step("drive traffic through vex-core with the connectors paused")
    for name in connectors:
        stack.connect("PUT", f"/connectors/{name}/pause")
    for name in connectors:
        wait_for_tasks(stack, name, lambda states: states == {"PAUSED"})
    stack.compose("up", "--detach", "--wait", "core")
    stack.compose("run", "--rm", "traffic", "coverage", capture=False)

    step("lock the trading tables and resume the connectors: each Postgres one's upserts wait")
    # So that every Postgres connector has a statement in flight when Postgres hangs, each begun a
    # moment before: it is cut off while Postgres is hung.
    tables = sorted(config["table.name.format"] for config in postgres.values())
    lock = stack.lock(tables)
    for name in connectors:
        stack.connect("PUT", f"/connectors/{name}/resume")
    url = url_parameters()
    cut_off = unanswered(int(url["socketTimeout"]))
    started = wait_for_statements_waiting(stack, tables, cut_off - 5)

    step(f"Postgres is out for {OUTAGE_SECONDS} s, first hung: nothing gets an answer")
    hung_since = f"{time.time():.3f}"
    stack.compose("pause", "postgres")
    hung = time.monotonic()
    try:
        # A statement in flight fails `cut_off` after it began, and its batch is written again
        # after retry.backoff.ms over a new connection, whose login is cut off after loginTimeout.
        within = (
            cut_off
            + int(CONNECTOR_RULES["retry.backoff.ms"]) / 1000
            + int(url["loginTimeout"])
            + 30
        )
        wait_for_every_connector(stack, connectors, postgres, hung_since, hung + within, {
            "statement cut off for want of an answer": [("Write of", "Read timed out")],
            "login cut off for want of an answer": [("Unable to connect", "timed out")],
        })
        check_cut_off_times(stack, postgres, hung_since, started, cut_off)
    finally:
        stack.compose("unpause", "postgres")
        lock.communicate(timeout=30)

    step("then stopped: logins are refused")
    stopped_since = f"{time.time():.3f}"
    stack.compose("stop", "postgres")
    within = time.monotonic() + int(CONNECTOR_RULES["connection.backoff.ms"]) / 1000 + 30
    wait_for_every_connector(stack, connectors, postgres, stopped_since, within, {
        "login refused": [
            ("Unable to connect", "UnknownHostException"), ("Unable to connect", "refused")
        ],
    })
    while time.monotonic() < hung + OUTAGE_SECONDS:
        expect_no_failed_task(stack.connect("GET", "/connectors?expand=status"))
        time.sleep(2)
    stack.compose("up", "--detach", "--wait", "postgres")

    step("every connector catches up by itself, and no task failed")
    wait_until_caught_up(stack, connectors, every_topic_published=True)


def wait_for_statements_waiting(stack, tables, younger_than):
    """Waits until a statement of Connect's login waits for the lock of every one of `tables`.
    Returns when each waiting statement began, {table: [Unix time]}, and fails unless all began
    less than `younger_than` seconds ago."""
    deadline = time.monotonic() + CATCH_UP_SECONDS
    waiting = (
        "SELECT 'trading.' || relname, extract(epoch FROM query_start), "
        "extract(epoch FROM clock_timestamp() - query_start) FROM pg_locks "
        "JOIN pg_class ON pg_class.oid = relation JOIN pg_stat_activity USING (pid) "
        "WHERE NOT granted AND usename = 'readmodel_connect_login'"
    )
    while True:
        started, ages = defaultdict(list), []
        for line in stack.psql(waiting).splitlines():
            table, start, age = line.split("|")
            started[table].append(float(start))
            ages.append(float(age))
        if sorted(started) == tables:
            expect(
                max(ages) < younger_than,
                f"a statement of Connect's began {max(ages):.1f} s ago, over {younger_than} s",
            )
            return started
        expect(time.monotonic() < deadline, f"Connect's statements wait only for {list(started)}")
        time.sleep(1)


def check_cut_off_times(stack, connectors, since, started, cut_off):
    """Checks that each statement the worker logged as cut off since the Unix time `since` was
    cut off at most `cut_off` (and 5 s for scheduling) after the latest of its table's statements
    in `started` began: unanswered() holds."""
    delays = []
    for entry in connect_log(stack, since):
        write = re.match(r"\[([-\d]+ [:\d]+,\d+)\] WARN \[([\w-]+)\|task-\d+\] Write of", entry)
        if write and "Read timed out" in entry:
            # The worker logs in UTC.
            logged_at = datetime.strptime(write[1], "%Y-%m-%d %H:%M:%S,%f")
            table = connectors[write[2]]["table.name.format"]
            delays.append(
                logged_at.replace(tzinfo=timezone.utc).timestamp() - max(started[table])
            )
    expect(
        delays and max(delays) <= cut_off + 5,
        f"statements cut off {sorted(delays)} s after they began, not within {cut_off} s",
    )
    print(
        f"    {len(delays)} statements cut off {min(delays):.2f}-{max(delays):.2f} s after they "
        "began",
        flush=True,
    )


def wait_for_every_connector(stack, connectors, logging, since, deadline, expected):
    """Waits, while no connector task fails, until the worker has logged, since the Unix time
    `since` and for every one of the connectors `logging`, each of `expected`: {what: entries that
    show it, each as the texts it contains}. Fails after the monotonic time `deadline`."""
    while True:
        expect_no_failed_task(stack.connect("GET", "/connectors?expand=status"))
        entries = connect_log(stack, since)
        missing = {
            name: [
                what
                for what, shown_by in expected.items()
                if not any(logged(entries, name, *texts) for texts in shown_by)
            ]
            for name in logging
        }
        missing = {name: whats for name, whats in missing.items() if whats}
        if not missing:
            return
        expect(time.monotonic() < deadline, f"not logged in time: {missing}")
        time.sleep(2)


def expect_no_failed_task(states):
    failed = {
        name: task["trace"]
        for name, state in states.items()
        for task in state["status"]["tasks"]
        if task["state"] == "FAILED"
    }
    expect(not failed, f"failed connector tasks: {failed}")


def connect_log(stack, since=None):
    """The Connect worker's log entries, each a line and the stack trace after it, since the Unix
    time `since` or from the start."""
    log = stack.compose(
        "logs", "--no-color", "--no-log-prefix", *([f"--since={since}"] if since else []), "connect"
    )
    return re.split(r"\n(?=\[\d{4}-\d\d-\d\d )", log)


def logged(entries, connector, *texts):
    """Whether one of the log `entries` of `connector`'s tasks contains every one of `texts`."""
    return any(
        f"[{connector}|task-" in entry and all(text in entry for text in texts)
        for entry in entries
    )


def wait_until_caught_up(stack, connectors, every_topic_published=False):
    """Waits until every task of the `connectors` runs and has committed the end of its topics,
    and no topic has grown since the previous look. With `every_topic_published`, also until no
    topic is empty: vex-core publishes after it answers."""
    deadline = time.monotonic() + CATCH_UP_SECONDS
    # Each connector's group reads every partition of its topics.
    partitions = {
        f"connect-{name}": sum(stack.partitions[topic] for topic in config["topics"].split(","))
        for name, config in connectors.items()
    }
    previous_ends = None
    while True:
        states = {
            name: state
            for name, state in stack.connect("GET", "/connectors?expand=status").items()
            if name in connectors
        }
        expect_no_failed_task(states)
        try:
            groups = json.loads(stack.rpk("group", "describe", *partitions, "--format=json"))
            timed_out = []
        except TimeoutError as error:
            # Not caught up yet: the next look reads again.
            groups, timed_out = [], [str(error)]
        ends = {
            (partition["topic"], partition["partition"]): partition["log_end_offset"]
            for group in groups
            for partition in group["partitions"]
        }
        lagging = timed_out + [
            group["group_name"]
            for group in groups
            if len(group["partitions"]) != partitions[group["group_name"]]
            or any(not committed_end(partition) for partition in group["partitions"])
        ] + [
            name
            for name, state in states.items()
            if any(task["state"] != "RUNNING" for task in state["status"]["tasks"])
        ]
        empty = [
            topic
            for topic in TOPIC_MESSAGES
            if every_topic_published
            and not any(ends.get((topic, p)) for p in range(stack.partitions[topic]))
        ]
        if not lagging and not empty and ends == previous_ends:
            return
        expect(
            time.monotonic() < deadline,
            f"not caught up after {CATCH_UP_SECONDS} s: lagging {lagging}, empty {empty}",
        )
        previous_ends = ends
        time.sleep(2)


def committed_end(partition):
    """Whether a group has committed a partition's end, or the partition has no records."""
    if partition["current_offset"] == -1:
        return partition["log_end_offset"] == partition["log_start_offset"]
    return partition["current_offset"] == partition["log_end_offset"]


def consume(stack, topic, value_format):
    """Every record of `topic`, one line each: partition, key, value (in `value_format`). A read
    killed at RPK_SECONDS reads again, for up to CATCH_UP_SECONDS."""
    deadline = time.monotonic() + CATCH_UP_SECONDS
    while True:
        try:
            lines = stack.rpk("topic", "consume", topic, "--offset=:end", *value_format)
            return [line.split("\t", 2) for line in lines.splitlines()]
        except TimeoutError:
            if time.monotonic() >= deadline:
                raise
            print(f"    reading {topic} timed out; reading it again", flush=True)


def row_of(value, columns, descriptor, signed=True):
    """The row a connector writes for one event, decoded to proto3 JSON, into a table with
    `columns` (topic_columns). That JSON leaves out default values, which Connect writes. Integers
    are as Postgres stores them, or unsigned, as ClickHouse does, unless `signed`."""
    row = {}
    for column, path in columns.items():
        field_value = value
        for field in path:
            field_value = (field_value or {}).get(field["jsonName"])
        field = path[-1]
        if field["type"] == "TYPE_ENUM":
            enum = descriptor["enums"][field["typeName"].rsplit(".", 1)[1]]
            default = next(entry["name"] for entry in enum["value"] if entry.get("number", 0) == 0)
            row[column] = field_value if field_value is not None else default
        elif field["type"] == "TYPE_STRING":
            row[column] = field_value or ""
        else:
            # proto3 JSON writes 64-bit integers as strings; a Postgres column holds their bits.
            number = int(field_value or 0)
            row[column] = number - 2**64 if signed and number >= 2**63 else number
    return row


def published(stack, topic):
    """Every event on `topic`, decoded to proto3 JSON, each partition in order."""
    records = consume(stack, topic, ["--use-schema-registry=value", r"--format=%p\t%k\t%v\n"])
    return [json.loads(value) for _, _, value in records]


def rows_of(events, config, descriptor):
    """The rows a Postgres connector's table holds after `events`, by primary key. A key's events
    share a partition, in the order the sink reads them: the first copy of a (key, event_ts) stays,
    and a latest-state row keeps the greatest event_ts."""
    columns = topic_columns(config["topics"], config, descriptor)
    rows = {}
    for row in (row_of(value, columns, descriptor) for value in events):
        key = key_of(row, config)
        if key not in rows or row["event_ts"] > rows[key]["event_ts"]:
            rows[key] = row
    return rows


def key_of(row, config):
    """The primary key of a row of a Postgres connector's table."""
    return tuple(row[column] for column in config["pk.fields"].split(","))


def check_tables_hold_the_topics(stack, connectors, descriptor):
    step("every topic lands in its tables; a latest-state row holds its key's greatest event_ts")
    events = {topic: published(stack, topic) for topic in TOPIC_MESSAGES}
    for topic, published_events in events.items():
        expect(published_events, f"vex-core published nothing on {topic}")
    postgres = postgres_connectors(connectors)
    for name, config in postgres.items():
        expected = rows_of(events[config["topics"]], config, descriptor)
        rows = stack.json_rows(config["table.name.format"])
        expect(
            sorted(rows, key=canonical) == sorted(expected.values(), key=canonical),
            f"{name}: {config['table.name.format']} holds {rows}, not {list(expected.values())}",
        )
    check_clickhouse_holds_the_topics(
        stack, connectors["clickhouse"], topic_keys(postgres), descriptor, events
    )


def check_clickhouse_holds_the_topics(stack, config, keys, descriptor, events):
    step("every topic lands in its ClickHouse table, once per event")
    stored = {}
    for topic, table in CLICKHOUSE_TABLES.items():
        columns = topic_columns(topic, config, descriptor)
        key = keys[topic] + ["event_ts"]
        expected = {}
        # Of two copies of an event, the one written last stays.
        for row in (row_of(value, columns, descriptor, signed=False) for value in events[topic]):
            expected[tuple(row[column] for column in key)] = row
        stored[topic] = sorted(expected.values(), key=canonical)
        rows = sorted(stack.clickhouse_rows(f"SELECT * FROM {table} FINAL"), key=canonical)
        expect(rows == stored[topic], f"{table} holds {rows}, not {stored[topic]}")

    step("the per-minute tables hold each minute's last balance and last trade price")

    def minute(row):
        return row["event_ts"] // 60_000_000_000 * 60

    balances = {}
    for row in stored["balances"]:
        last = {column: row[column] for column in ("user_id", "asset_id", "total", "event_ts")}
        last["minute"] = minute(row)
        key = (row["user_id"], row["asset_id"], last["minute"])
        if key not in balances or row["event_ts"] > balances[key]["event_ts"]:
            balances[key] = last
    rows = stack.clickhouse_rows(
        "SELECT user_id, asset_id, toUnixTimestamp(minute) AS minute, total, event_ts "
        "FROM balances_last_per_min FINAL"
    )
    expected = sorted(balances.values(), key=canonical)
    expect(sorted(rows, key=canonical) == expected, f"balances per minute: {rows}, not {expected}")
    prices = {}
    for row in stored["trades"]:
        # The last fill of a command: a bid taker fills from the lowest ask price up, an ask
        # taker from the highest bid price down.
        order = (row["event_ts"], -row["price"] if row["taker_side"] == "SIDE_ASK" else row["price"])
        key = (row["market_id"], minute(row))
        if key not in prices or order > prices[key][0]:
            prices[key] = (order, {"market_id": key[0], "minute": key[1], "price": row["price"]})
    rows = stack.clickhouse_rows(
        "SELECT market_id, toUnixTimestamp(minute) AS minute, price "
        "FROM market_prices_last_per_min FINAL"
    )
    expected = sorted((last for _, last in prices.values()), key=canonical)
    expect(sorted(rows, key=canonical) == expected, f"prices per minute: {rows}, not {expected}")


def check_last_fill_price(stack, connectors):
    step("a minute's price is its last command's last fill, whatever order the fills arrive in")
    market = 4242
    now = time.time_ns() // 60_000_000_000 * 60_000_000_000
    # One ask taker's fills, from the highest bid down, then a bid taker's, from the lowest ask
    # up, each command's fills sharing its timestamp: the last fill comes first in each. Each
    # fill is written once the one before it is, so by an insert of its own: with event_ts alone
    # as the version, the fills would tie and the one inserted later, not the last, would win.
    commands = [("SIDE_ASK", now, [90, 100]), ("SIDE_BID", now + 1, [80, 70])]
    for side, timestamp, fills in commands:
        for maker, price in enumerate(fills):
            trade = {
                "makerUserId": "1", "takerUserId": "2", "marketId": market, "price": str(price),
                "size": "1", "makerOrderId": str(maker), "takerOrderId": str(timestamp),
                "takerSide": side, "timestamp": str(timestamp),
            }
            produce(stack, "trades", f"{timestamp}:{maker}", "TradeEvent", [trade])
            wait_until_caught_up(stack, connectors)
        price = stack.clickhouse(
            f"SELECT price FROM market_prices_last_per_min FINAL WHERE market_id = {market}"
        )
        expect(price == str(fills[0]), f"market {market} has price {price}, not {fills[0]}")


def check_u64_round_trip(stack, connectors):
    step("u64 values above i64::MAX keep their bits")
    # A balance of 10^19, through the real core.
    balance = stack.psql(
        "SELECT available, trading.u64(available) FROM trading.balances "
        f"WHERE user_id = {WHALE} AND asset_id = {ETH}"
    )
    expect(balance == f"{WHALE_ETH - 2**64}|{WHALE_ETH}", f"the balance of 10^19 is {balance}")
    maker = stack.psql(
        f"SELECT count(*) FROM trading.balances WHERE trading.u64(user_id) = {MAKER}"
    )
    expect(maker == "2", f"user 2^63 + 1 has {maker} balances, not 2")
    # ClickHouse holds them unsigned, and the gateway's view scales them.
    balance = stack.clickhouse(
        f"SELECT available FROM balance_events FINAL WHERE user_id = {WHALE} AND asset_id = {ETH}"
    )
    expect(balance == str(WHALE_ETH), f"ClickHouse has the balance of 10^19 as {balance}")
    maker = stack.clickhouse(
        f"SELECT uniqExact(asset_id) FROM balance_events FINAL WHERE user_id = {MAKER}"
    )
    expect(maker == "2", f"ClickHouse has balances of user 2^63 + 1 in {maker} assets, not 2")
    scaled = stack.clickhouse(
        "SELECT total FROM balances_per_minute "
        f"WHERE user_id = {WHALE} AND asset_id = {ETH} ORDER BY minute DESC LIMIT 1",
        user="readmodel_gateway_login",
    )
    expect(scaled == "10", f"the gateway reads the balance of 10^19 ETH units as {scaled} ETH")
    # vex-core's order ids reach 2^63 around 2029, so this one is produced here, encoded with the
    # schema the core registered.
    order_id = 2**63 + 12345
    order = {
        "order": {
            "orderId": str(order_id),
            "userId": str(MAKER),
            "price": "1",
            "size": "1",
            "side": "SIDE_BID",
            "timeInForce": "TIF_GTC",
            "status": "STATUS_PLACED",
            "timestamp": str(time.time_ns()),
        },
        "marketId": 65538,
    }
    produce(stack, "orders", order_id, "OrderEvent", [order])
    wait_until_caught_up(stack, connectors)
    for table in ("trading.orders", "trading.order_events"):
        stored = stack.psql(
            f"SELECT order_order_id FROM {table} WHERE trading.u64(order_order_id) = {order_id}"
        )
        # The gateway reads the column as i64 and decodes it with `as u64`.
        expect(
            stored == str(order_id - 2**64) and int(stored) % 2**64 == order_id,
            f"{table}: order {order_id} is {stored!r}",
        )
    stored = stack.clickhouse(
        f"SELECT order_order_id FROM order_events FINAL WHERE order_order_id = {order_id}"
    )
    expect(stored == str(order_id), f"ClickHouse has order {order_id} as {stored!r}")


def check_replayed_suffix_changes_nothing(stack, connectors):
    step("replaying an out-of-order, duplicated suffix changes no table and no view")
    before = stack.snapshot()
    # ClickHouse keeps a copy written in another insert than the original until their parts
    # merge, which they do not meanwhile: reads through FINAL see each event once all the same.
    # Copies within one insert collapse as it is written.
    stack.clickhouse("SYSTEM STOP MERGES")
    clickhouse_before = stack.clickhouse_snapshot()
    stored_before = stack.clickhouse_stored_rows()
    replayed = {}
    for topic in TOPIC_MESSAGES:
        records = defaultdict(list)
        for partition, key, value in consume(stack, topic, [r"--format=%p\t%k\t%v{base64}\n"]):
            records[partition].append(f"{partition}\t{key}\t{value}\n")
        suffix = [record for log in records.values() for record in log[len(log) // 2 :]]
        # Each record again, then every one once more in reverse: older copies after newer
        # events, which a missing guard would leave as the latest rows.
        replay = suffix + suffix[::-1]
        replayed[topic] = len(suffix)
        stack.rpk(
            "topic", "produce", topic, "--partition=0", r"--format=%p\t%k\t%v{base64}\n",
            stdin="".join(replay),
        )
    wait_until_caught_up(stack, connectors)
    expect(stack.snapshot() == before, "the replayed suffix changed a table")
    expect(
        stack.clickhouse_snapshot() == clickhouse_before,
        "the replayed suffix changed a ClickHouse table or view",
    )
    stored = stack.clickhouse_stored_rows()
    for topic, table in CLICKHOUSE_TABLES.items():
        expect(
            stored[table] >= stored_before[table] + replayed[topic],
            f"{table} stored {stored[table] - stored_before[table]} rows for copies of "
            f"{replayed[topic]} stored events",
        )
    for table in PER_MINUTE_TABLES:
        expect(stored[table] > stored_before[table], f"{table} stored no copy")
    stack.clickhouse("SYSTEM START MERGES")


def check_acceptance(stack, connectors, descriptor):
    """The read model's acceptance (trade-vex/vex-core#189): every latest-state row is the
    engine's state after a restart of the core with a gateway connected, which replays its
    journal, after a Kafka outage while orders flow, and after a kill -9 of the core with events
    queued behind paused brokers.
    The engine's state is the ledger the traffic driver builds from the core's answers
    (ledger_round), which Kafka's latest event per key must agree with (check_engine_state)."""
    # Each round connects as a gateway of its own; gateway 1 drove the coverage traffic.
    gateways = itertools.count(2)
    ledger = {}

    step("acceptance: orders flow, the core restarts and replays its journal, orders flow on")
    # The core stops with the round's gateway connected, as production gateways always are, and
    # starts at once.
    with connected_round(stack, ledger, next(gateways)) as ledger:
        stack.compose("stop", f"--timeout={STOP_SECONDS}", "core")
    stack.compose("up", "--detach", "--wait", "core")
    ledger = ledger_round(stack, ledger, next(gateways))
    check_engine_state(stack, connectors, descriptor, ledger)

    step("acceptance: Kafka is out while orders flow")
    with brokers_paused(stack):
        ledger = ledger_round(stack, ledger, next(gateways))
    check_engine_state(stack, connectors, descriptor, ledger)

    step("acceptance: kill -9 of the core with events queued behind paused brokers")
    # Every command of the ledger publishes a balance: the latest is the last command's.
    last_before = max(balance["timestamp"] for balance in ledger["balances"].values())
    with brokers_paused(stack):
        ledger = ledger_round(stack, ledger, next(gateways))
        stack.compose("kill", "--signal=SIGKILL", "core")
    # No command of the round was acknowledged, so the delivered watermark the core left is below
    # all of them: its restart must re-publish them all.
    watermark = delivered_watermark(stack)
    expect(
        watermark <= last_before,
        f"the core left the watermark {watermark}, past the round's commands (after {last_before})",
    )
    # No wait for the driver to release the killed core's journal: the core waits for that itself
    # (vex-core#201).
    stack.compose("up", "--detach", "--wait", "core")
    check_engine_state(stack, connectors, descriptor, ledger)


@contextlib.contextmanager
def connected_round(stack, ledger, gateway):
    """Runs one round of the traffic driver's ledger against the running core, as gateway
    `gateway`, and yields the ledger it continues: {topic: {Kafka key: event in proto3 JSON}},
    the latest event per key of balances, orders and orderbook that the core's answers say it
    published. The gateway stays connected until the block ends."""
    driver = subprocess.Popen(
        stack.command("run", "--rm", "--no-deps", "-T", "traffic", "ledger", str(gateway)),
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=stack.env,
    )
    try:
        driver.stdin.write(json.dumps(ledger) + "\n")
        driver.stdin.flush()
        continued = driver.stdout.readline()
        expect(continued, f"gateway {gateway}'s round wrote no ledger")
        yield json.loads(continued)
    finally:
        driver.stdin.close()
        status = driver.wait()
    expect(status == 0, f"gateway {gateway}'s round exited {status}")


def ledger_round(stack, ledger, gateway):
    """One round of connected_round, whose gateway disconnects once it is done."""
    with connected_round(stack, ledger, gateway) as continued:
        return continued


def check_engine_state(stack, connectors, descriptor, ledger):
    """Every latest-state row is the engine's state, by the ledger: waits until Kafka's latest
    event for each key in the ledger is the ledger's, then until the connectors have caught up,
    and checks that the latest-state tables hold the ledger's rows, and every table, in Postgres
    and ClickHouse, what Kafka holds."""
    latest_state = {
        config["topics"]: config
        for config in postgres_connectors(connectors).values()
        if "event_ts" not in config["pk.fields"].split(",")
    }
    expect(set(latest_state) == {"balances", "orders", "orderbook"}, sorted(latest_state))
    engine = {
        topic: rows_of(ledger[topic].values(), config, descriptor)
        for topic, config in latest_state.items()
    }
    deadline = time.monotonic() + CATCH_UP_SECONDS
    while True:
        differing = {}
        for topic, config in latest_state.items():
            kafka = rows_of(published(stack, topic), config, descriptor)
            keys = [key for key, row in engine[topic].items() if kafka.get(key) != row]
            if keys:
                differing[topic] = keys
        if not differing:
            break
        expect(
            time.monotonic() < deadline,
            f"Kafka's latest events are not the engine's state for {differing}",
        )
        time.sleep(2)
    wait_until_caught_up(stack, connectors)
    for topic, config in latest_state.items():
        table = config["table.name.format"]
        stored = {key_of(row, config): row for row in stack.json_rows(table)}
        rows = {key: stored.get(key) for key in engine[topic]}
        expect(rows == engine[topic], f"{table} holds {rows}, the engine {engine[topic]}")
    check_tables_hold_the_topics(stack, connectors, descriptor)


@contextlib.contextmanager
def brokers_paused(stack):
    """Freezes every broker (docker pause) for the duration, then waits until they serve again
    (wait_for_kafka)."""
    stack.compose("pause", *BROKERS)
    try:
        yield
    finally:
        stack.compose("unpause", *BROKERS)
    wait_for_kafka(stack)


def wait_for_kafka(stack):
    """Waits until every broker passes its healthcheck again (compose.yml: the cluster is healthy
    and the Schema Registry answers). docker pause marks a broker unhealthy until then: Compose
    starts no service that depends on it, and a read begun sooner can hang (RPK_SECONDS)."""
    start = time.monotonic()
    while True:
        health = stack.compose("ps", "--format={{.Health}}", *BROKERS).split()
        if health == ["healthy"] * len(BROKERS):
            print(f"    the brokers serve again after {time.monotonic() - start:.1f} s", flush=True)
            return
        expect(
            time.monotonic() < start + CATCH_UP_SECONDS,
            f"the brokers are {health} after {CATCH_UP_SECONDS} s",
        )
        time.sleep(1)


def delivered_watermark(stack):
    """The delivered watermark the core wrote last to vex-core.delivered: a big-endian u64 under
    the key w."""
    records = consume(stack, "vex-core.delivered", [r"--format=%p\t%k\t%v{hex}\n"])
    expect(records, "vex-core.delivered is empty")
    _, key, value = records[-1]
    expect(key == "w", f"vex-core.delivered holds key {key!r}")
    return int(value, 16)


def check_tls(stack, connectors):
    step("Connect's sessions use TLS, and its role may not connect without")
    sessions = stack.psql(
        "SELECT count(*), bool_and(ssl) FROM pg_stat_ssl JOIN pg_stat_activity USING (pid) "
        "WHERE usename = 'readmodel_connect_login'"
    )
    count, all_tls = sessions.split("|")
    expect(int(count) > 0 and all_tls == "t", f"Connect's sessions, all over TLS: {sessions}")
    refused = stack.compose(
        "exec", "-T", "postgres", "psql", "--command=SELECT 1",
        "host=postgres dbname=readmodel user=readmodel_connect_login password=test-only "
        "sslmode=disable",
        check=False, stderr=subprocess.STDOUT,
    )
    expect("no encryption" in refused, refused)

    user = 4243
    written = f"SELECT count(*) FROM trading.deposit_events WHERE user_id = {user}"
    # Each certificate that must not verify, and what the worker logs when a task refuses it: for
    # untrusted.crt, whose CA has the trusted CA's name, "PKIX path validation failed" (a CA of
    # another name gives "PKIX path building failed"). A refused certificate is a failed
    # connection attempt: the task tries again after connection.backoff.ms, writing nothing.
    unverifiable = {"untrusted": "PKIX path", "wrong-name": "could not be verified"}
    for certificate, error in unverifiable.items():
        step(f"a connector refuses Postgres presenting {certificate}.crt, and writes nothing")
        serve_certificate(stack, certificate)
        now = str(time.time_ns())
        deposit = {"userId": str(user), "assetId": 1, "amount": "1", "timestamp": now}
        produce(stack, "deposits", user, "DepositEvent", [deposit])
        wait_for_log(stack, "pg-deposit-events", error)
        expect(stack.psql(written) == "0", f"a deposit landed through {certificate}.crt")

    step("with the CA's certificate back, the connector resumes by itself with no gap")
    serve_certificate(stack, "postgres")
    wait_until_caught_up(stack, connectors)
    count = stack.psql(written)
    expect(count == str(len(unverifiable)), f"{count} of user {user}'s deposits landed")


def check_clickhouse_outage_and_tls(stack, connectors):
    user = 4244
    written = f"SELECT count() FROM deposit_events FINAL WHERE user_id = {user}"

    def deposit():
        now = str(time.time_ns())
        produce(stack, "deposits", user, "DepositEvent", [
            {"userId": str(user), "assetId": 1, "amount": "1", "timestamp": now}
        ])

    step("ClickHouse is down: its connector retries, and writes once it is back")
    stopped_since = f"{time.time():.3f}"
    stack.compose("stop", "clickhouse")
    deposit()
    wait_for_log(stack, "clickhouse", "RetriableException", stopped_since)
    expect_no_failed_task(stack.connect("GET", "/connectors?expand=status"))
    stack.compose("up", "--detach", "--wait", "clickhouse")
    wait_until_caught_up(stack, connectors)
    expect(stack.clickhouse(written) == "1", "the deposit sent while ClickHouse was down is lost")

    # Each certificate that must not verify, and what the worker logs when the task refuses it.
    # The sink reports a refused certificate as a failed connection, which it retries.
    unverifiable = {
        "clickhouse-untrusted": "PKIX path",
        "wrong-name": "doesn't match any of the subject alternative names",
    }
    for certificate, error in unverifiable.items():
        step(f"the ClickHouse connector refuses ClickHouse presenting {certificate}.crt, and retries")
        served_since = f"{time.time():.3f}"
        serve_clickhouse_certificate(stack, certificate)
        deposit()
        wait_for_log(stack, "clickhouse", error, served_since)
        expect_no_failed_task(stack.connect("GET", "/connectors?expand=status"))
        expect(stack.clickhouse(written) == "1", f"a deposit landed through {certificate}.crt")

    step("with the CA's certificate back, the ClickHouse connector resumes by itself with no gap")
    serve_clickhouse_certificate(stack, "clickhouse")
    wait_until_caught_up(stack, connectors)
    count = stack.clickhouse(written)
    expect(count == str(1 + len(unverifiable)), f"{count} of user {user}'s deposits landed")


def serve_clickhouse_certificate(stack, name):
    """Makes ClickHouse present the certificate `name` from TLS_DIR, and restarts it, so that
    every connection to it is new."""
    for suffix in ("crt", "key"):
        stack.compose(
            "exec", "-T", "clickhouse", "install", "-o", "clickhouse", "-g", "clickhouse", "-m",
            "600", f"/tls/{name}.{suffix}", f"/etc/clickhouse-server/tls/clickhouse.{suffix}",
        )
    stack.compose("restart", "clickhouse")
    stack.compose("up", "--detach", "--wait", "clickhouse")


def wait_for_log(stack, connector, text, since=None):
    """Waits until the worker logs an entry of `connector`'s tasks with `text`, since the Unix time
    `since` or from the start: at most a connection backoff, and 30 s for the attempt after it."""
    deadline = time.monotonic() + int(CONNECTOR_RULES["connection.backoff.ms"]) / 1000 + 30
    while not logged(connect_log(stack, since), connector, text):
        status = stack.connect("GET", f"/connectors/{connector}/status")
        expect(time.monotonic() < deadline, f"{connector} logged no {text!r}; status {status}")
        time.sleep(1)


def serve_certificate(stack, name):
    """Makes Postgres present the certificate `name` from TLS_DIR to new connections, and ends
    Connect's sessions, so that its tasks connect anew."""
    for suffix in ("crt", "key"):
        stack.compose(
            "exec", "-T", "postgres", "install", "-o", "postgres", "-g", "postgres", "-m", "600",
            f"/tls/{name}.{suffix}", f"/var/lib/postgresql/tls/postgres.{suffix}",
        )
    stack.psql(
        "SELECT pg_reload_conf(); SELECT pg_terminate_backend(pid) FROM pg_stat_activity "
        "WHERE usename = 'readmodel_connect_login'"
    )


def check_skew(stack, connectors, clickhouse, core):
    step("a field without a column fails its connector, and nothing is skipped")
    # vex-core starts publishing a field that has no column yet.
    proto = (core / "processors" / "src" / "protos" / "trading.proto").read_text()
    evolved = proto.replace(
        "message DepositEvent {", "message DepositEvent {\n  uint64 skew_probe = 100;", 1
    )
    expect(evolved != proto, "trading.proto has no DepositEvent")
    stack.compose("exec", "-T", "redpanda-0", "sh", "-c", "cat > /tmp/evolved.proto", stdin=evolved)
    stack.rpk("registry", "schema", "create", "deposits-value", "--schema=/tmp/evolved.proto")
    user = 4242
    now = time.time_ns()
    deposits = [
        {
            "userId": str(user),
            "assetId": 1,
            "amount": "1",
            "timestamp": str(now + n),
            "skewProbe": str(n),
        }
        for n in range(3)
    ]
    produce(stack, "deposits", user, "DepositEvent", deposits[:1])

    tasks = wait_for_tasks(stack, "pg-deposit-events", lambda states: "FAILED" in states)
    expect(any("skew_probe" in task.get("trace", "") for task in tasks), tasks)
    produce(stack, "deposits", user, "DepositEvent", deposits[1:])
    written = stack.psql(f"SELECT count(*) FROM trading.deposit_events WHERE user_id = {user}")
    expect(written == "0", f"{written} deposits written past the failure")
    # ClickHouse's sink writes them, without the field: the coverage check is the guard there.
    wait_until_caught_up(stack, clickhouse)
    written = stack.clickhouse(f"SELECT count() FROM deposit_events FINAL WHERE user_id = {user}")
    expect(written == "3", f"ClickHouse has {written} of the 3 deposits")

    step("after the migration, a restart resumes with no gap")
    stack.psql("ALTER TABLE trading.deposit_events ADD COLUMN skew_probe BIGINT")
    restart(stack, "pg-deposit-events")
    wait_until_caught_up(stack, connectors)
    probes = stack.psql(
        f"SELECT skew_probe FROM trading.deposit_events WHERE user_id = {user} ORDER BY event_ts"
    ).split()
    expect(probes == ["0", "1", "2"], f"deposits {probes} landed, not 0, 1 and 2")


def produce(stack, topic, key, message, values):
    """Produces `values`, trading.proto `message`s in proto3 JSON, to `topic` under `key`, encoded
    with the latest schema registered for the topic."""
    stack.rpk(
        "topic", "produce", topic, f"--key={key}",
        "--schema-id=topic", f"--schema-type=trading.{message}",
        stdin="".join(json.dumps(value) + "\n" for value in values),
    )


def restart(stack, connector):
    """Restarts a connector with every one of its tasks and waits until they run. Every task: a
    failed task's partitions move to its siblings, which may be failing on the same record as the
    restart arrives, or still hold the table or the connection the failure was about."""
    stack.connect("POST", f"/connectors/{connector}/restart?includeTasks=true")
    wait_for_tasks(stack, connector, lambda states: states == {"RUNNING"})


def wait_for_tasks(stack, connector, done):
    """Waits until `done` holds for the set of the connector's task states; returns the tasks."""
    deadline = time.monotonic() + CATCH_UP_SECONDS
    while True:
        tasks = stack.connect("GET", f"/connectors/{connector}/status")["tasks"]
        if done({task["state"] for task in tasks}):
            return tasks
        expect(time.monotonic() < deadline, f"{connector}: tasks stayed {tasks}")
        time.sleep(1)


class Stack:
    """readmodel/tests/compose.yml as a Compose project of its own."""

    def __init__(self, core, rev, tls):
        self.project = f"vex-readmodel-{os.getpid()}"
        self.env = {**os.environ, "CORE_DIR": str(core), "CORE_REV": rev, "TLS_DIR": str(tls)}
        self.files = ["compose.yml"]
        # Each event topic's partition count, from vex-core's topics.toml (apply_topics).
        self.partitions = None
        # The Actions runtime, which ci.yml exposes to this step, reaches the Actions cache.
        if "ACTIONS_RUNTIME_TOKEN" in os.environ:
            self.files.append("compose.cache.yml")
        self.url = None

    def command(self, *args):
        project = ["--project-name", self.project]
        project += [option for file in self.files for option in ("--file", str(TESTS / file))]
        return ["docker", "compose", *project, *args]

    def compose(self, *args, **options):
        return run(self.command(*args), env=self.env, **options)

    def psql_command(self, database="readmodel"):
        """psql as the superuser, with unaligned output, fields split by "|"."""
        return self.command(
            "exec", "-T", "postgres", "psql", "--username=postgres", f"--dbname={database}",
            "--set=ON_ERROR_STOP=1", "--quiet", "--no-align", "--tuples-only",
        )

    def psql(self, sql, database="readmodel"):
        """Runs `sql` and returns its output (psql_command)."""
        return run(self.psql_command(database), stdin=sql, env=self.env).strip()

    def lock(self, tables):
        """Locks `tables` in SHARE mode, in a transaction that makes every write to them wait
        until the returned psql process ends, when its stdin closes."""
        holder = subprocess.Popen(
            self.psql_command(), stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
            env=self.env,
        )
        holder.stdin.write(f"BEGIN; LOCK {', '.join(tables)} IN SHARE MODE; SELECT 'locked';\n")
        holder.stdin.flush()
        expect(holder.stdout.readline() == "locked\n", f"could not lock {tables}")
        return holder

    def migrate(self, database="readmodel", **options):
        """Runs golang-migrate's `up` on `database` as the superuser; returns its output."""
        url = f"postgres://postgres:postgres@postgres:5432/{database}?sslmode=disable"
        return self.compose(
            "run", "--rm", "migrate", "-path=/migrations/postgres", f"-database={url}", "up",
            **options,
        )

    def migrate_clickhouse(self, database="readmodel", **options):
        """Runs golang-migrate's `up` on the ClickHouse `database` as its administrator; returns
        its output."""
        url = (
            "clickhouse://clickhouse:9000?username=admin&password=admin"
            f"&database={database}&x-multi-statement=true"
        )
        return self.compose(
            "run", "--rm", "migrate", "-path=/migrations/clickhouse", f"-database={url}", "up",
            **options,
        )

    def clickhouse(self, sql, user="admin", check=True, options=()):
        """Runs `sql` in the database readmodel with clickhouse-client, as `user` (the
        administrator, or a user with the test password), and returns its output, in
        TabSeparated; with `check` false, also its error."""
        password = "admin" if user == "admin" else "test-only"
        command = self.command(
            "exec", "-T", "clickhouse", "clickhouse-client", f"--user={user}",
            f"--password={password}", "--database=readmodel", "--multiquery", *options,
        )
        stderr = subprocess.PIPE if check else subprocess.STDOUT
        return run(command, stdin=sql, env=self.env, check=check, stderr=stderr).strip()

    def clickhouse_rows(self, sql):
        """The rows `sql` selects, as dicts, with 64-bit integers as numbers."""
        output = self.clickhouse(
            sql,
            options=["--format=JSONEachRow", "--output_format_json_quote_64bit_integers=0"],
        )
        return [json.loads(line) for line in output.splitlines()]

    def clickhouse_schema(self):
        return self.clickhouse(
            "SELECT name, create_table_query FROM system.tables WHERE database = 'readmodel' "
            "AND name <> 'schema_migrations' ORDER BY name FORMAT TSVRaw; "
            "SELECT * FROM system.grants WHERE database = 'readmodel' ORDER BY ALL FORMAT TSVRaw"
        )

    def clickhouse_tables(self, engine):
        return self.clickhouse(
            "SELECT name FROM system.tables "
            f"WHERE database = 'readmodel' AND engine = '{engine}' AND name <> 'schema_migrations'"
        ).split()

    def clickhouse_snapshot(self):
        """Every MergeTree table, read through FINAL, and every view, as text."""
        sources = [f"{table} FINAL" for table in self.clickhouse_tables("ReplacingMergeTree")]
        sources += self.clickhouse_tables("View")
        return {
            source: self.clickhouse(f"SELECT * FROM {source} ORDER BY ALL") for source in sources
        }

    def clickhouse_stored_rows(self):
        """The rows each MergeTree table stores, copies of an event that are not merged yet
        included."""
        return {
            table: int(self.clickhouse(f"SELECT count() FROM {table}"))
            for table in self.clickhouse_tables("ReplacingMergeTree")
        }

    def json_rows(self, table):
        rows = self.psql(f"SELECT row_to_json(t) FROM {table} t")
        return [json.loads(line) for line in rows.splitlines()]

    def snapshot(self):
        tables = self.psql(
            "SELECT 'trading.' || tablename FROM pg_tables WHERE schemaname = 'trading'"
        ).split()
        return {table: sorted(self.json_rows(table), key=canonical) for table in tables}

    def dump_schema(self):
        # A fixed \restrict key: pg_dump otherwise writes a random one into every dump.
        return self.compose(
            "exec", "-T", "postgres", "pg_dump", "--username=postgres", "--schema-only",
            "--restrict-key=readmodel", "readmodel",
        )

    def rpk(self, *args, stdin=None):
        """rpk in redpanda-0, killed there if it runs for RPK_SECONDS: run() then raises
        TimeoutError."""
        return self.compose(
            "exec", "-T", "redpanda-0", "timeout", "--signal=KILL", str(RPK_SECONDS), "rpk", *args,
            stdin=stdin,
        )

    def connect_url(self):
        if self.url is None:
            self.url = "http://" + self.compose("port", "connect", "8083").strip()
        return self.url

    def connect(self, method, path, body=None):
        """A Kafka Connect REST call; returns the decoded response."""
        request = urllib.request.Request(
            self.connect_url() + path,
            method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read()
        return json.loads(body) if body else None


def pinned_core_rev():
    lock = (ROOT / "Cargo.lock").read_text()
    revs = set(re.findall(r'"git\+https://github\.com/trade-vex/vex-core\?rev=\w+#(\w+)"', lock))
    expect(len(revs) == 1, f"Cargo.lock must pin one vex-core rev, found {revs}")
    return revs.pop()


def run(args, stdin=None, capture=True, check=True, stderr=subprocess.PIPE, env=None):
    """Runs `args` and returns its output; fails on a non-zero exit unless `check` is false: with
    TimeoutError for exit status 137 (SIGKILL: Stack.rpk's bound, though the OOM killer gives it
    too), and RuntimeError otherwise. Only Kafka reads catch TimeoutError (consume and
    wait_until_caught_up)."""
    result = subprocess.run(
        args,
        input=stdin,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=stderr if capture else None,
        env=env,
    )
    if check and result.returncode != 0:
        output = (result.stdout or "") + (result.stderr or "")
        failure = TimeoutError if result.returncode == 137 else RuntimeError
        raise failure(f"{' '.join(map(str, args))} exited {result.returncode}\n{output}")
    return result.stdout or ""


def canonical(row):
    return json.dumps(row, sort_keys=True)


def expect(condition, message):
    if not condition:
        raise AssertionError(message)


def step(message):
    print(f"==> {message}", flush=True)


if __name__ == "__main__":
    sys.exit(main())

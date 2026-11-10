#!/usr/bin/env python3
"""The read model's tests, against a real stack (trade-vex/vex-core#189).

Checks that apply-connectors.sh retries a request Connect refuses while it rebalances, a bounded
number of times, and no other, and that one put() of a connector blocks for less than the sink
consumers' max.poll.interval.ms. Then builds and starts readmodel/tests/compose.yml with vex-core
at the rev Cargo.lock pins, and with Postgres serving TLS under a throwaway CA, and checks:
- the migrations refuse a database in which roles other than its owner may create in schema
  public, apply to an empty database, and a second run is a no-op;
- the seeds apply, twice;
- schema.sql: the keep_newer guard on every trading table, the guard itself, trading.u64 and the
  roles' privileges;
- every field of every trading.proto event, after Flatten, has a column of the right type in its
  connectors' tables, and every connector follows the read model's rules;
- end to end, with traffic through the real core and Postgres out for 90 s, first hung with a
  statement of every connector in flight, then stopped: no connector task fails, every connector
  cuts off its statement, no later than check_put_bound assumes, and its login that gets no answer,
  and retries its refused logins, every topic lands in its tables, a latest-state row holds its
  key's greatest event_ts, u64 values above i64::MAX keep their bits, replaying an out-of-order
  and duplicated suffix changes no table, Connect reaches Postgres over TLS only, a connector
  refuses a server certificate its CA did not sign or that names another host and resumes by
  itself once the right one is back, and a field without a column stops its connector, skipping
  nothing, until the migration adds the column and a restart resumes it.

Needs git, openssl, Docker with Compose v2, and network access to GitHub. Stops and removes the
stack at the end, also on failure. In GitHub Actions, with the Actions runtime exposed to it
(ci.yml), it keeps the images' layers in the Actions cache (compose.cache.yml).

    python3 readmodel/tests/run.py
"""

import http.server
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
READMODEL = ROOT / "readmodel"
TESTS = READMODEL / "tests"
CORE_REPOSITORY = "https://github.com/trade-vex/vex-core"

# vex-core's topics and the trading.proto message each carries (vex-core processors/src/events.rs).
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
PARTITIONS = 3
# The column type of each proto field type (design doc §3.4). A type missing here has none yet.
COLUMN_TYPES = {
    "TYPE_UINT64": "bigint",
    "TYPE_UINT32": "bigint",
    "TYPE_ENUM": "text",
    "TYPE_STRING": "text",
}

SECRETS = "/etc/vex-connect/postgres.properties"
POSTGRES_CA = "/etc/vex-connect/postgres-ca.pem"
# Settings every connector has, with these values.
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
# Settings that differ between connectors. Anything else (a dead-letter queue, say) is refused.
CONNECTOR_SETTINGS = {
    "tasks.max",
    "topics",
    "table.name.format",
    "pk.fields",
    "transforms.event_ts.renames",
}

# What readmodel/tests/traffic sends: user 2^63 + 1 trades USD and BTC, user 3 deposits 10^19 ETH.
MAKER = 2**63 + 1
WHALE = 3
ETH = 3
WHALE_ETH = 10**19
CATCH_UP_SECONDS = 180
# How long Postgres is out after the traffic, counted from when it hangs: within what the
# connectors ride out.
OUTAGE_SECONDS = 90
# How long the JDBC sink waits for the check of a cached connection (CachedConnectionProvider).
VALIDITY_CHECK_SECONDS = 5


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
        step("generate the stack's CA and Postgres certificates")
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
    stack.compose("--profile", "tools", "build", "connect", "core", "traffic", capture=False)
    step("start Redpanda and Postgres")
    stack.compose("up", "--detach", "--wait", "redpanda-0", "redpanda-1", "redpanda-2", "postgres")
    # The brokers do not create topics: every topic exists, with a fixed partition count.
    stack.rpk("topic", "create", *TOPIC_MESSAGES, f"--partitions={PARTITIONS}", "--replicas=3")

    check_migrations(stack)
    check_seeds(stack)
    step("schema.sql: triggers, guard, trading.u64, roles")
    stack.psql((TESTS / "schema.sql").read_text())

    connectors = {
        path.stem: json.loads(path.read_text())
        for path in sorted((READMODEL / "connect" / "connectors").glob("*.json"))
    }
    descriptor = proto_descriptor(stack)
    check_coverage(stack, connectors, descriptor)
    start_connect(stack, connectors)
    drive_traffic_through_an_outage(stack, connectors)

    check_tables_hold_the_topics(stack, connectors, descriptor)
    check_u64_round_trip(stack, connectors)
    check_replayed_suffix_changes_nothing(stack, connectors)
    check_tls(stack, connectors)
    check_skew(stack, connectors, core)


def make_certificates(directory):
    """Writes into `directory` a throwaway CA (ca.crt) and, each with its key, the certificate it
    signs for the host name the connectors reach Postgres by (postgres.crt), and two that must not
    verify: one another CA of the same name signs (untrusted.crt), and one the CA signs for another
    host name (wrong-name.crt)."""

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
    certificate("untrusted", "postgres", ca="rogue-ca")
    certificate("wrong-name", "impostor", ca="ca")


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


def check_seeds(stack):
    step("the seeds apply, and again with no change")
    seeds = (READMODEL / "seeds" / "development.sql").read_text()
    rows = "SELECT * FROM assets ORDER BY asset_id; SELECT * FROM markets ORDER BY market_id"
    stack.psql(seeds)
    seeded = stack.psql(rows)
    stack.psql(seeds)
    expect(seeded and stack.psql(rows) == seeded, seeded)


def proto_descriptor(stack):
    """trading.proto at the pinned rev, compiled by buf: its messages and enums by name."""
    image = json.loads(stack.compose("run", "--rm", "buf"))
    (proto,) = [file for file in image["file"] if file["name"] == "trading.proto"]
    return {
        "messages": {message["name"]: message for message in proto["messageType"]},
        "enums": {enum["name"]: enum for enum in proto.get("enumType", [])},
    }


def connector_columns(config, descriptor):
    """{column: path of proto fields} of a connector's table: its topic's message after Flatten
    (delimiter "_") and the event_ts rename."""
    renames = dict(pair.split(":") for pair in config["transforms.event_ts.renames"].split(","))

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

    return dict(flatten(TOPIC_MESSAGES[config["topics"]], ()))


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
            for column, path in connector_columns(config, descriptor).items()
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


def drive_traffic_through_an_outage(stack, connectors):
    step("drive traffic through vex-core with the connectors paused")
    for name in connectors:
        stack.connect("PUT", f"/connectors/{name}/pause")
    for name in connectors:
        wait_for_tasks(stack, name, lambda states: states == {"PAUSED"})
    stack.compose("up", "--detach", "--wait", "core")
    stack.compose("run", "--rm", "traffic", capture=False)

    step("lock the trading tables and resume the connectors: each one's upserts wait")
    # So that every connector has a statement in flight when Postgres hangs, each begun a moment
    # before: it is cut off while Postgres is hung.
    tables = sorted(config["table.name.format"] for config in connectors.values())
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
        wait_for_every_connector(stack, connectors, hung_since, hung + within, {
            "statement cut off for want of an answer": [("Write of", "Read timed out")],
            "login cut off for want of an answer": [("Unable to connect", "timed out")],
        })
        check_cut_off_times(stack, connectors, hung_since, started, cut_off)
    finally:
        stack.compose("unpause", "postgres")
        lock.communicate(timeout=30)

    step("then stopped: logins are refused")
    stopped_since = f"{time.time():.3f}"
    stack.compose("stop", "postgres")
    within = time.monotonic() + int(CONNECTOR_RULES["connection.backoff.ms"]) / 1000 + 30
    wait_for_every_connector(stack, connectors, stopped_since, within, {
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


def wait_for_every_connector(stack, connectors, since, deadline, expected):
    """Waits, while no connector task fails, until the worker has logged, since the Unix time
    `since` and for every connector, each of `expected`: {what: entries that show it, each as the
    texts it contains}. Fails after the monotonic time `deadline`."""
    while True:
        expect_no_failed_task(stack.connect("GET", "/connectors?expand=status"))
        entries = connect_log(stack, since)
        missing = {
            name: [
                what
                for what, shown_by in expected.items()
                if not any(logged(entries, name, *texts) for texts in shown_by)
            ]
            for name in connectors
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
    """Waits until every connector task runs and has committed the end of its topic, and no topic
    has grown since the previous look. With `every_topic_published`, also until no topic is
    empty: vex-core publishes after it answers."""
    deadline = time.monotonic() + CATCH_UP_SECONDS
    previous_ends = None
    while True:
        states = stack.connect("GET", "/connectors?expand=status")
        expect_no_failed_task(states)
        group_names = [f"connect-{name}" for name in connectors]
        groups = json.loads(stack.rpk("group", "describe", *group_names, "--format=json"))
        ends = {
            (partition["topic"], partition["partition"]): partition["log_end_offset"]
            for group in groups
            for partition in group["partitions"]
        }
        lagging = [
            group["group_name"]
            for group in groups
            if len(group["partitions"]) != PARTITIONS
            or any(not committed_end(partition) for partition in group["partitions"])
        ] + [
            name
            for name, state in states.items()
            if any(task["state"] != "RUNNING" for task in state["status"]["tasks"])
        ]
        empty = [
            topic
            for topic in TOPIC_MESSAGES
            if every_topic_published and not any(ends.get((topic, p)) for p in range(PARTITIONS))
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
    """Every record of `topic`, one line each: partition, key, value (in `value_format`)."""
    lines = stack.rpk("topic", "consume", topic, "--offset=:end", *value_format).splitlines()
    return [line.split("\t", 2) for line in lines]


def row_of(value, config, descriptor):
    """The row a connector writes for one event, decoded to proto3 JSON. That JSON leaves out
    default values, which Connect writes."""
    row = {}
    for column, path in connector_columns(config, descriptor).items():
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
            # proto3 JSON writes 64-bit integers as strings; the column holds their bits.
            number = int(field_value or 0)
            row[column] = number - 2**64 if number >= 2**63 else number
    return row


def check_tables_hold_the_topics(stack, connectors, descriptor):
    step("every topic lands in its tables; a latest-state row holds its key's greatest event_ts")
    events = {}
    for topic in TOPIC_MESSAGES:
        records = consume(stack, topic, ["--use-schema-registry=value", r"--format=%p\t%k\t%v\n"])
        events[topic] = [json.loads(value) for _, _, value in records]
        expect(events[topic], f"vex-core published nothing on {topic}")
    for name, config in connectors.items():
        primary_key = config["pk.fields"].split(",")
        expected = {}
        # A key's events share a partition, in the order the sink reads them: the first copy of
        # a (key, event_ts) stays, and a latest-state row keeps the greatest event_ts.
        for row in (row_of(value, config, descriptor) for value in events[config["topics"]]):
            key = tuple(row[column] for column in primary_key)
            if key not in expected or row["event_ts"] > expected[key]["event_ts"]:
                expected[key] = row
        rows = stack.json_rows(config["table.name.format"])
        expect(
            sorted(rows, key=canonical) == sorted(expected.values(), key=canonical),
            f"{name}: {config['table.name.format']} holds {rows}, not {list(expected.values())}",
        )


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


def check_replayed_suffix_changes_nothing(stack, connectors):
    step("replaying an out-of-order, duplicated suffix changes no table")
    before = stack.snapshot()
    for topic in TOPIC_MESSAGES:
        records = defaultdict(list)
        for partition, key, value in consume(stack, topic, [r"--format=%p\t%k\t%v{base64}\n"]):
            records[partition].append(f"{partition}\t{key}\t{value}\n")
        suffix = [record for log in records.values() for record in log[len(log) // 2 :]]
        # Each record again, then every one once more in reverse: older copies after newer
        # events, which a missing guard would leave as the latest rows.
        replay = suffix + suffix[::-1]
        stack.rpk(
            "topic", "produce", topic, "--partition=0", r"--format=%p\t%k\t%v{base64}\n",
            stdin="".join(replay),
        )
    wait_until_caught_up(stack, connectors)
    expect(stack.snapshot() == before, "the replayed suffix changed a table")


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


def wait_for_log(stack, connector, text):
    """Waits until the worker logs an entry of `connector`'s tasks with `text`: at most a
    connection backoff, and 30 s for the attempt after it."""
    deadline = time.monotonic() + int(CONNECTOR_RULES["connection.backoff.ms"]) / 1000 + 30
    while not logged(connect_log(stack), connector, text):
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


def check_skew(stack, connectors, core):
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
            "run", "--rm", "migrate", "-path=/migrations", f"-database={url}", "up", **options
        )

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
        return self.compose("exec", "-T", "redpanda-0", "rpk", *args, stdin=stdin)

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
    """Runs `args` and returns its output; fails on a non-zero exit unless `check` is false."""
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
        raise RuntimeError(f"{' '.join(map(str, args))} exited {result.returncode}\n{output}")
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

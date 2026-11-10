# Read model

The gateway's Postgres and ClickHouse schemas and the Kafka Connect deployment that fills their
event tables from vex-core's events (trade-vex/vex-core#189). RUNBOOK.md "Read Model" has the
design in brief, how to bring up an environment, the roles, and what to do when a connector
fails.

| Path | What |
|---|---|
| `migrations/postgres/`, `migrations/clickhouse/` | The schemas: forward-only golang-migrate migrations, plain SQL. |
| `seeds/` | `readmodel-seeds`, which writes the `markets` and `assets` rows, for Postgres or ClickHouse, from the `symbols` section of a vex-core configuration file, read and validated by vex-config, and `development.sql` and `development.clickhouse.sql`, its output for the pinned vex-core's `config.dev.yaml`. |
| `connect/` | The Connect worker image, its configuration, one JDBC sink connector per `trading` table, one ClickHouse sink connector for every topic and `apply-connectors.sh`. |
| `tests/` | `run.py` and the stack it runs (`compose.yml`): the migration, schema, coverage and end-to-end tests, with real traffic through vex-core from `traffic/`, the topics from vex-core's `deploy/kafka/topics.toml`, and the acceptance tests: after a restart of vex-core, a Kafka outage and a `kill -9` of vex-core, the latest-state tables hold the engine's state, and every Postgres table, ClickHouse table (read through `FINAL`) and per-minute table holds what Kafka holds. |

```bash
python3 readmodel/tests/run.py       # needs Docker and openssl; minutes once the images are built
cargo test -p readmodel-seeds        # the development seeds match the pinned vex-core
```

Each Redpanda broker reserves Linux AIO contexts at startup. On a host where other processes hold
most of `fs.aio-max-nr`, a broker exits with "does not satisfy minimum AIO requirements"; raise
the limit (`sysctl -w fs.aio-max-nr=1048576`) or stop them.

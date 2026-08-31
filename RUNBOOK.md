# VEX Gateway Service Runbook

## Table of Contents
1. [Deployment](#deployment)
2. [Configuration](#configuration)
3. [Common Operations](#common-operations)
4. [Troubleshooting](#troubleshooting)
5. [Emergency Procedures](#emergency-procedures)
6. [API Reference](#api-reference)

---

## Deployment

### Prerequisites

1. **Rust Toolchain**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   rustup default stable
   ```

2. **System Dependencies**
   - PostgreSQL client libraries
   - Redis client libraries
   - Aeron media driver (for VEX Core communication)

### Build

```bash
# Build release binary
cargo build --release

# Binary location: target/release/gateway
```

### Run

```bash
# Development mode (uses development.toml config)
APP_ENV=development ./target/release/gateway

# Production mode (requires production.toml config)
APP_ENV=production ./target/release/gateway
```

### Environment Variables

The gateway reads two environment variables by name, and does not read a `.env` file:

- `APP_ENV` (required): `development`, `testing` or `production`. Selects `<APP_ENV>.toml`, whose
  `environment` key must name the same environment.
- `APP_CONFIG_DIR` (optional): the directory holding that file. The gateway refuses to start
  when it does not name a directory. When it is unset, the gateway uses the nearest `config/`
  directory at or above its working directory.

They choose the configuration file, so they cannot be set in it. Every other setting (server,
logging, database, Redis, ClickHouse, JWT, email, OAuth, trusted proxies, the vex-core link) lives
in that file (see [Configuration](#configuration)), and the `APP__` layer of
[Configuration Precedence](#configuration-precedence) is the only way to override one:

```bash
APP_ENV=production
APP__SERVER__HOST=0.0.0.0                                  # server.host
APP__AUTH__SERVER__TRUSTED_PROXIES=10.0.1.0/24,10.0.2.0/24 # auth.server.trusted_proxies
APP__CORE__AUTH_KEY=<signed 32-bit integer>                # core.auth_key, kept out of the file
```

---

## Configuration

### Configuration Files

Configuration is loaded from TOML files in the `config/` directory:

- `config/development.toml` - Development settings; `scripts/setup_dev.sh` creates it from the
  committed `config/development.toml.example`
- `config/testing.toml` - Testing settings
- `config/production.toml` - Production settings

The three environment files are gitignored. The selected environment's file must exist: without
it the gateway does not start.

### Configuration Precedence

1. Environment-specific TOML file
2. Environment variables `APP__<SECTION>__<KEY>`, named like vex-core's `VEX__` variables: the key
   path in upper case with `__` between its levels. `APP__SERVER__PORT` overrides `server.port`,
   `APP__AUTH__SERVER__TRUSTED_PROXIES` overrides `auth.server.trusted_proxies` and
   `APP__CORE__AUTH_KEY` overrides `core.auth_key`. A list takes comma-separated values
   (`10.0.1.0/24,10.0.2.0/24`). `APP_ENV` and `APP_CONFIG_DIR`, with a single `_`, are not
   overrides.

An `APP__` variable that names no key stops the gateway, and the error names the variable; so does
a key in the file that no setting reads. A missing required setting, a malformed file or an invalid
value stops the gateway in every environment, development included. Only optional settings have
defaults. The error never shows a value, so a secret set in the wrong place stays out of the log.
It names the setting (its variable, or its key and the file) and what the setting expects; for a
section or list given a single value (`APP__AUTH__DATABASE=…` instead of
`APP__AUTH__DATABASE__URL=…`) it names only what was expected. An `APP__` variable whose name
or value is not UTF-8 also stops the gateway, and the error names it. Apart from `APP_ENV` and
`APP_CONFIG_DIR`, the configuration reads no other variable, so another program's variable,
whatever it holds, cannot stop the gateway.

The `config` library converts the values leniently, and the gateway keeps its rules. A numeric
setting reads `true`, `yes` and `on` as 1 and `false`, `no` and `off` as 0. In the file a decimal
is rounded (`port = 8080.6` is 8081, and a negative decimal is 0 for a setting that cannot be
negative), while in a variable it is an error. Key names are not case-sensitive. A variable's
value reaches a text setting verbatim; an empty variable sets an empty value and cannot unset an
optional setting.

### Configuration Validation

The service validates configuration on startup:
- **Production**: Cannot bind to localhost, cannot use debug logging, port must be > 0,
  `auth.server.trusted_proxies` and `core.auth_key` must not be empty
- **Development**: More lenient validation
- **Testing**: Allows ephemeral ports (port 0)
- **Every environment**: `auth.oauth.providers` may list only `google`, `github` and `twitter`;
  another provider stops the gateway at startup, and the error names it

### OAuth Redirects

Once `auth.oauth.providers` lists a provider, the gateway needs the two URLs a completed OAuth
login is redirected to, and refuses to start without them:

- `auth.oauth.frontend_url_web`: the web frontend page that finishes a browser login (the session
  cookie is already set).
- `auth.oauth.frontend_url_app`: the mobile app's landing URL, usually a custom-scheme deep link
  such as `vex://oauth`. It receives only a one-time `login_code`.

### ClickHouse

`clickhouse.url` (required, no default) is the ClickHouse HTTP endpoint the portfolio and
balance-chart endpoints read from, such as `http://clickhouse.internal:8123`. Override it with
`APP__CLICKHOUSE__URL`.

### VEX Core Connection

The `[core]` section holds the Aeron link to vex-core (see `config/development.toml.example`).
`APP__CORE__<KEY>` overrides a key, such as `APP__CORE__CONTROL_PORT` for `core.control_port`:

- `gateway_id`: this gateway's id at the core, 0-15, unique among the gateways connected to it.
- `auth_key`: the handshake credential (below).
- `address`: vex-core's Aeron address.
- `port`, `control_port`: vex-core's `core_networking.initial_port` and `initial_control_port`:
  3521 and 3522 for a production core, 40001 and 40002 for vex-core's development and test
  defaults. The two must differ.
- `context_dir`: this gateway's Aeron media driver directory.

`auth_key` is a signed 32-bit integer equal to vex-core's
`VEX__GATEWAY_NETWORKING__GATEWAY_AUTHENTICATION_KEY`, shared by every gateway. With
`APP_ENV=production` the gateway refuses to start when it is empty, as vex-core does. Leave it
empty only for a core running without authentication
(`VEX__CORE_NETWORKING__ENABLE_AUTHENTICATION=false`). To keep it out of the file, set
`APP__CORE__AUTH_KEY`. The key travels in cleartext in the handshake, so keep gateway-to-core traffic
on a trusted network segment.

The gateway exits at startup when one of these keys is missing, malformed or unknown, or when
vex-core's own rules reject the combination (`Invalid vex-core link settings in [core]: ...`, for
example equal ports).

### Client IP Behind a Proxy

Login, signup, email-verification and OAuth rate limits, and API-key IP blacklists, key on the
client IP. By default that is the TCP peer, so behind a load balancer every client shares the
balancer's address: one client's failed logins then lock that email out for everyone.

Set `auth.server.trusted_proxies` in the TOML file, or `APP__AUTH__SERVER__TRUSTED_PROXIES`
(comma-separated), to the addresses or CIDR ranges of the proxies in front of the gateway. Only for
a request whose TCP peer is in that list is `X-Forwarded-For` read, from the right; the first hop
that is not itself a trusted proxy is the client. List only proxies that append to
`X-Forwarded-For`: a listed address that forwards a client's header unchanged lets that client
choose its own IP. An invalid entry stops the gateway at startup, before it connects to vex-core.
The error gives the entry's position in the list, never its value; empty items of a
comma-separated value are skipped and not counted.

With `APP_ENV=production` the list is required: the gateway refuses to start when it is empty.
Development and testing default to an empty list, so the TCP peer is the client.

Production runs behind an AWS Application Load Balancer (ALB):

- Set `auth.server.trusted_proxies` to the CIDR ranges of the ALB's subnets. ALB nodes take their
  addresses from those subnets and the addresses change, so list the subnets, not node IPs.
- The gateway's security group must accept inbound traffic only from the ALB's security group.
  Anything else that could connect from inside those subnets would be trusted, and could set its
  own `X-Forwarded-For` to choose its client IP.
- Keep the ALB attribute `routing.http.xff_header_processing.mode` at `append` (the default).
  `preserve` forwards the client's header unchanged and `remove` drops it.
- Keep `routing.http.xff_client_port.enabled` at `false` (the default). With it on, the ALB appends
  `ip:port`, which is not a bare IP, so every client resolves to the ALB's address.
- Terminate HTTPS at the ALB and keep the `X-Forwarded-Proto` header it sets: the gateway marks
  session cookies `Secure` when a trusted proxy's `X-Forwarded-Proto` (its last value) is `https`,
  or when the connection to the gateway itself uses TLS. From any other peer the header is
  ignored.

### Redis Eviction Policy

Run Redis with `maxmemory-policy noeviction`. Two kinds of gateway state must never be evicted:

- `token_version:<user_id>` has no TTL and is what revokes a user's outstanding access and refresh
  tokens after logout, password change or password reset. An `allkeys-*` policy can evict it, which
  silently makes those tokens valid again.
- Order idempotency claims (`vex:order-idempotency:*`, 24-hour TTL) stop a retried order from being
  published twice. A `volatile-*` or `allkeys-*` policy can evict a claim early, and the retry is
  then accepted as a new order.

With `noeviction`, a full Redis refuses writes and the affected requests fail instead.

### Balance Cache

The gateway only reads balances from Redis. It lists a user's assets from the set
`user:<user_id>:balances` and then reads each `user:<user_id>:asset:<asset_id>:balance` hash.
When the set is empty, or lists an asset without a readable hash, the gateway serves the user's
balances from Postgres instead.

The Events Handler must therefore `SADD` the asset id to that set in the same `MULTI`/`EXEC` as the
`HSET` of the balance hash. A `SADD` that lands after the `HSET` leaves a window in which the new
asset is missing from an otherwise complete answer. Until the writer maintains the set, every
balance read is served from Postgres, which can lag Redis.

### Generate JWT Keys

```bash
# Use provided script
./scripts/generate_keys.sh

# Or manually
openssl genrsa -out private_key.pem 2048
openssl rsa -in private_key.pem -pubout -out public_key.pem
```

---

## Common Operations

### Start Service

```bash
# Development
APP_ENV=development cargo run --bin gateway

# Production
APP_ENV=production ./target/release/gateway
```

### Stop Service

```bash
# Graceful shutdown (SIGTERM)
kill -TERM <pid>

# Force stop (SIGKILL) - use only if graceful shutdown fails
kill -9 <pid>
```

### Restart Service

```bash
# Graceful restart
kill -TERM <pid>
# Wait for shutdown, then start again
./target/release/gateway
```

### View Logs

```bash
# If logging to file
tail -f logs/app.log

# If logging to stdout (development)
# Logs appear in terminal
```

---

## Troubleshooting

### Service Won't Start

**Symptom**: Service exits immediately or fails to bind to port

**Diagnosis**:
```bash
# Check configuration
APP_ENV=production cargo run --bin gateway 2>&1 | grep -i "config\|error"

# Check port availability
netstat -tuln | grep 8081
lsof -i :8081

# Check configuration file exists
ls -la config/production.toml
```

**Solutions**:
- Verify `config/production.toml` exists: the gateway does not start without the selected
  environment's file, and the error names the path it expected. `APP__` variables only override
  settings in it.
- Check port is not already in use
- Verify environment variables are set correctly
- Check file permissions for JWT keys

### Database Connection Failures

**Symptom**: Logs show "database connection" errors

**Diagnosis**:
```bash
# Test the database connection with the configured auth.database.url
psql "<auth.database.url>" -c "SELECT 1"
```

**Solutions**:
- Verify `auth.database.url` is correct
- Check PostgreSQL is running: `pg_isready`
- Check network connectivity to database host

### Redis Connection Failures

**Symptom**: Logs show Redis errors, OAuth/session failures

**Diagnosis**:
```bash
# Test the Redis connection with the configured auth.redis.url
redis-cli -u "<auth.redis.url>" ping

# Check Redis is running
redis-cli -u "<auth.redis.url>" INFO server
```

**Solutions**:
- Verify `auth.redis.url` is correct (API-key signature replay protection uses the same Redis)
- Check Redis is running
- Verify network connectivity
- Check Redis max connections limit

### VEX Core Communication Failures

**Symptom**: Orders fail with "Failed to send order" errors

**Diagnosis**:
```bash
# Check Aeron context directory exists
ls -la <core.context_dir>

# Check VEX Core is running

# Check network connectivity
ping <core.address>
```

**Solutions**:
- Verify `core.context_dir` exists and is writable
- Check VEX Core service is running
- Verify `core.address`, `core.port` and `core.control_port` are correct (see "VEX Core Connection")
- `Connection rejected: Handshake rejected` at startup, with `handshake_rejected` and
  `reason=Authentication failed: Invalid credentials` in the vex-core log, means `core.auth_key`
  differs from vex-core's key
- Check Aeron media driver is running
- Verify network connectivity to VEX Core

### Order Timeouts

**Symptom**: Orders return 408 timeout responses

**Diagnosis**:
```bash
# Check correlation service logs
grep "timeout" logs/app.log
```

**Solutions**:
- VEX Core may be overloaded or slow
- Check VEX Core health and performance
- Increase timeout (currently 5 seconds hardcoded)
- Check network latency between gateway and core

### Stranded Order Idempotency Claims

**Symptom**: Repeated order requests with the same idempotency key return `408` with
`status: "outcome_unknown"` and the same `client_order_id`, but open orders and order history never
show the order.

**Cause**: The gateway may have stopped after claiming
`vex:order-idempotency:<user_id>:<key>` and before publishing the order. The in-flight claim is kept
for 24 hours to prevent a retry from publishing a duplicate order.

A claim the gateway gives back because the engine executed nothing (busy, or the publish failed)
is not stranded: a release marker replaces it for 60 seconds, a request already waiting on it gets
a `503` for why it was released (busy, with `Retry-After: 1`, or `Matching engine unavailable`),
and the retry claims the key as usual.

**Remedy**:
1. Reconcile the returned `client_order_id` against open orders, order history, and gateway/core
   logs.
2. Only after confirming that the order was never published, remove the exact stranded claim:
   ```bash
   redis-cli -u "<auth.redis.url>" DEL "vex:order-idempotency:<user_id>:<key>"
   ```
3. The client may submit the order again after the claim is removed. If publication cannot be
   ruled out, do not delete the claim; deleting it could allow a duplicate order.

### Authentication Failures

**Symptom**: 401 Unauthorized responses

**Diagnosis**:
```bash
# Check the JWT keys at auth.jwt.private_key_path and auth.jwt.public_key_path exist
ls -la "<auth.jwt.private_key_path>" "<auth.jwt.public_key_path>"

# Test token generation (if you have test script)
```

**Solutions**:
- Verify JWT keys exist and are readable
- Check `auth.jwt.issuer` and `auth.jwt.audience` match token claims
- Verify token hasn't expired
- Check Redis is accessible (for session validation)

### High Memory Usage

**Symptom**: Service consumes excessive memory

**Diagnosis**:
```bash
# Check memory usage
ps aux | grep gateway
top -p <pid>

# Check for memory leaks in logs
grep -i "memory\|oom" logs/app.log
```

**Solutions**:
- Check connection pool sizes (database, Redis)
- Review correlation service memory (may accumulate if responses are slow)
- Restart service periodically if needed
- Profile with `valgrind` or `heaptrack` if persistent

---

## Emergency Procedures

### Service is Down

1. **Check Service Status**
   ```bash
   ps aux | grep gateway
   systemctl status gateway  
   ```

2. **Check Logs**
   ```bash
   tail -100 logs/app.log
   journalctl -u gateway -n 100 
   ```

3. **Restart Service**
   ```bash
   systemctl restart gateway
   
   # Manual restart
   ./target/release/gateway
   ```

4. **Verify Health**
   ```bash
   curl http://localhost:8081/api/health
   ```

### Database is Down

1. **Check Database Status**
   ```bash
   pg_isready -h <host> -p <port>
   ```

2. **Service Impact**
   - User registration/login may fail
   - Order history queries will fail
   - API key management will fail
   - **Order placement may still work** (if VEX Core is up)

3. **Mitigation**
   - Restore database from backup
   - Switch to backup database if available
   - Service will log errors but may continue operating for some endpoints

### Redis is Down

1. **Check Redis Status**
   ```bash
   redis-cli -u "<auth.redis.url>" ping
   ```

2. **Service Impact**
   - Every JWT-authenticated request gets 503 (`service_unavailable`): the token revocation
     checks read Redis.
   - Signed API-key requests get 503 (`Signature replay protection failed`): their signatures
     are recorded in Redis against replay.
   - **Order placement fails**: every order's idempotency claim is kept in Redis, so nothing is
     sent to the engine. An order that gets past authentication is answered 503 (`Order was not
     sent because idempotency storage is unavailable`).
   - OAuth flows will fail (state storage)
   - Session refresh may fail

3. **Mitigation**
   - Restart Redis
   - Switch to backup Redis if available

### VEX Core is Down

1. **Check VEX Core Status**
   ```bash
   # Check Aeron connection
   ls -la <core.context_dir>
   
   # Check network connectivity
   ping <core.address>
   ```

2. **Service Impact**
   - **All order operations will fail** (place/cancel)
   - `GET /api/health` returns 503 `engine_link: not connected to vex-core` while no core reads
     this gateway's publication: a few seconds after the core stops, and from then on, also once
     the core is back, because a restarted core has lost this gateway's session (see
     Mitigation).
   - It also returns 503 (`engine_link: response from vex-core timed out`) once order requests
     have kept timing out for 10 s with no engine response in between (timeouts more than 30 s
     apart count as separate incidents). With no order traffic this check keeps passing, and
     once red it turns green only on the next routed response, so do not use the endpoint as a
     load-balancer readiness check: an instance taken out of rotation would stay red. Use it as
     the orchestrator's health check, which restarts the gateway.
   - User data endpoints keep working only until the orchestrator restarts the gateway. A gateway
     cannot start without a core (its handshake times out after 20 s and it exits), so from then
     on the whole gateway is down, and it comes back with the orchestrator's first restart after
     the core is up again.

3. **Mitigation**
   - Restore VEX Core service, then restart every gateway: a gateway does not reconnect to a
     restarted core, and cannot publish until it restarts and handshakes again. The orchestrator
     restarts it when its health check uses `/api/health`.
   - Order and cancel requests return 503 `Matching engine unavailable` (no `Retry-After`) while
     no core is connected; a busy core answers 503 `Matching engine is busy; please retry` with
     `Retry-After: 1` instead. Until Aeron notices that the core has gone, which takes a few
     seconds, requests can still time out (408) or get the busy 503
   - Consider rate limiting or disabling order endpoints temporarily

### High Error Rate

1. **Identify Error Pattern**
   ```bash
   # Count errors by type
   grep "ERROR" logs/app.log | awk '{print $5}' | sort | uniq -c | sort -rn
   
   # Check error rate over time
   grep "ERROR" logs/app.log | cut -d' ' -f1-2 | uniq -c
   ```

2. **Common Causes**
   - Database connection pool exhausted
   - Redis connection issues
   - VEX Core timeout/overload
   - Invalid requests (check client behavior)

3. **Mitigation**
   - Increase connection pool sizes
   - Restart affected services
   - Enable rate limiting if needed
   - Check upstream service health

---

## API Reference

### Public Endpoints (No Authentication)

- `GET /api/health` - Health check
- `POST /api/signup` - User registration
- `POST /api/login` - User login
- `POST /api/refresh` - Refresh JWT token
- `GET /api/oauth/providers` - List OAuth providers
- `GET /api/oauth/{provider}` - Initiate OAuth flow
- `GET /api/oauth/{provider}/callback` - OAuth callback

### Protected Endpoints (Requires Authentication)

#### Order Management
- `POST /api/order` - Place order
- `POST /api/cancel` - Cancel order

#### User Data
- `GET /api/balances` - Get user balances
- `GET /api/orders/open` - Get open orders
- `GET /api/orders/history` - Get order history
- `GET /api/portfolio` - Get portfolio overview
- `GET /api/balance-chart` - Get balance chart data
- `GET /api/pnl-chart` - Get P&L chart data
- `GET /api/favorites` - Get favorite markets
- `POST /api/favorites` - Toggle favorite market

#### API Key Management
- `POST /api/api-key` - Create API key
- `GET /api/api-key` - List API keys
- `GET /api/api-key/{id}` - Get API key details
- `PUT /api/api-key/{id}` - Update API key
- `DELETE /api/api-key/{id}` - Delete API key
- `PATCH /api/api-key/{id}/toggle` - Enable/disable API key

#### Session
- `GET /api/session/check` - Check session validity

### Authentication Methods

1. **JWT Bearer Token**
   ```bash
   curl -H "Authorization: Bearer <token>" http://localhost:8081/api/balances
   ```

2. **API Key with HMAC Signature**
   ```bash
   curl -H "X-MBX-APIKEY: <key_id>" \
        'http://localhost:8081/api/balances?timestamp=<unix_time_ms>&signature=<signature>'
   ```

   Every request uses exactly one construction: sign the UTF-8 bytes of
   `ENC(method) LF ENC(path) LF SORTED_QUERY LF HEX(body)`. `ENC` is URL form
   percent-encoding, `SORTED_QUERY` is the parsed query sorted by key and value and
   form-encoded after removing only parameters whose decoded key is exactly `signature`, and
   `HEX(body)` is lowercase hex of the raw body (with an exact `signature` form field removed
   for form bodies). An empty query contributes an empty query component, and an empty body
   contributes an empty body component; the construction does not branch on emptiness. Signed
   requests with a duplicate decoded parameter name in the query or form body are rejected.
   This scheme deliberately binds method and path and is therefore not Binance
   signature-compatible.

### Order Request Format

Every order carries an `Idempotency-Key` header, one key per logical order: not empty, at most 128
bytes, no control characters. Without one the gateway answers 400. A retry with the same key and
body gets the first attempt's answer instead of placing a second order; the same key with a
different body gets 409.

```bash
curl -X POST http://localhost:8081/api/order \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: <unique-logical-order-key>" \
  -d '{"price": 5000000, "volume": 100, "market_id": 1, "side": "Bid", "time_in_force": "Gtc", "order_type": "Limit"}'
```

The body:

```json
{
  "price": 5000000,
  "volume": 100,
  "market_id": 1,
  "side": "Bid",
  "time_in_force": "Gtc",
  "order_type": "Limit"
}
```

### Order Response Format

```json
{
  "order_id": "123456789",
  "client_order_id": "987654321",
  "status": "Processing",
  "message": "Order processed successfully"
}
```

---

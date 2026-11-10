-- Tests of the read model's Postgres schema, run by run.py as a superuser against a migrated and
-- seeded database: psql -v ON_ERROR_STOP=1 -f schema.sql. A failed ASSERT stops the run with
-- its message. Everything happens in one transaction that is rolled back.
BEGIN;

-- Every trading table carries the keep_newer guard, so a table added without it fails here.
DO $$
DECLARE
    tables  int;
    missing text;
BEGIN
    SELECT count(*), string_agg(c.relname, ', ' ORDER BY c.relname) FILTER (
               WHERE NOT EXISTS (
                   SELECT FROM pg_trigger t
                   WHERE t.tgrelid = c.oid
                     AND t.tgfoid = 'trading.keep_newer()'::regprocedure
                     AND t.tgenabled = 'O'
                     -- FOR EACH ROW (1), BEFORE (2), UPDATE (16)
                     AND t.tgtype & 19 = 19))
      INTO tables, missing
      FROM pg_class c
     WHERE c.relnamespace = 'trading'::regnamespace AND c.relkind IN ('r', 'p');
    ASSERT tables > 0, 'the trading schema has no tables';
    ASSERT missing IS NULL, format('trading tables without the keep_newer trigger: %s', missing);
END
$$;

-- The guard, through the statement the JDBC sink sends in upsert mode.
CREATE FUNCTION pg_temp.upsert_balance(available BIGINT, event_ts BIGINT) RETURNS void
LANGUAGE sql AS $$
    INSERT INTO trading.balances (user_id, asset_id, available, locked, total, event_ts)
    VALUES (1, 1, available, 0, available, event_ts)
    ON CONFLICT (user_id, asset_id) DO UPDATE SET
        available = EXCLUDED.available, locked = EXCLUDED.locked, total = EXCLUDED.total,
        event_ts = EXCLUDED.event_ts;
$$;
CREATE FUNCTION pg_temp.upsert_balance_event(available BIGINT, event_ts BIGINT) RETURNS void
LANGUAGE sql AS $$
    INSERT INTO trading.balance_events (user_id, asset_id, available, locked, total, event_ts)
    VALUES (1, 1, available, 0, available, event_ts)
    ON CONFLICT (user_id, asset_id, event_ts) DO UPDATE SET
        available = EXCLUDED.available, locked = EXCLUDED.locked, total = EXCLUDED.total;
$$;

DO $$
DECLARE
    latest trading.balances;
BEGIN
    PERFORM pg_temp.upsert_balance(10, 100);
    PERFORM pg_temp.upsert_balance(20, 200);
    SELECT * INTO latest FROM trading.balances;
    ASSERT (latest.available, latest.event_ts) = (20, 200), 'a newer event replaces the latest row';

    PERFORM pg_temp.upsert_balance(99, 200);
    SELECT * INTO latest FROM trading.balances;
    ASSERT (latest.available, latest.event_ts) = (20, 200), 'the first copy of an event stays';

    PERFORM pg_temp.upsert_balance(15, 150);
    SELECT * INTO latest FROM trading.balances;
    ASSERT (latest.available, latest.event_ts) = (20, 200), 'an older event is skipped';

    PERFORM pg_temp.upsert_balance_event(10, 100);
    PERFORM pg_temp.upsert_balance_event(99, 100);
    PERFORM pg_temp.upsert_balance_event(5, 50);
    ASSERT (SELECT array_agg(available ORDER BY event_ts) FROM trading.balance_events) = '{5,10}',
        'history keeps every event once, whatever the order, and a duplicate changes nothing';
END
$$;

-- trading.u64 reads a BIGINT as the u64 with the same bits.
DO $$
BEGIN
    ASSERT trading.u64(0) = 0;
    ASSERT trading.u64(9223372036854775807) = 9223372036854775807;
    ASSERT trading.u64(-9223372036854775808) = 9223372036854775808;
    ASSERT trading.u64(-8446744073709551616) = 10000000000000000000;
    ASSERT trading.u64(-1) = 18446744073709551615;
END
$$;

-- The gateway's role reads trading.* and is denied every write to it.
SET LOCAL ROLE readmodel_gateway;
DO $$
DECLARE
    t text;
    statement text;
    denied boolean;
BEGIN
    FOR t IN SELECT format('%I.%I', schemaname, tablename) FROM pg_tables WHERE schemaname = 'trading' LOOP
        EXECUTE format('SELECT count(*) FROM %s', t);
        FOREACH statement IN ARRAY ARRAY[
            format('INSERT INTO %s DEFAULT VALUES', t),
            format('UPDATE %s SET event_ts = event_ts', t),
            format('DELETE FROM %s', t),
            format('TRUNCATE %s', t)]
        LOOP
            BEGIN
                EXECUTE statement;
                denied := false;
            EXCEPTION
                WHEN insufficient_privilege THEN denied := true;
                WHEN OTHERS THEN denied := false;  -- past the privilege check, e.g. NOT NULL
            END;
            ASSERT denied, format('readmodel_gateway may run: %s', statement);
        END LOOP;
    END LOOP;
END
$$;

-- What the gateway does to its own tables.
INSERT INTO users (user_id, email, username, auth_provider)
VALUES ('00000000-0000-0000-0000-000000000001', 'role@example.com', 'role', 'email');
UPDATE users SET email_verified = true WHERE username = 'role';
INSERT INTO api_keys (id, api_key, permissions, recv_window, enabled)
VALUES ('00000000-0000-0000-0000-000000000002', 'role-key', '{trade}', 5000, true);
UPDATE api_keys SET enabled = false WHERE api_key = 'role-key';
INSERT INTO user_favorite_markets (user_id, market_id) VALUES (-1, 65538);
DELETE FROM user_favorite_markets WHERE user_id = -1;
DO $$
BEGIN
    PERFORM FROM assets JOIN markets ON markets.base_asset = assets.asset_id;
    ASSERT NOT has_table_privilege('assets', 'INSERT, UPDATE, DELETE'), 'assets are seeded only';
    ASSERT NOT has_table_privilege('markets', 'INSERT, UPDATE, DELETE'), 'markets are seeded only';
    -- Only the owner creates objects, also in schema public.
    ASSERT NOT has_schema_privilege('public', 'CREATE'), 'readmodel_gateway may create in schema public';
END
$$;
RESET ROLE;

-- Kafka Connect's role may upsert trading.* and nothing more.
SET LOCAL ROLE readmodel_connect;
DO $$
DECLARE
    t text;
BEGIN
    FOR t IN SELECT format('%I.%I', schemaname, tablename) FROM pg_tables WHERE schemaname = 'trading' LOOP
        ASSERT has_table_privilege(t, 'SELECT') AND has_table_privilege(t, 'INSERT')
            AND has_table_privilege(t, 'UPDATE'), format('readmodel_connect cannot upsert %s', t);
        ASSERT NOT has_table_privilege(t, 'DELETE, TRUNCATE'), format('readmodel_connect may delete from %s', t);
    END LOOP;
    ASSERT NOT has_table_privilege('users', 'SELECT'), 'readmodel_connect may read users';
    ASSERT NOT has_schema_privilege('public', 'CREATE'), 'readmodel_connect may create in schema public';
END
$$;
RESET ROLE;

ROLLBACK;

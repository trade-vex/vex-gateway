# Timestamp Generation Locations for Trade Events

This document tracks where timestamps are generated for storing trade events in the database.

## Current Status: Using Nanoseconds

All timestamps are now stored as **nanoseconds since Unix epoch** (i64 in database, u64 in messages).

## Timestamp Generation Locations

### 1. **vex-gateway** (This Repository) ✅ UPDATED

#### OrderCommand Timestamp Generation:
- **File**: `crates/libs/lib-api/src/handlers/order.rs`
  - **Line 247-250**: `place_order()` handler - OrderCommand timestamp
  - **Line 416-419**: `cancel_order()` handler - OrderCommand timestamp
  - **Format**: `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64`

#### Test Files (Updated):
- `crates/tests/tests/user_integration_test.rs` (lines 90-93, 191-194)
- `crates/tests/tests/stress_test.rs` (lines 93-96, 158-161, 218-221)
- `crates/tests/tests/order_integration_test.rs` (lines 67-70, 156-159)
- `crates/tests/tests/multi_user_trading_test.rs` (lines 98-101, 163-166, 223-226)

#### Client Order ID Generation:
- **File**: `crates/libs/lib-core/src/correlation.rs`
  - **Line 17-20**: Uses nanoseconds for client_order_id generation
  - **Status**: Already using `.as_nanos()`

### 2. **vex-core** (External Repository)

**Location**: `https://github.com/trade-vex/vex-core` (rev: 56ee430)

**Where to check**:
- `OrderCommand` struct definition - timestamp field type
- `TradeEvent` struct definition - timestamp field type
- Matching engine code that generates timestamps when trades are executed
- Kafka producer code that publishes TradeEvent messages

**Expected locations**:
- Trade execution/matching logic that creates TradeEvent
- Order processing that sets OrderCommand.timestamp
- Any timestamp normalization or conversion code

### 3. **Consumer** (External Service)

**Location**: `/Users/nikhilsharma/code/testing-setup/trading-events-consumer.py` (out of workspace)

**Key functions**:
- `normalize_timestamp_to_nanoseconds()` - Already normalizes to nanoseconds
- `process_trade()` - Processes trade events with timestamp_ns
- `insert_trade_events_batch()` - Inserts into database

**Status**: Consumer already expects and normalizes timestamps to nanoseconds.

### 4. **vex-market-datagateway** (External Service)

**Location**: Not found in current workspace. Check:
- Separate repository or service
- Market data aggregation service
- Any service that publishes market data events

## Database Storage

**Table**: `trading.trades_events`
- **Column**: `timestamp` (type: `i64`)
- **Format**: Nanoseconds since Unix epoch (1970-01-01 00:00:00 UTC)

**Example**:
- Current time in nanoseconds: `1728000000000000000` (20 days after epoch)
- This represents: `1728000` seconds = January 21, 1970

## Migration Notes

When updating other services:

1. **vex-core**: Update timestamp generation in:
   - TradeEvent creation (when trades are executed)
   - OrderCommand timestamp (if generated there)
   - Any timestamp conversion utilities

2. **vex-market-datagateway**: Update if it generates timestamps for:
   - Market data events
   - Price updates
   - Trade events

3. **Consumer**: Already handles nanoseconds, but verify:
   - All timestamp normalization functions
   - Database insertion code
   - Any timestamp conversion logic

## Verification

To verify timestamps are in nanoseconds:
- Check value magnitude: nanoseconds are ~19 digits for current dates
- Microseconds would be ~16 digits
- Milliseconds would be ~13 digits
- Seconds would be ~10 digits

Example for 2024-01-01:
- Nanoseconds: `1704067200000000000` (19 digits)
- Microseconds: `1704067200000000` (16 digits)
- Milliseconds: `1704067200000` (13 digits)
- Seconds: `1704067200` (10 digits)


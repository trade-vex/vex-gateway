## API Reference

### SqlDb

Core operations that all SQL databases must support:

- `connect(url)` - Establish connection with pooling
- `execute_with(sql, args)` - Execute parameterized query
- `query_with(sql, args)` - Query multiple rows
- `query_one_with(sql, args)` - Query single row
- `query_optional_with(sql, args)` - Query optional single row
- `native_pool()` - Get native pool for escape hatch access
- `pool_stats()` - Get connection pool statistics

### BasicKvStore

Core operations that all KV stores must support:

- `connect(url)` - Establish connection
- `set(key, value)` - Store value with serialization
- `get(key)` - Retrieve and deserialize value
- `delete(key)` - Remove key
- `exists(key)` - Check if key exists
- `set_bytes(key, value)` - Store raw bytes
- `get_bytes(key)` - Retrieve raw bytes

### AdvancedKvStore

Advanced features for performance and monitoring:

- `ping()` - Health check
- `pool_stats()` - Connection pool statistics
- `mget(keys)` - Batch retrieve
- `mset(pairs)` - Batch store
- `mdel(keys)` - Batch delete

### AtomicKvStore

Atomic operations for concurrent access:

- `incr(key, delta)` - Atomic increment/decrement
- `set_nx(key, value)` - Set if not exists
- `cas(key, old, new)` - Compare and swap

### ExpiringKvStore

Time-based expiration:

- `set_ex(key, value, ttl)` - Set with expiration
- `expire(key, ttl)` - Set expiration on existing key
- `ttl(key)` - Get remaining time to live

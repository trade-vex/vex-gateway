//! The gateway's one rate-limit primitive: a fixed window counted in Redis. The auth endpoints'
//! own limits (`EmailService::enforce_rate_limit`) and the API's budgets (lib-server's rate-limit
//! middleware) both count with it.

use redis::{aio::ConnectionManager, RedisResult, Script};
use serde::Deserialize;
use std::sync::LazyLock;

/// At most `max_requests` per window of `window_seconds`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct Limit {
    pub max_requests: u64,
    pub window_seconds: u64,
}

/// Counts a request in one round trip and returns the count and the seconds left in the window.
/// The window starts with its first request; a refused request is counted too, but never moves
/// the window's end.
static FIXED_WINDOW: LazyLock<Script> = LazyLock::new(|| {
    Script::new(
        r#"
local count = redis.call('INCR', KEYS[1])
if count == 1 then
    redis.call('EXPIRE', KEYS[1], ARGV[1])
end
local ttl = redis.call('TTL', KEYS[1])
return {count, ttl}
"#,
    )
});

/// Counts one request against the window at `key`: `None` within `limit`, and over it the seconds
/// left in the window (the client's `Retry-After`). A Redis error is returned, never taken for
/// "within the limit", so that the caller fails closed.
pub async fn hit(redis: &ConnectionManager, key: &str, limit: Limit) -> RedisResult<Option<u64>> {
    let (count, ttl): (u64, i64) = FIXED_WINDOW
        .key(key)
        .arg(limit.window_seconds)
        .invoke_async(&mut redis.clone())
        .await?;

    let retry_after = if ttl > 0 {
        ttl as u64
    } else {
        limit.window_seconds
    };
    Ok((count > limit.max_requests).then_some(retry_after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::AsyncCommands;
    use uuid::Uuid;

    /// The test Redis (CI sets `REDIS_URL`).
    async fn redis() -> ConnectionManager {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        ConnectionManager::new(redis::Client::open(redis_url).unwrap())
            .await
            .expect("rate-limit tests need Redis")
    }

    fn key() -> String {
        format!("rate_limit:test:{}", Uuid::new_v4())
    }

    #[tokio::test]
    async fn requests_over_the_limit_get_the_rest_of_the_window() {
        let redis = redis().await;
        let key = key();
        let limit = Limit {
            max_requests: 3,
            window_seconds: 60,
        };

        for _ in 0..3 {
            assert_eq!(hit(&redis, &key, limit).await.unwrap(), None);
        }
        for _ in 0..2 {
            let retry_after = hit(&redis, &key, limit).await.unwrap();
            assert!(
                matches!(retry_after, Some(1..=60)),
                "retry after {retry_after:?}"
            );
        }

        // Refused requests are counted, and the window still ends when its first request set.
        let mut connection = redis.clone();
        let count: u64 = connection.get(&key).await.unwrap();
        assert_eq!(count, 5);
        let ttl: i64 = connection.ttl(&key).await.unwrap();
        assert!((1..=60).contains(&ttl), "ttl {ttl}");
    }

    #[tokio::test]
    async fn each_key_has_its_own_window() {
        let redis = redis().await;
        let limit = Limit {
            max_requests: 1,
            window_seconds: 60,
        };
        let (first, second) = (key(), key());

        assert_eq!(hit(&redis, &first, limit).await.unwrap(), None);
        assert!(hit(&redis, &first, limit).await.unwrap().is_some());
        assert_eq!(hit(&redis, &second, limit).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_redis_error_is_returned() {
        let redis = redis().await;
        let key = key();
        // INCR fails on a value that is not an integer.
        let _: () = redis.clone().set_ex(&key, "not-a-count", 60).await.unwrap();

        let limit = Limit {
            max_requests: 1,
            window_seconds: 60,
        };
        assert!(hit(&redis, &key, limit).await.is_err());
    }
}

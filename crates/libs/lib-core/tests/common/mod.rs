/// The tests' Redis: `REDIS_URL`, or the default local port when it is unset. CI sets `REDIS_URL`
/// to its Redis service.
pub fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

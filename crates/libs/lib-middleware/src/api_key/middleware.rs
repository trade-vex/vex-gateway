use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    error::PayloadError,
    web, Error, HttpMessage, HttpRequest,
};
use futures_util::future::{ready, LocalBoxFuture, Ready};
use futures_util::stream;
use std::hash::{BuildHasher, RandomState};
use std::sync::{RwLock, RwLockWriteGuard};
use std::{
    collections::HashMap,
    pin::Pin,
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::{debug, error, info, warn};

use crate::api_key::{
    error::ApiKeyError,
    security::{ApiKeyConfig, KeyMaterial, SecurityType},
    signature::SignatureVerifier,
};
use crate::repository::api_key_repository::ApiKeyRepository;

/// Route-specific security configuration
/// Placeholder : Will be removed and be replaced by direct actix based routing
#[derive(Debug, Clone)]
pub struct RouteSecurityConfig {
    pub path_pattern: String,
    pub security_type: SecurityType,
}

/// API Key Store with multi-level caching
///
/// Cache Hierarchy:
/// 1. L1 (In-Memory LRU)
/// 2. L2 (Redis) - TODO
/// 3. L3 (Database)
#[derive(Clone)]
pub struct ApiKeyStore {
    /// L1 Cache: In-memory LRU for hot keys, sharded by key
    l1_cache: Arc<L1Cache>,
    /// Database repository for persistent storage
    api_key_repo: Option<Arc<dyn ApiKeyRepository>>,
    /// Fallback: In-memory HashMap for development
    memory_store: Arc<HashMap<String, ApiKeyConfig>>,
    /// Cache configuration
    config: CacheConfig,
    /// Performance metrics
    metrics: Arc<CacheMetrics>,
}

/// Cached API key with TTL
#[derive(Clone)]
struct CachedApiKey {
    config: Arc<ApiKeyConfig>,
    cached_at: SystemTime,
}

/// How many independently locked shards the L1 cache has. A lookup updates its entry's LRU
/// recency, so it takes a write lock: with one lock, every API-key request would queue on it.
const L1_SHARDS: usize = 16;

/// The L1 cache, split into [`L1_SHARDS`] shards chosen by a hash of the API key. Everything kept
/// per key (its entry, its invalidation epoch and in-flight fills) lives in that key's shard, so a
/// fill and an invalidation of one key still meet under one lock.
struct L1Cache {
    shards: [RwLock<L1CacheState>; L1_SHARDS],
    hasher: RandomState,
}

impl L1Cache {
    /// `max_size` is spread over the shards, rounded up.
    fn new(max_size: usize) -> Self {
        let shard_size = max_size.div_ceil(L1_SHARDS);
        Self {
            shards: std::array::from_fn(|_| RwLock::new(L1CacheState::new(shard_size))),
            hasher: RandomState::new(),
        }
    }

    fn shard_index(&self, api_key: &str) -> usize {
        (self.hasher.hash_one(api_key) % L1_SHARDS as u64) as usize
    }

    /// The write lock of `api_key`'s shard.
    fn write(&self, api_key: &str) -> RwLockWriteGuard<'_, L1CacheState> {
        self.shards[self.shard_index(api_key)]
            .write()
            .unwrap_or_else(|e| e.into_inner())
    }
}

struct L1CacheState {
    entries: lru::LruCache<String, CachedApiKey>,
    invalidation_epochs: HashMap<String, InvalidationEpoch>,
}

#[derive(Default)]
struct InvalidationEpoch {
    value: u64,
    in_flight: usize,
}

impl L1CacheState {
    fn new(max_size: usize) -> Self {
        Self {
            entries: lru::LruCache::new(
                std::num::NonZeroUsize::new(max_size).expect("L1 cache size must be > 0"),
            ),
            invalidation_epochs: HashMap::new(),
        }
    }

    fn begin_fill(&mut self, api_key: &str) -> u64 {
        let epoch = self
            .invalidation_epochs
            .entry(api_key.to_string())
            .or_default();
        epoch.in_flight += 1;
        epoch.value
    }

    fn finish_fill(&mut self, api_key: &str) {
        if let Some(epoch) = self.invalidation_epochs.get_mut(api_key) {
            epoch.in_flight -= 1;
        }
        self.prune_epoch_if_idle(api_key);
    }

    fn bump_epoch(&mut self, api_key: &str) {
        let epoch = self
            .invalidation_epochs
            .entry(api_key.to_string())
            .or_default();
        epoch.value = epoch.value.wrapping_add(1);
    }

    fn put(&mut self, api_key: String, cached: CachedApiKey) {
        if let Some((evicted_key, _)) = self.entries.push(api_key, cached) {
            self.prune_epoch_if_idle(&evicted_key);
        }
    }

    fn prune_epoch_if_idle(&mut self, api_key: &str) {
        if self
            .invalidation_epochs
            .get(api_key)
            .is_some_and(|epoch| epoch.in_flight == 0)
        {
            self.invalidation_epochs.remove(api_key);
        }
    }
}

impl CachedApiKey {
    fn new(config: ApiKeyConfig) -> Self {
        Self {
            config: Arc::new(config),
            cached_at: SystemTime::now(),
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.cached_at.elapsed().unwrap_or(Duration::MAX) > ttl
    }
}

/// Cache configuration
#[derive(Clone)]
pub struct CacheConfig {
    pub l1_max_size: usize,
    pub l1_ttl: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            l1_max_size: 100_000,             // 100K hot keys in memory
            l1_ttl: Duration::from_secs(300), // 5 minute TTL
        }
    }
}

/// Cache performance metrics
pub struct CacheMetrics {
    pub l1_hits: std::sync::atomic::AtomicU64,
    pub l1_misses: std::sync::atomic::AtomicU64,
    pub db_queries: std::sync::atomic::AtomicU64,
}

impl Default for CacheMetrics {
    fn default() -> Self {
        Self {
            l1_hits: std::sync::atomic::AtomicU64::new(0),
            l1_misses: std::sync::atomic::AtomicU64::new(0),
            db_queries: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl CacheMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get L1 cache hit rate for monitoring
    pub fn get_l1_hit_rate(&self) -> f64 {
        let hits = self.l1_hits.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.l1_misses.load(std::sync::atomic::Ordering::Relaxed);
        if hits + misses == 0 {
            0.0
        } else {
            hits as f64 / (hits + misses) as f64
        }
    }
}

/// Ends an in-flight cache fill exactly once, on completion or on drop.
///
/// `Drop` runs even when the owning future is cancelled (an actix client disconnect), which is the
/// path that previously leaked `in_flight` and pinned the per-key epoch entry forever (G2-2).
struct CacheFillGuard {
    cache: Arc<L1Cache>,
    api_key: String,
    epoch: u64,
    finished: bool,
}

impl CacheFillGuard {
    /// Completes the fill, writing `config` into L1 only if no invalidation raced us.
    fn finish(mut self, config: Option<ApiKeyConfig>) {
        let mut cache = self.cache.write(&self.api_key);
        let epoch_unchanged = cache
            .invalidation_epochs
            .get(&self.api_key)
            .is_some_and(|epoch| epoch.value == self.epoch);

        if epoch_unchanged {
            if let Some(config) = config {
                cache.put(self.api_key.clone(), CachedApiKey::new(config));
            }
        }
        cache.finish_fill(&self.api_key);
        self.finished = true;
    }
}

impl Drop for CacheFillGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // Cancelled mid-fill: release the bookkeeping so the epoch entry can be pruned. Never
        // write to the cache here - we do not know whether the lookup completed.
        let mut cache = self.cache.write(&self.api_key);
        cache.finish_fill(&self.api_key);
    }
}

impl ApiKeyStore {
    /// Create new store
    pub fn new(config: CacheConfig) -> Self {
        let l1_cache = Arc::new(L1Cache::new(config.l1_max_size));

        Self {
            l1_cache,
            api_key_repo: None,
            memory_store: Arc::new(HashMap::new()),
            config,
            metrics: Arc::new(CacheMetrics::new()),
        }
    }

    /// Create store with database repository
    pub fn with_repository(config: CacheConfig, api_key_repo: Arc<dyn ApiKeyRepository>) -> Self {
        let l1_cache = Arc::new(L1Cache::new(config.l1_max_size));

        Self {
            l1_cache,
            api_key_repo: Some(api_key_repo),
            memory_store: Arc::new(HashMap::new()),
            config,
            metrics: Arc::new(CacheMetrics::new()),
        }
    }

    /// Add API keys for development
    pub fn with_api_keys(mut self, configs: Vec<ApiKeyConfig>) -> Self {
        let mut api_keys = HashMap::new();
        for config in configs {
            api_keys.insert(config.api_key.clone(), config);
        }
        self.memory_store = Arc::new(api_keys);
        self
    }

    /// Marks a cache fill in flight and returns a guard that ALWAYS ends it.
    ///
    /// The bookkeeping must be released even if the caller's future is dropped mid-flight, which
    /// actix does routinely when a client disconnects. A bare begin/finish pair leaked `in_flight`
    /// on that path, and since pruning requires `in_flight == 0`, the per-key epoch entry could
    /// then never be removed - growing without bound past the LRU capacity (finding G2-2).
    fn begin_cache_fill(&self, api_key: &str) -> CacheFillGuard {
        let epoch = self.l1_cache.write(api_key).begin_fill(api_key);
        CacheFillGuard {
            cache: Arc::clone(&self.l1_cache),
            api_key: api_key.to_string(),
            epoch,
            finished: false,
        }
    }

    fn finish_cache_fill(&self, guard: CacheFillGuard, config: Option<ApiKeyConfig>) {
        guard.finish(config);
    }

    /// Fast API key lookup with caching
    pub async fn get_api_key(&self, api_key: &str) -> Option<Arc<ApiKeyConfig>> {
        // L1 Cache: Check in-memory LRU first
        {
            let mut cache = self.l1_cache.write(api_key);
            if let Some(cached) = cache.entries.get(api_key) {
                if !cached.is_expired(self.config.l1_ttl) {
                    self.metrics
                        .l1_hits
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    debug!(
                        "L1 cache hit for key: {}",
                        ApiKeyStore::mask_api_key(api_key)
                    );
                    return Some(cached.config.clone());
                } else {
                    // Remove expired entry
                    cache.entries.pop(api_key);
                    cache.prune_epoch_if_idle(api_key);
                }
            }
        }

        self.metrics
            .l1_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // L3: Check database first (if available)
        if let Some(repo) = &self.api_key_repo {
            let fill_guard = self.begin_cache_fill(api_key);
            match repo.find_by_api_key(api_key).await {
                Ok(Some(model)) => {
                    if model.is_valid() {
                        self.metrics
                            .db_queries
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let config = model.to_api_key_config();
                        let config_arc = Arc::new(config.clone());

                        self.finish_cache_fill(fill_guard, Some(config));

                        debug!(
                            "Database hit for key: {}",
                            ApiKeyStore::mask_api_key(api_key)
                        );
                        return Some(config_arc);
                    } else {
                        self.finish_cache_fill(fill_guard, None);
                        debug!(
                            "API key found but invalid (disabled/expired): {}",
                            ApiKeyStore::mask_api_key(api_key)
                        );
                        return None;
                    }
                }
                Ok(None) => {
                    self.finish_cache_fill(fill_guard, None);
                    debug!(
                        "API key not found in database: {}",
                        ApiKeyStore::mask_api_key(api_key)
                    );
                }
                Err(e) => {
                    self.finish_cache_fill(fill_guard, None);
                    error!(
                        "Database error while fetching API key {}: {}",
                        ApiKeyStore::mask_api_key(api_key),
                        e
                    );
                }
            }
        }

        // Fallback: Check memory store (for development/testing)
        let fill_guard = self.begin_cache_fill(api_key);
        if let Some(config) = self.memory_store.get(api_key) {
            self.metrics
                .db_queries
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let config_arc = Arc::new(config.clone());

            self.finish_cache_fill(fill_guard, Some(config.clone()));

            debug!(
                "Memory store hit for key: {}",
                ApiKeyStore::mask_api_key(api_key)
            );
            return Some(config_arc);
        }
        self.finish_cache_fill(fill_guard, None);

        // TODO: Add Redis (L2) lookup between L1 and L3
        // if let Some(config) = self.check_redis(api_key).await {
        //     self.update_l1_cache(api_key, config.clone()).await;
        //     return Some(config);
        // }

        warn!("API key not found: {}", ApiKeyStore::mask_api_key(api_key));
        None
    }

    /// Invalidate cache entry (when key is updated/disabled)
    pub async fn invalidate(&self, api_key: &str) {
        let mut cache = self.l1_cache.write(api_key);
        cache.bump_epoch(api_key);
        cache.entries.pop(api_key);
        cache.prune_epoch_if_idle(api_key);
        // TODO: Invalidate Redis cache when implemented
        info!(
            "Invalidated cache for key: {}",
            ApiKeyStore::mask_api_key(api_key)
        );
    }

    /// Get cache metrics for monitoring
    pub fn metrics(&self) -> &CacheMetrics {
        &self.metrics
    }

    /// Add a new API key to the cache
    pub async fn add_api_key(&self, config: ApiKeyConfig) {
        let mut cache = self.l1_cache.write(&config.api_key);
        let cached = CachedApiKey::new(config.clone());
        cache.put(config.api_key.clone(), cached);

        info!(
            "Added API key to cache: {}",
            Self::mask_api_key(&config.api_key)
        );
    }

    /// Remove an API key from the cache
    pub async fn remove_api_key(&self, api_key: &str) {
        let mut cache = self.l1_cache.write(api_key);
        cache.entries.pop(api_key);
        cache.prune_epoch_if_idle(api_key);

        info!(
            "Removed API key from cache: {}",
            Self::mask_api_key(api_key)
        );
    }

    /// Mask API key for secure logging
    fn mask_api_key(api_key: &str) -> String {
        if api_key.len() <= 4 {
            // For very short keys, just show asterisks
            "*".repeat(api_key.len())
        } else {
            // Show last 4 characters with asterisks
            format!(
                "{}...{}",
                "*".repeat(api_key.len().saturating_sub(4)),
                &api_key[api_key.len() - 4..]
            )
        }
    }
}

#[derive(Clone)]
pub struct ApiKeyMiddleware {
    api_key_store: ApiKeyStore,
    default_security: SecurityType,
    route_configs: Arc<Vec<RouteSecurityConfig>>,
    strict_timestamp_validation: bool,
}

impl Default for ApiKeyMiddleware {
    fn default() -> Self {
        Self {
            api_key_store: ApiKeyStore::new(CacheConfig::default()),
            default_security: SecurityType::None,
            route_configs: Arc::new(vec![]),
            strict_timestamp_validation: true,
        }
    }
}

impl ApiKeyMiddleware {
    /// Create new middleware
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure with custom cache settings
    pub fn with_cache_config(mut self, config: CacheConfig) -> Self {
        self.api_key_store = ApiKeyStore::new(config);
        self
    }

    /// Configure with database repository for dynamic loading
    pub fn with_repository(mut self, api_key_repo: Arc<dyn ApiKeyRepository>) -> Self {
        self.api_key_store = ApiKeyStore::with_repository(CacheConfig::default(), api_key_repo);
        self
    }

    /// Add single API key (for development)
    pub fn with_api_key(mut self, config: ApiKeyConfig) -> Self {
        self.api_key_store = self.api_key_store.with_api_keys(vec![config]);
        self
    }

    /// Add multiple API keys (for development/testing)
    pub fn with_api_keys(mut self, configs: Vec<ApiKeyConfig>) -> Self {
        self.api_key_store = self.api_key_store.with_api_keys(configs);
        self
    }

    pub fn with_default_security(mut self, security: SecurityType) -> Self {
        self.default_security = security;
        self
    }

    pub fn with_route_configs(mut self, route_configs: Vec<RouteSecurityConfig>) -> Self {
        self.route_configs = Arc::new(route_configs);
        self
    }

    pub fn with_strict_timestamp_validation(mut self, strict: bool) -> Self {
        self.strict_timestamp_validation = strict;
        self
    }

    /// Get cache metrics for monitoring
    pub fn cache_metrics(&self) -> &CacheMetrics {
        self.api_key_store.metrics()
    }

    /// Get a handle to the shared API key store.
    pub fn api_key_store(&self) -> ApiKeyStore {
        self.api_key_store.clone()
    }

    /// Add a new API key to the cache
    pub async fn add_api_key(&self, config: ApiKeyConfig) {
        self.api_key_store.add_api_key(config).await;
    }

    /// Remove an API key from the cache
    pub async fn remove_api_key(&self, api_key: &str) {
        self.api_key_store.remove_api_key(api_key).await;
    }

    /// Get API key configuration (for use in combined middleware)
    pub async fn get_api_key_config(&self, api_key: &str) -> Option<Arc<ApiKeyConfig>> {
        self.api_key_store.get_api_key(api_key).await
    }

    /// Determine security type for the current route
    fn get_route_security_type(
        &self,
        path: &str,
        route_configs: &[RouteSecurityConfig],
    ) -> SecurityType {
        for config in route_configs {
            if path.starts_with(&config.path_pattern) {
                return config.security_type;
            }
        }
        self.default_security
    }

    /// Extract API key from request headers
    fn extract_api_key(req: &HttpRequest) -> Result<String, ApiKeyError> {
        req.headers()
            .get("X-MBX-APIKEY")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string())
            .ok_or(ApiKeyError::MissingApiKey)
    }

    /// Extract client IP address
    fn extract_client_ip(req: &HttpRequest) -> String {
        // Check X-Forwarded-For header first (for proxies)
        if let Some(forwarded) = req.headers().get("X-Forwarded-For") {
            if let Ok(forwarded_str) = forwarded.to_str() {
                if let Some(first_ip) = forwarded_str.split(',').next() {
                    return first_ip.trim().to_string();
                }
            }
        }

        // Check X-Real-IP header (nginx)
        if let Some(real_ip) = req.headers().get("X-Real-IP") {
            if let Ok(real_ip_str) = real_ip.to_str() {
                return real_ip_str.to_string();
            }
        }

        // Fall back to connection info
        req.connection_info()
            .peer_addr()
            .unwrap_or("unknown")
            .to_string()
    }

    /// Validate signature based on the API key's algorithm
    fn validate_signature(
        api_config: &ApiKeyConfig,
        query_params: &HashMap<String, String>,
        body_params: &HashMap<String, String>,
        raw_query: &str,
        raw_body: &str,
    ) -> Result<(), ApiKeyError> {
        let signature = query_params
            .get("signature")
            .or_else(|| body_params.get("signature"))
            .ok_or(ApiKeyError::MissingSignature)?;

        let key_material = api_config.get_key_material().ok_or_else(|| {
            ApiKeyError::InvalidApiKey("No key material configured for signed endpoint".to_string())
        })?;

        // Build payload based on whether we have mixed query/body or separate parameters
        let payload = if !raw_query.is_empty() && !raw_body.is_empty() {
            // Mixed case as per Binance example 3
            SignatureVerifier::build_mixed_payload(raw_query, raw_body)
        } else if !raw_body.is_empty() {
            // JSON-only signed requests - use raw body as payload
            SignatureVerifier::build_mixed_payload("", raw_body)
        } else {
            // Standard case - fall back to parsed parameters
            SignatureVerifier::build_payload(query_params, body_params)
        };

        // Verify signature based on algorithm
        let is_valid = match key_material {
            KeyMaterial::HmacSecret(secret) => {
                SignatureVerifier::verify_hmac(&payload, signature, secret)?
            }
            KeyMaterial::RsaPublicKey(public_key) => {
                SignatureVerifier::verify_rsa(&payload, signature, public_key)?
            }
            KeyMaterial::Ed25519PublicKey(public_key) => {
                SignatureVerifier::verify_ed25519(&payload, signature, public_key)?
            }
        };

        if !is_valid {
            return Err(ApiKeyError::InvalidSignature(format!(
                "Signature verification failed for algorithm: {:?}",
                key_material.algorithm()
            )));
        }

        Ok(())
    }

    /// Parse query parameters from URL
    fn parse_query_params(req: &HttpRequest) -> HashMap<String, String> {
        req.uri()
            .query()
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get raw query string
    fn get_raw_query_string(req: &HttpRequest) -> String {
        req.uri().query().unwrap_or("").to_string()
    }

    /// Mask API key for secure logging
    fn mask_api_key(api_key: &str) -> String {
        if api_key.len() <= 4 {
            // For very short keys, just show asterisks
            "*".repeat(api_key.len())
        } else {
            // Show last 4 characters with asterisks
            format!(
                "{}...{}",
                "*".repeat(api_key.len().saturating_sub(4)),
                &api_key[api_key.len() - 4..]
            )
        }
    }

    /// Parse body parameters from different content types
    async fn parse_body_params(
        req: &mut ServiceRequest,
    ) -> Result<(HashMap<String, String>, String), ApiKeyError> {
        let content_type = req
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("application/x-www-form-urlencoded")
            .to_string();

        // Read the body
        let mut body = web::BytesMut::new();
        let mut payload = req.take_payload();

        use futures_util::StreamExt;
        while let Some(chunk) = payload.next().await {
            let chunk =
                chunk.map_err(|e| ApiKeyError::Internal(format!("Failed to read body: {}", e)))?;
            body.extend_from_slice(&chunk);
        }

        let body_bytes = body.freeze();
        let body_string = String::from_utf8(body_bytes.to_vec())
            .map_err(|e| ApiKeyError::Internal(format!("Invalid UTF-8 in body: {}", e)))?;

        // Restore the payload for downstream handlers
        let new_payload = {
            let stream = stream::once(async move { Ok::<_, PayloadError>(body_bytes) });
            let boxed_stream: Pin<
                Box<dyn futures_util::Stream<Item = Result<actix_web::web::Bytes, PayloadError>>>,
            > = Box::pin(stream);
            actix_web::dev::Payload::from(boxed_stream)
        };
        req.set_payload(new_payload);

        let body_params = if content_type.starts_with("application/x-www-form-urlencoded") {
            url::form_urlencoded::parse(body_string.as_bytes())
                .into_owned()
                .collect()
        } else if content_type.starts_with("application/json") {
            // For JSON, we don't parse into form parameters for signature
            // The raw JSON string is used for signature verification
            HashMap::new()
        } else {
            HashMap::new()
        };

        Ok((body_params, body_string))
    }

    /// Validate timestamp with proper Binance logic
    fn validate_timestamp_with_recv_window(
        query_params: &HashMap<String, String>,
        body_params: &HashMap<String, String>,
        recv_window: u64,
        strict: bool,
    ) -> Result<(), ApiKeyError> {
        let timestamp_str = query_params
            .get("timestamp")
            .or_else(|| body_params.get("timestamp"))
            .ok_or(ApiKeyError::MissingTimestamp)?;

        let request_time = SignatureVerifier::parse_timestamp(timestamp_str)?;

        let server_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Validate using the exact Binance timing logic
        SignatureVerifier::validate_timestamp(request_time, recv_window, server_time)?;

        // Additional strict validation if enabled
        if strict {
            // Ensure recvWindow parameter is present and valid
            if let Some(recv_window_str) = query_params
                .get("recvWindow")
                .or_else(|| body_params.get("recvWindow"))
            {
                let provided_recv_window = recv_window_str.parse::<u64>().map_err(|_| {
                    ApiKeyError::InvalidTimestamp("Invalid recvWindow format".to_string())
                })?;

                if provided_recv_window > 60000 {
                    return Err(ApiKeyError::InvalidTimestamp(
                        "recvWindow cannot exceed 60000ms".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }
}

impl<S, B> Transform<S, ServiceRequest> for ApiKeyMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = ApiKeyMiddlewareService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(ApiKeyMiddlewareService {
            service: Rc::new(service),
            api_key_store: self.api_key_store.clone(),
            default_security: self.default_security,
            route_configs: self.route_configs.clone(),
            strict_timestamp_validation: self.strict_timestamp_validation,
        }))
    }
}

pub struct ApiKeyMiddlewareService<S> {
    service: Rc<S>,
    api_key_store: ApiKeyStore,
    default_security: SecurityType,
    route_configs: Arc<Vec<RouteSecurityConfig>>,
    strict_timestamp_validation: bool,
}

impl<S, B> Service<ServiceRequest> for ApiKeyMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, mut req: ServiceRequest) -> Self::Future {
        let api_key_store = self.api_key_store.clone();
        let default_security = self.default_security;
        let route_configs = self.route_configs.clone();
        let strict_timestamp_validation = self.strict_timestamp_validation;
        let service = Rc::clone(&self.service);

        Box::pin(async move {
            // Determine required security level for this route
            let path = req.uri().path();
            let middleware = ApiKeyMiddleware {
                api_key_store: api_key_store.clone(),
                default_security,
                route_configs: route_configs.clone(),
                strict_timestamp_validation,
            };
            let required_security = middleware.get_route_security_type(path, &route_configs);

            // For NONE security type, skip validation
            if required_security == SecurityType::None {
                return service.call(req).await;
            }

            // Extract API key
            let api_key = match ApiKeyMiddleware::extract_api_key(req.request()) {
                Ok(key) => key,
                Err(e) => {
                    error!("API key extraction failed: {}", e);
                    return Err(actix_web::error::ErrorUnauthorized(format!(
                        "API key required: {}",
                        e
                    )));
                }
            };

            // Get API key configuration
            let api_config = match api_key_store.get_api_key(&api_key).await {
                Some(config) => config,
                None => {
                    error!(
                        "API key not found: {}",
                        ApiKeyMiddleware::mask_api_key(&api_key)
                    );
                    return Err(actix_web::error::ErrorUnauthorized("Invalid API key"));
                }
            };

            // Validate API key state
            if !api_config.is_valid() {
                warn!(
                    "API key is disabled or expired: {}",
                    ApiKeyMiddleware::mask_api_key(&api_key)
                );
                return Err(actix_web::error::ErrorUnauthorized(
                    "API key is disabled or expired",
                ));
            }

            // Validate IP blacklist
            let client_ip = ApiKeyMiddleware::extract_client_ip(req.request());
            if !api_config.is_ip_allowed(&client_ip) {
                warn!(
                    "IP {} is blacklisted for API key: {}",
                    client_ip,
                    ApiKeyMiddleware::mask_api_key(&api_key)
                );
                return Err(actix_web::error::ErrorForbidden(
                    "IP address is blacklisted",
                ));
            }

            // Check permissions
            if !api_config.has_permission(required_security) {
                error!(
                    "Insufficient permissions. Required: {}, Available: {:?}",
                    required_security, api_config.permissions
                );
                return Err(actix_web::error::ErrorForbidden("Insufficient permissions"));
            }

            // For signed endpoints, validate signature and timestamp
            if api_config.requires_signature(required_security) {
                let query_params = ApiKeyMiddleware::parse_query_params(req.request());
                let raw_query = ApiKeyMiddleware::get_raw_query_string(req.request());

                let (body_params, raw_body) =
                    match ApiKeyMiddleware::parse_body_params(&mut req).await {
                        Ok(result) => result,
                        Err(e) => {
                            error!("Failed to parse body: {}", e);
                            return Err(actix_web::error::ErrorBadRequest(format!(
                                "Failed to parse request body: {}",
                                e
                            )));
                        }
                    };

                // Validate timestamp
                if let Err(e) = ApiKeyMiddleware::validate_timestamp_with_recv_window(
                    &query_params,
                    &body_params,
                    api_config.recv_window,
                    strict_timestamp_validation,
                ) {
                    error!("Timestamp validation failed: {}", e);
                    return Err(actix_web::error::ErrorBadRequest(format!(
                        "Timestamp validation failed: {}",
                        e
                    )));
                }

                // Validate signature
                if let Err(e) = ApiKeyMiddleware::validate_signature(
                    &api_config,
                    &query_params,
                    &body_params,
                    &raw_query,
                    &raw_body,
                ) {
                    error!("Signature validation failed: {}", e);
                    return Err(actix_web::error::ErrorBadRequest(format!(
                        "Signature validation failed: {}",
                        e
                    )));
                }
            }

            // Add API key info to request extensions for downstream handlers
            req.extensions_mut().insert(api_config.clone());

            info!(
                "API key validation successful for key: {} (security: {}, ip: {})",
                ApiKeyMiddleware::mask_api_key(&api_key),
                required_security,
                client_ip
            );

            // Forward the request to the next service
            service.call(req).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::api_key::{ActiveModel, Model, NewApiKey};
    use chrono::{DateTime, Utc};
    use tokio::sync::Notify;
    use uuid::Uuid;

    #[tokio::test]
    async fn cancelled_cache_fill_does_not_leak_epoch_bookkeeping() {
        // Finding G2-2: actix drops a request future when the client disconnects. If that happens
        // between begin_fill and finish_fill, `in_flight` stayed incremented forever, and because
        // pruning requires in_flight == 0 the per-key epoch entry could never be removed - it grew
        // past the LRU capacity with one entry per distinct key an attacker chose to send.
        const API_KEY: &str = "cancelled-fill-key";
        let store = ApiKeyStore::new(CacheConfig::default());

        {
            let guard = store.begin_cache_fill(API_KEY);
            assert_eq!(
                store
                    .l1_cache
                    .write(API_KEY)
                    .invalidation_epochs
                    .get(API_KEY)
                    .map(|epoch| epoch.in_flight),
                Some(1),
                "the fill must be recorded as in flight"
            );
            drop(guard); // the request future is cancelled here
        }

        let cache = store.l1_cache.write(API_KEY);
        assert!(
            !cache.invalidation_epochs.contains_key(API_KEY),
            "a cancelled fill must release its bookkeeping so the epoch entry can be pruned"
        );
    }

    #[test]
    fn lookups_of_keys_in_different_shards_do_not_share_a_lock() {
        const HELD_KEY: &str = "held-shard-key";
        let store = ApiKeyStore::new(CacheConfig::default());
        let other_key = (0..)
            .map(|i| format!("other-shard-key-{i}"))
            .find(|key| store.l1_cache.shard_index(key) != store.l1_cache.shard_index(HELD_KEY))
            .unwrap();
        let store = store.with_api_keys(vec![ApiKeyConfig::new(other_key.clone())]);

        // Another request holds the held key's shard. With one lock for the whole cache, this
        // lookup would wait until it is released.
        let _held = store.l1_cache.write(HELD_KEY);
        let (found_tx, found_rx) = std::sync::mpsc::channel();
        let lookup_store = store.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let found = runtime.block_on(lookup_store.get_api_key(&other_key));
            found_tx.send(found.is_some()).unwrap();
        });

        assert_eq!(found_rx.recv_timeout(Duration::from_secs(5)), Ok(true));
    }

    struct BlockingApiKeyRepository {
        model: Model,
        read_started: Notify,
        release_read: Notify,
    }

    #[async_trait::async_trait]
    impl ApiKeyRepository for BlockingApiKeyRepository {
        async fn find_by_api_key(&self, _api_key: &str) -> Result<Option<Model>, ApiKeyError> {
            self.read_started.notify_one();
            self.release_read.notified().await;
            Ok(Some(self.model.clone()))
        }

        async fn find_by_id(&self, _id: Uuid) -> Result<Option<Model>, ApiKeyError> {
            unreachable!()
        }

        async fn find_by_user_id(&self, _user_id: Uuid) -> Result<Vec<Model>, ApiKeyError> {
            unreachable!()
        }

        async fn create(&self, _new_api_key: NewApiKey) -> Result<Model, ApiKeyError> {
            unreachable!()
        }

        async fn update(&self, _id: Uuid, _updates: ActiveModel) -> Result<Model, ApiKeyError> {
            unreachable!()
        }

        async fn set_enabled(&self, _id: Uuid, _enabled: bool) -> Result<(), ApiKeyError> {
            unreachable!()
        }

        async fn update_last_used(
            &self,
            _id: Uuid,
            _timestamp: DateTime<Utc>,
        ) -> Result<(), ApiKeyError> {
            unreachable!()
        }

        async fn delete(&self, _id: Uuid) -> Result<(), ApiKeyError> {
            unreachable!()
        }

        async fn find_active_keys(&self, _limit: Option<u64>) -> Result<Vec<Model>, ApiKeyError> {
            unreachable!()
        }

        async fn find_expiring_keys(&self, _days: i64) -> Result<Vec<Model>, ApiKeyError> {
            unreachable!()
        }

        async fn count_by_user(&self, _user_id: Uuid) -> Result<u64, ApiKeyError> {
            unreachable!()
        }

        async fn find_by_permission(
            &self,
            _permission: SecurityType,
        ) -> Result<Vec<Model>, ApiKeyError> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn invalidation_during_fill_does_not_cache_stale_config() {
        const API_KEY: &str = "stale-fill-key";
        let now = Utc::now();
        let repository = Arc::new(BlockingApiKeyRepository {
            model: Model {
                id: Uuid::new_v4(),
                api_key: API_KEY.to_string(),
                key_material_type: None,
                key_material_value: None,
                permissions: vec![SecurityType::None],
                recv_window: 5_000,
                enabled: true,
                description: Some("stale config".to_string()),
                ip_blacklist: vec![],
                user_id: None,
                created_at: now,
                updated_at: now,
                expires_at: None,
                last_used_at: None,
            },
            read_started: Notify::new(),
            release_read: Notify::new(),
        });
        let store = ApiKeyStore::with_repository(
            CacheConfig {
                l1_max_size: 2,
                l1_ttl: Duration::from_secs(300),
            },
            repository.clone(),
        );

        let fill_store = store.clone();
        let fill = tokio::spawn(async move { fill_store.get_api_key(API_KEY).await });
        repository.read_started.notified().await;

        store.invalidate(API_KEY).await;
        repository.release_read.notify_one();

        let returned = fill.await.unwrap().unwrap();
        assert_eq!(returned.description.as_deref(), Some("stale config"));

        let cache = store.l1_cache.write(API_KEY);
        assert!(cache.entries.peek(API_KEY).is_none());
        assert!(!cache.invalidation_epochs.contains_key(API_KEY));
    }
}

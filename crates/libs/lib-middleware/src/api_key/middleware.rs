use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error, HttpMessage, HttpRequest, web,
};
use futures_util::future::{ready, LocalBoxFuture, Ready};
use std::{
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH, Duration},
};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::api_key::{
    error::ApiKeyError,
    security::{ApiKeyConfig, SecurityType, KeyMaterial},
    signature::SignatureVerifier,
};

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
/// 2. L2 (Redis)
/// 3. L3 (Database)
#[derive(Clone)]
pub struct ApiKeyStore {
    /// L1 Cache: In-memory LRU for hot keys
    l1_cache: Arc<RwLock<lru::LruCache<String, CachedApiKey>>>,
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
            l1_max_size: 100_000,  // 100K hot keys in memory
            l1_ttl: Duration::from_secs(300),  // 5 minute TTL
        }
    }
}

/// Cache performance metrics
pub struct CacheMetrics {
    pub l1_hits: std::sync::atomic::AtomicU64,
    pub l1_misses: std::sync::atomic::AtomicU64,
    pub db_queries: std::sync::atomic::AtomicU64,
}

impl CacheMetrics {
    pub fn new() -> Self {
        Self {
            l1_hits: std::sync::atomic::AtomicU64::new(0),
            l1_misses: std::sync::atomic::AtomicU64::new(0),
            db_queries: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Get L1 cache hit rate for monitoring
    pub fn get_l1_hit_rate(&self) -> f64 {
        let hits = self.l1_hits.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.l1_misses.load(std::sync::atomic::Ordering::Relaxed);
        if hits + misses == 0 { 0.0 } else { hits as f64 / (hits + misses) as f64 }
    }
}

impl ApiKeyStore {
    /// Create new store 
    pub fn new(config: CacheConfig) -> Self {
        let l1_cache = Arc::new(RwLock::new(
            lru::LruCache::new(
                std::num::NonZeroUsize::new(config.l1_max_size)
                    .expect("L1 cache size must be > 0")
            )
        ));

        Self {
            l1_cache,
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

    /// Fast API key lookup with caching
    pub async fn get_api_key(&self, api_key: &str) -> Option<Arc<ApiKeyConfig>> {
        // L1 Cache: Check in-memory LRU first 
        {
            let mut cache = self.l1_cache.write().await;
            if let Some(cached) = cache.get(api_key) {
                if !cached.is_expired(self.config.l1_ttl) {
                    self.metrics.l1_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    debug!("L1 cache hit for key: {}", api_key);
                    return Some(cached.config.clone());
                } else {
                    // Remove expired entry
                    cache.pop(api_key);
                }
            }
        }
        
        self.metrics.l1_misses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // L2/L3: Check memory store (fallback for now, will be Redis + DB)
        if let Some(config) = self.memory_store.get(api_key) {
            self.metrics.db_queries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let config_arc = Arc::new(config.clone());
            
            // Update L1 cache
            let cached = CachedApiKey::new(config.clone());
            let mut cache = self.l1_cache.write().await;
            cache.put(api_key.to_string(), cached);
            
            debug!("Memory store hit for key: {}", api_key);
            return Some(config_arc);
        }

        // TODO: Add Redis (L2) and Database (L3) lookup here
        // if let Some(config) = self.check_redis(api_key).await {
        //     self.update_l1_cache(api_key, config.clone()).await;
        //     return Some(config);
        // }
        // 
        // if let Some(config) = self.check_database(api_key).await {
        //     self.update_redis(api_key, &config).await;
        //     self.update_l1_cache(api_key, config.clone()).await;
        //     return Some(config);
        // }

        warn!("API key not found: {}", api_key);
        None
    }

    /// Invalidate cache entry (when key is updated/disabled)
    pub async fn invalidate(&self, api_key: &str) {
        let mut cache = self.l1_cache.write().await;
        cache.pop(api_key);
        // TODO: Invalidate Redis cache when implemented
        info!("Invalidated cache for key: {}", api_key);
    }

    /// Get cache metrics for monitoring
    pub fn metrics(&self) -> &CacheMetrics {
        &self.metrics
    }
}

pub struct ApiKeyMiddleware {
    api_key_store: ApiKeyStore,
    default_security: SecurityType,
    route_configs: Arc<Vec<RouteSecurityConfig>>,
    strict_timestamp_validation: bool,
}

impl ApiKeyMiddleware {
    /// Create new middleware 
    pub fn new() -> Self {
        Self {
            api_key_store: ApiKeyStore::new(CacheConfig::default()),
            default_security: SecurityType::None,
            route_configs: Arc::new(vec![]),
            strict_timestamp_validation: true,
        }
    }

    /// Configure with custom cache settings
    pub fn with_cache_config(mut self, config: CacheConfig) -> Self {
        self.api_key_store = ApiKeyStore::new(config);
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

        let key_material = api_config
            .get_key_material()
            .ok_or_else(|| ApiKeyError::InvalidApiKey("No key material configured for signed endpoint".to_string()))?;

        // Build payload based on whether we have mixed query/body or separate parameters
        let payload = if !raw_query.is_empty() && !raw_body.is_empty() {
            // Mixed case as per Binance example 3
            SignatureVerifier::build_mixed_payload(raw_query, raw_body)
        } else {
            // Standard case
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

    /// Parse body parameters from different content types
    async fn parse_body_params(req: &mut ServiceRequest) -> Result<(HashMap<String, String>, String), ApiKeyError> {
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
            let chunk = chunk.map_err(|e| ApiKeyError::Internal(format!("Failed to read body: {}", e)))?;
            body.extend_from_slice(&chunk);
        }

        let body_string = String::from_utf8(body.to_vec())
            .map_err(|e| ApiKeyError::Internal(format!("Invalid UTF-8 in body: {}", e)))?;

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
            if let Some(recv_window_str) = query_params.get("recvWindow").or_else(|| body_params.get("recvWindow")) {
                let provided_recv_window = recv_window_str
                    .parse::<u64>()
                    .map_err(|_| ApiKeyError::InvalidTimestamp("Invalid recvWindow format".to_string()))?;
                
                if provided_recv_window > 60000 {
                    return Err(ApiKeyError::InvalidTimestamp("recvWindow cannot exceed 60000ms".to_string()));
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
        let _default_security = self.default_security;
        let route_configs = self.route_configs.clone();
        let strict_timestamp_validation = self.strict_timestamp_validation;
        let service = Rc::clone(&self.service);

        Box::pin(async move {
            // Determine required security level for this route
            let path = req.uri().path();
            let required_security = ApiKeyMiddleware::get_route_security_type(
                &ApiKeyMiddleware::new(),
                path,
                &route_configs,
            );

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
                        "API key required: {}", e
                    )));
                }
            };

            // Get API key configuration
            let api_config = match api_key_store.get_api_key(&api_key).await {
                Some(config) => config,
                None => {
                    error!("API key not found: {}", api_key);
                    return Err(actix_web::error::ErrorUnauthorized("Invalid API key"));
                }
            };

            // Validate API key state
            if !api_config.is_valid() {
                warn!("API key is disabled or expired: {}", api_key);
                return Err(actix_web::error::ErrorUnauthorized("API key is disabled or expired"));
            }

            // Validate IP whitelist
            let client_ip = ApiKeyMiddleware::extract_client_ip(req.request());
            if !api_config.is_ip_allowed(&client_ip) {
                warn!("IP {} not in whitelist for API key: {}", client_ip, api_key);
                return Err(actix_web::error::ErrorForbidden("IP address not allowed"));
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
                
                let (body_params, raw_body) = match ApiKeyMiddleware::parse_body_params(&mut req).await {
                    Ok(result) => result,
                    Err(e) => {
                        error!("Failed to parse body: {}", e);
                        return Err(actix_web::error::ErrorBadRequest(format!(
                            "Failed to parse request body: {}", e
                        )));
                    }
                };

                // Validate timestamp
                if let Err(e) = ApiKeyMiddleware::validate_timestamp_with_recv_window(
                    &query_params, 
                    &body_params, 
                    api_config.recv_window,
                    strict_timestamp_validation
                ) {
                    error!("Timestamp validation failed: {}", e);
                    return Err(actix_web::error::ErrorBadRequest(format!(
                        "Timestamp validation failed: {}", e
                    )));
                }

                // Validate signature
                if let Err(e) = ApiKeyMiddleware::validate_signature(
                    &api_config, 
                    &query_params, 
                    &body_params,
                    &raw_query,
                    &raw_body
                ) {
                    error!("Signature validation failed: {}", e);
                    return Err(actix_web::error::ErrorBadRequest(format!(
                        "Signature validation failed: {}", e
                    )));
                }
            }

            // Add API key info to request extensions for downstream handlers
            req.extensions_mut().insert(api_config.clone());

            info!(
                "API key validation successful for key: {} (security: {}, ip: {})", 
                api_key, required_security, client_ip
            );
            
            // Forward the request to the next service
            service.call(req).await
        })
    }
}
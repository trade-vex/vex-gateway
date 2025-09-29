use crate::{
    error::{DbError, Result},
    traits::{AdvancedKvStore, AtomicKvStore, BasicKvStore, ExpiringKvStore, PoolStats},
};
use async_trait::async_trait;
use dashmap::{DashMap, mapref::entry::Entry};
use once_cell::sync::Lazy;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// Global registry for multiple memory stores
///
/// This registry allows you to create and manage multiple isolated memory stores,
/// each with its own configuration and data. This is useful for separating
/// different types of cached data (e.g., user sessions, product cache, etc.).
///
/// ## Example Usage
/// ```rust
/// use lib_db::memory_kv::REGISTRY;
/// use lib_db::traits::BasicKvStore;
/// use lib_db::error::Result;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     // Get different stores for different purposes
///     let user_cache = REGISTRY.get_or_create("user_cache").await;
///     let session_store = REGISTRY.get_or_create("sessions").await;
///     let temp_data = REGISTRY.get_or_create("temp_data").await;
///     
///     // Each store is completely isolated
///     user_cache.set("user:123", &"Alice").await?;
///     session_store.set("session:456", &"token").await?;
///     temp_data.set("temp:789", &"data").await?;
///     
///     Ok(())
/// }
/// ```
pub static REGISTRY: Lazy<MemoryStoreRegistry> = Lazy::new(MemoryStoreRegistry::new);

/// Registry for managing multiple memory stores
///
/// This registry provides a centralized way to create and access multiple
/// isolated memory stores. Each store can have its own configuration and
/// operates independently of others.
#[derive(Clone)]
pub struct MemoryStoreRegistry {
    stores: DashMap<String, Arc<MemoryDb>>,
    default_config: MemoryConfig,
}

impl Default for MemoryStoreRegistry {
    fn default() -> Self {
        Self {
            stores: DashMap::new(),
            default_config: MemoryConfig::default(),
        }
    }
}

impl MemoryStoreRegistry {
    /// Create a new registry with default configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new registry with custom default configuration
    pub fn with_default_config(config: MemoryConfig) -> Self {
        Self {
            stores: DashMap::new(),
            default_config: config,
        }
    }

    /// Get or create a named store with default configuration
    pub async fn get_or_create(&self, name: &str) -> Arc<MemoryDb> {
        match self.stores.entry(name.to_string()) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let store = Arc::new(MemoryDb::with_config(name, self.default_config.clone()));
                entry.insert(store.clone());
                store
            }
        }
    }

    /// Get or create a named store with custom configuration
    pub async fn get_or_create_with_config(
        &self,
        name: &str,
        config: MemoryConfig,
    ) -> Arc<MemoryDb> {
        self.stores
            .entry(name.to_string())
            .or_insert_with(move || Arc::new(MemoryDb::with_config(name, config)))
            .clone()
    }

    /// List all store names
    pub fn list_stores(&self) -> Vec<String> {
        self.stores
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Remove a store from the registry
    pub fn remove_store(&self, name: &str) -> Option<Arc<MemoryDb>> {
        self.stores.remove(name).map(|(_, store)| store)
    }

    /// Get statistics for all stores
    pub async fn stats(&self) -> RegistryStats {
        // First collect all store clones to avoid holding iteration guards across await
        let stores: Vec<Arc<MemoryDb>> = self
            .stores
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        let mut total_entries = 0;
        let mut total_memory = 0;
        let store_count = stores.len();

        // Now iterate over the collected stores and compute stats
        for store in stores {
            let stats = store.stats().await;
            total_entries += stats.total_entries;
            total_memory += stats.memory_usage;
        }

        RegistryStats {
            store_count,
            total_entries,
            total_memory,
        }
    }

    /// Check if a store exists
    pub fn has_store(&self, name: &str) -> bool {
        self.stores.contains_key(name)
    }
}

/// Statistics for the entire registry
#[derive(Debug, Clone)]
pub struct RegistryStats {
    pub store_count: usize,
    pub total_entries: usize,
    pub total_memory: usize,
}

/// High-performance in-memory key-value store implementation
///
/// This implementation uses DashMap for fine-grained locking, providing
/// excellent concurrency performance. It's suitable for use cases like:
/// - Application-level caching
/// - Session storage
/// - Temporary data storage
/// - High-performance lookups
///
/// ## Features
/// - Full CRUD operations with atomic guarantees
/// - Automatic expiration with background cleanup
/// - Batch operations for performance
/// - Memory-efficient storage
/// - Thread-safe concurrent access with fine-grained locking
/// - Configurable capacity limits and eviction policies
///
/// ## Performance Benefits
/// - **DashMap**: Fine-grained locking per key instead of global locks
/// - **Concurrent Access**: Multiple operations can happen simultaneously
/// - **No Read-Write Contention**: Reads don't block other reads
/// - **Background Cleanup**: Automatic expiration without blocking operations
///
/// ## Example Usage
/// ```rust
/// use lib_db::traits::{BasicKvStore, AdvancedKvStore, AtomicKvStore, ExpiringKvStore};
/// use lib_db::memory_kv::MemoryDb;
/// use lib_db::error::Result;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     // Connect to a named store
///     let kv = MemoryDb::connect("memory://my_cache").await?;
///     
///     // All operations available with high concurrency
///     kv.set("user:123", &"Alice").await?;
///     kv.set_ex("session:123", &"token", 3600).await?; // 1 hour TTL
///     kv.incr("counter", 1).await?;
///     kv.ping().await?;
///     
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct MemoryDb {
    store: DashMap<String, MemoryEntry>,
    config: MemoryConfig,
    name: String,
    #[allow(dead_code)]
    cleanup_task: Option<Arc<JoinHandle<()>>>,
}

/// Internal storage entry with metadata
#[derive(Clone)]
struct MemoryEntry {
    data: Vec<u8>,
    expires_at: Option<Instant>,
    #[allow(dead_code)]
    created_at: Instant,
    access_count: u64,
    last_accessed: Instant,
}

/// Configuration for memory store behavior
#[derive(Clone, Debug)]
pub struct MemoryConfig {
    /// Maximum number of entries in the store
    pub max_entries: usize,
    /// Default TTL for entries (None = no expiration)
    pub default_ttl: Option<Duration>,
    /// Enable background cleanup of expired entries
    pub enable_cleanup: bool,
    /// Cleanup interval
    pub cleanup_interval: Duration,
    /// Eviction policy when capacity is reached
    pub eviction_policy: EvictionPolicy,
    /// Enable compression for stored data
    pub compression_enabled: bool,
    /// Enable metrics collection
    pub metrics_enabled: bool,
}

/// Eviction policy for when capacity is reached
#[derive(Clone, Debug, Default)]
pub enum EvictionPolicy {
    /// No eviction (return error when capacity reached)
    #[default]
    None,
    /// Least Recently Used
    LRU,
    /// Least Frequently Used
    LFU,
    /// Time-based only (expired entries)
    TTL,
    /// Random eviction
    Random,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            default_ttl: None,
            enable_cleanup: true,
            cleanup_interval: Duration::from_secs(60), // 1 minute
            eviction_policy: EvictionPolicy::None,
            compression_enabled: false,
            metrics_enabled: true,
        }
    }
}

impl MemoryDb {
    /// Create a new in-memory store with default configuration
    pub fn new(name: &str) -> Self {
        Self::with_config(name, MemoryConfig::default())
    }

    /// Create a new in-memory store with custom configuration
    pub fn with_config(name: &str, config: MemoryConfig) -> Self {
        let store = DashMap::new();
        let name = name.to_string();

        // Start background cleanup if enabled
        let cleanup_task = if config.enable_cleanup {
            let store_clone = store.clone();
            let cleanup_interval = config.cleanup_interval;
            let name_clone = name.clone();

            let handle = tokio::spawn(async move {
                let mut interval = tokio::time::interval(cleanup_interval);
                loop {
                    interval.tick().await;
                    Self::cleanup_expired_entries(&store_clone, &name_clone);
                }
            });
            Some(Arc::new(handle))
        } else {
            None
        };

        Self {
            store,
            config,
            name,
            cleanup_task,
        }
    }

    /// Clean up expired entries (non-async since DashMap handles locking)
    fn cleanup_expired_entries(store: &DashMap<String, MemoryEntry>, _name: &str) {
        let now = Instant::now();
        store.retain(|_, entry| entry.expires_at.is_none_or(|expires_at| expires_at > now));
    }

    /// Check if an entry is expired
    fn is_expired(entry: &MemoryEntry) -> bool {
        entry
            .expires_at
            .is_some_and(|expires_at| expires_at <= Instant::now())
    }

    /// Update access metadata
    fn update_access(entry: &mut MemoryEntry) {
        entry.access_count += 1;
        entry.last_accessed = Instant::now();
    }

    /// Ensure capacity for new entries
    async fn ensure_capacity(&self, new_keys: usize) -> Result<()> {
        let current_len = self.store.len();

        if current_len + new_keys <= self.config.max_entries {
            return Ok(());
        }

        // Implement eviction based on policy
        match self.config.eviction_policy {
            EvictionPolicy::LRU => self.evict_lru(new_keys).await,
            EvictionPolicy::LFU => self.evict_lfu(new_keys).await,
            EvictionPolicy::TTL => self.evict_expired().await,
            EvictionPolicy::Random => self.evict_random(new_keys).await,
            EvictionPolicy::None => Err(DbError::Connection {
                message: format!("Memory store '{}' capacity limit reached", self.name),
                source: None,
            }),
        }
    }

    /// Evict least recently used entries
    async fn evict_lru(&self, needed: usize) -> Result<()> {
        let mut entries: Vec<_> = self
            .store
            .iter()
            .map(|entry| {
                let key = entry.key().clone();
                let last_accessed = entry.last_accessed;
                (key, last_accessed)
            })
            .collect();

        entries.sort_by_key(|(_, last_accessed)| *last_accessed);

        for (key, _) in entries.iter().take(needed) {
            self.store.remove(key.as_str());
        }

        Ok(())
    }

    /// Evict least frequently used entries
    async fn evict_lfu(&self, needed: usize) -> Result<()> {
        let mut entries: Vec<_> = self
            .store
            .iter()
            .map(|entry| {
                let key = entry.key().clone();
                let access_count = entry.access_count;
                (key, access_count)
            })
            .collect();

        entries.sort_by_key(|(_, access_count)| *access_count);

        for (key, _) in entries.iter().take(needed) {
            self.store.remove(key.as_str());
        }

        Ok(())
    }

    /// Evict expired entries
    async fn evict_expired(&self) -> Result<()> {
        Self::cleanup_expired_entries(&self.store, &self.name);
        Ok(())
    }

    /// Evict random entries
    async fn evict_random(&self, needed: usize) -> Result<()> {
        use rand::seq::SliceRandom;
        use rand::thread_rng;

        let keys: Vec<_> = self.store.iter().map(|entry| entry.key().clone()).collect();
        let mut rng = thread_rng();

        for key in keys.choose_multiple(&mut rng, needed) {
            self.store.remove(key);
        }

        Ok(())
    }

    /// Get current store statistics
    pub async fn stats(&self) -> MemoryStats {
        let total_entries = self.store.len();
        let expired_entries = self
            .store
            .iter()
            .filter(|entry| Self::is_expired(entry.value()))
            .count();

        let memory_usage = self
            .store
            .iter()
            .map(|entry| std::mem::size_of::<MemoryEntry>() + entry.value().data.len())
            .sum();

        MemoryStats {
            total_entries,
            expired_entries,
            max_entries: self.config.max_entries,
            memory_usage,
            store_name: self.name.clone(),
        }
    }

    /// Get the store name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Clear all entries (useful for testing)
    #[cfg(test)]
    pub async fn clear(&self) {
        self.store.clear();
    }

    /// Get the number of entries (useful for testing)
    #[cfg(test)]
    pub async fn len(&self) -> usize {
        self.store.len()
    }

    /// Check if the store is empty (useful for testing)
    #[cfg(test)]
    pub async fn is_empty(&self) -> bool {
        self.store.is_empty()
    }
}

/// Statistics for a memory store
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_entries: usize,
    pub expired_entries: usize,
    pub max_entries: usize,
    pub memory_usage: usize,
    pub store_name: String,
}

#[async_trait]
impl BasicKvStore for MemoryDb {
    async fn connect(url: &str) -> Result<Self> {
        // Parse memory://store_name?config=options
        if let Some(store_name) = url.strip_prefix("memory://") {
            let (name, config) = Self::parse_url_config(store_name);
            Ok(Self::with_config(&name, config))
        } else {
            Err(DbError::Configuration {
                message: "Invalid memory URL format. Expected 'memory://store_name'".to_string(),
            })
        }
    }

    async fn set<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize + Send + Sync,
    {
        let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
            message: format!("Failed to serialize value: {}", e),
            source: Some(Box::new(e)),
        })?;

        // Check capacity before inserting
        if !self.store.contains_key(key) {
            self.ensure_capacity(1).await?;
        }

        let expires_at = self.config.default_ttl.map(|ttl| Instant::now() + ttl);
        let now = Instant::now();

        self.store.insert(
            key.to_string(),
            MemoryEntry {
                data: bytes,
                expires_at,
                created_at: now,
                access_count: 0,
                last_accessed: now,
            },
        );

        Ok(())
    }

    async fn get<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        if let Some(mut entry) = self.store.get_mut(key) {
            // Check if expired
            if Self::is_expired(&entry) {
                return Ok(None);
            }

            // Update access metadata
            Self::update_access(&mut entry);

            let value = bincode::deserialize(&entry.data).map_err(|e| DbError::Serialization {
                message: format!("Failed to deserialize value: {}", e),
                source: Some(Box::new(e)),
            })?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    async fn delete(&self, key: &str) -> Result<bool> {
        Ok(self.store.remove(key).is_some())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        if let Some(entry) = self.store.get(key) {
            Ok(!Self::is_expired(&entry))
        } else {
            Ok(false)
        }
    }

    async fn set_bytes(&self, key: &str, value: &[u8]) -> Result<()> {
        // Check capacity before inserting
        if !self.store.contains_key(key) {
            self.ensure_capacity(1).await?;
        }

        let expires_at = self.config.default_ttl.map(|ttl| Instant::now() + ttl);
        let now = Instant::now();

        self.store.insert(
            key.to_string(),
            MemoryEntry {
                data: value.to_vec(),
                expires_at,
                created_at: now,
                access_count: 0,
                last_accessed: now,
            },
        );

        Ok(())
    }

    async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if let Some(mut entry) = self.store.get_mut(key) {
            if Self::is_expired(&entry) {
                Ok(None)
            } else {
                Self::update_access(&mut entry);
                Ok(Some(entry.data.clone()))
            }
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl AdvancedKvStore for MemoryDb {
    async fn ping(&self) -> Result<()> {
        // Always successful for in-memory store
        Ok(())
    }

    fn pool_stats(&self) -> PoolStats {
        // For in-memory store, we return stats about the store itself
        PoolStats {
            size: self.store.len() as u32,
            idle: 0,   // Not applicable for in-memory
            active: 0, // Not applicable for in-memory
            max_size: self.config.max_entries as u32,
        }
    }

    async fn mget<T>(&self, keys: &[&str]) -> Result<Vec<Option<T>>>
    where
        T: DeserializeOwned,
    {
        let mut results = Vec::with_capacity(keys.len());

        for &key in keys {
            if let Some(mut entry) = self.store.get_mut(key) {
                if Self::is_expired(&entry) {
                    results.push(None);
                } else {
                    Self::update_access(&mut entry);
                    let value =
                        bincode::deserialize(&entry.data).map_err(|e| DbError::Serialization {
                            message: format!("Failed to deserialize value: {}", e),
                            source: Some(Box::new(e)),
                        })?;
                    results.push(Some(value));
                }
            } else {
                results.push(None);
            }
        }

        Ok(results)
    }

    async fn mset<T>(&self, pairs: &[(&str, &T)]) -> Result<()>
    where
        T: Serialize + Send + Sync,
    {
        // Check capacity for new keys
        let new_keys = pairs
            .iter()
            .filter(|(key, _)| !self.store.contains_key(*key))
            .count();

        if new_keys > 0 {
            self.ensure_capacity(new_keys).await?;
        }

        let expires_at = self.config.default_ttl.map(|ttl| Instant::now() + ttl);
        let now = Instant::now();

        for (key, value) in pairs {
            let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
                message: format!("Failed to serialize value: {}", e),
                source: Some(Box::new(e)),
            })?;

            self.store.insert(
                key.to_string(),
                MemoryEntry {
                    data: bytes,
                    expires_at,
                    created_at: now,
                    access_count: 0,
                    last_accessed: now,
                },
            );
        }

        Ok(())
    }

    async fn mdel(&self, keys: &[&str]) -> Result<u64> {
        let mut deleted = 0;

        for &key in keys {
            if self.store.remove(key).is_some() {
                deleted += 1;
            }
        }

        Ok(deleted)
    }
}

#[async_trait]
impl AtomicKvStore for MemoryDb {
    async fn incr(&self, key: &str, delta: i64) -> Result<i64> {
        if let Some(mut entry) = self.store.get_mut(key) {
            if Self::is_expired(&entry) {
                let now = Instant::now();
                let new_value = delta;
                entry.data =
                    bincode::serialize(&new_value).map_err(|e| DbError::Serialization {
                        message: format!("Failed to serialize value: {}", e),
                        source: Some(Box::new(e)),
                    })?;
                entry.expires_at = self.config.default_ttl.map(|ttl| now + ttl);
                entry.created_at = now;
                entry.access_count = 0;
                entry.last_accessed = now;
                return Ok(new_value);
            }
            let current = bincode::deserialize::<i64>(&entry.data).unwrap_or(0);
            let new_value = current + delta;
            entry.data = bincode::serialize(&new_value).map_err(|e| DbError::Serialization {
                message: format!("Failed to serialize value: {}", e),
                source: Some(Box::new(e)),
            })?;
            Self::update_access(&mut entry);
            return Ok(new_value);
        }

        // Insert new key: ensure capacity then insert
        self.ensure_capacity(1).await?;
        let now = Instant::now();
        let expires_at = self.config.default_ttl.map(|ttl| now + ttl);
        let new_value = delta;
        let bytes = bincode::serialize(&new_value).map_err(|e| DbError::Serialization {
            message: format!("Failed to serialize value: {}", e),
            source: Some(Box::new(e)),
        })?;
        self.store.insert(
            key.to_string(),
            MemoryEntry {
                data: bytes,
                expires_at,
                created_at: now,
                access_count: 0,
                last_accessed: now,
            },
        );
        Ok(new_value)
    }

    async fn set_nx<T>(&self, key: &str, value: &T) -> Result<bool>
    where
        T: Serialize + Send + Sync,
    {
        use dashmap::mapref::entry::Entry;
        match self.store.entry(key.to_string()) {
            Entry::Occupied(mut occ) => {
                if Self::is_expired(occ.get()) {
                    let now = Instant::now();
                    let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
                        message: format!("Failed to serialize value: {}", e),
                        source: Some(Box::new(e)),
                    })?;
                    occ.insert(MemoryEntry {
                        data: bytes,
                        expires_at: self.config.default_ttl.map(|ttl| now + ttl),
                        created_at: now,
                        access_count: 0,
                        last_accessed: now,
                    });
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Entry::Vacant(vac) => {
                self.ensure_capacity(1).await?;
                let now = Instant::now();
                let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
                    message: format!("Failed to serialize value: {}", e),
                    source: Some(Box::new(e)),
                })?;
                vac.insert(MemoryEntry {
                    data: bytes,
                    expires_at: self.config.default_ttl.map(|ttl| now + ttl),
                    created_at: now,
                    access_count: 0,
                    last_accessed: now,
                });
                Ok(true)
            }
        }
    }

    async fn cas<T>(&self, key: &str, old: &T, new: &T) -> Result<bool>
    where
        T: Serialize + DeserializeOwned + Send + Sync + PartialEq,
    {
        if let Some(mut entry) = self.store.get_mut(key) {
            if Self::is_expired(&entry) {
                return Ok(false);
            }
            let current = bincode::deserialize::<T>(&entry.data).ok();
            if current.as_ref() != Some(old) {
                return Ok(false);
            }
            let now = Instant::now();
            entry.data = bincode::serialize(new).map_err(|e| DbError::Serialization {
                message: format!("Failed to serialize value: {}", e),
                source: Some(Box::new(e)),
            })?;
            entry.expires_at = self.config.default_ttl.map(|ttl| now + ttl);
            Self::update_access(&mut entry);
            return Ok(true);
        }
        Ok(false)
    }
}

#[async_trait]
impl ExpiringKvStore for MemoryDb {
    async fn set_ex<T>(&self, key: &str, value: &T, ttl_secs: u64) -> Result<()>
    where
        T: Serialize + Send + Sync,
    {
        let bytes = bincode::serialize(value).map_err(|e| DbError::Serialization {
            message: format!("Failed to serialize value: {}", e),
            source: Some(Box::new(e)),
        })?;

        // Check capacity before inserting
        if !self.store.contains_key(key) {
            self.ensure_capacity(1).await?;
        }

        let expires_at = Instant::now() + Duration::from_secs(ttl_secs);
        let now = Instant::now();

        self.store.insert(
            key.to_string(),
            MemoryEntry {
                data: bytes,
                expires_at: Some(expires_at),
                created_at: now,
                access_count: 0,
                last_accessed: now,
            },
        );

        Ok(())
    }

    async fn expire(&self, key: &str, ttl_secs: u64) -> Result<bool> {
        if let Some(mut entry) = self.store.get_mut(key) {
            if !Self::is_expired(&entry) {
                entry.expires_at = Some(Instant::now() + Duration::from_secs(ttl_secs));
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn ttl(&self, key: &str) -> Result<Option<u64>> {
        if let Some(entry) = self.store.get(key) {
            if let Some(expires_at) = entry.expires_at {
                let now = Instant::now();
                if expires_at > now {
                    let remaining = expires_at.duration_since(now).as_secs();
                    Ok(Some(remaining))
                } else {
                    Ok(Some(0)) // Expired
                }
            } else {
                Ok(None) // No TTL set
            }
        } else {
            Ok(None) // Key doesn't exist
        }
    }
}

impl MemoryDb {
    /// Parse URL configuration from memory://store_name?param=value format
    fn parse_url_config(url: &str) -> (String, MemoryConfig) {
        let mut config = MemoryConfig::default();
        let name = if let Some((store_name, params)) = url.split_once('?') {
            // Parse query parameters
            for param in params.split('&') {
                if let Some((key, value)) = param.split_once('=') {
                    match key {
                        "max_entries" => {
                            if let Ok(max) = value.parse::<usize>() {
                                config.max_entries = max;
                            }
                        }
                        "ttl" => {
                            if let Ok(ttl_secs) = value.parse::<u64>() {
                                config.default_ttl = Some(Duration::from_secs(ttl_secs));
                            }
                        }
                        "eviction" => {
                            config.eviction_policy = match value {
                                "lru" => EvictionPolicy::LRU,
                                "lfu" => EvictionPolicy::LFU,
                                "ttl" => EvictionPolicy::TTL,
                                "random" => EvictionPolicy::Random,
                                _ => EvictionPolicy::None,
                            };
                        }
                        "cleanup" => {
                            config.enable_cleanup = value == "true";
                        }
                        _ => {}
                    }
                }
            }
            store_name.to_string()
        } else {
            url.to_string()
        };

        (name, config)
    }
}

impl Default for MemoryDb {
    fn default() -> Self {
        Self::new("default")
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use serde::{Deserialize, Serialize};

//     #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
//     struct TestUser {
//         id: u32,
//         name: String,
//     }

//     #[tokio::test]
//     async fn test_basic_operations() {
//         let kv = MemoryDb::connect("memory://test_store").await.unwrap();

//         // Test set and get
//         let user = TestUser {
//             id: 123,
//             name: "Alice".to_string(),
//         };

//         kv.set("user:123", &user).await.unwrap();
//         let retrieved: Option<TestUser> = kv.get("user:123").await.unwrap();
//         assert_eq!(retrieved, Some(user));

//         // Test exists
//         assert!(kv.exists("user:123").await.unwrap());
//         assert!(!kv.exists("user:456").await.unwrap());

//         // Test delete
//         assert!(kv.delete("user:123").await.unwrap());
//         assert!(!kv.delete("user:123").await.unwrap()); // Already deleted

//         // Test get after delete
//         let retrieved: Option<TestUser> = kv.get("user:123").await.unwrap();
//         assert_eq!(retrieved, None);
//     }

//     // #[tokio::test]
//     // async fn test_advanced_operations() {
//     //     let kv = MemoryDb::connect("memory://test_advanced").await.unwrap();

//     //     // Test batch operations
//     //     let user1 = TestUser {
//     //         id: 1,
//     //         name: "Alice".to_string(),
//     //     };
//     //     let user2 = TestUser {
//     //         id: 2,
//     //         name: "Bob".to_string(),
//     //     };

//     //     kv.mset(&[("user:1", &user1), ("user:2", &user2)])
//     //         .await
//     //         .unwrap();
//     //     let users: Vec<Option<TestUser>> = kv.mget(&["user:1", "user:2", "user:3"]).await.unwrap();
//     //     assert_eq!(users.len(), 3);
//     //     assert_eq!(users[0], Some(user1));
//     //     assert_eq!(users[1], Some(user2));
//     //     assert_eq!(users[2], None);

//     //     // Test atomic operations
//     //     let new_count = kv.incr("counter", 5).await.unwrap();
//     //     assert_eq!(new_count, 5);

//     //     let was_set = kv.set_nx("lock:resource", &"locked").await.unwrap();
//     //     assert!(was_set);
//     //     let was_set_again = kv.set_nx("lock:resource", &"locked").await.unwrap();
//     //     assert!(!was_set_again);

//     //     // Test CAS
//     //     let swapped = kv.cas("counter", &5, &10).await.unwrap();
//     //     assert!(swapped);
//     //     let swapped_wrong = kv.cas("counter", &5, &15).await.unwrap();
//     //     assert!(!swapped_wrong);
//     // }

//     #[tokio::test]
//     async fn test_expiration() {
//         let kv = MemoryDb::connect("memory://test_expiration").await.unwrap();

//         // Test set with expiration
//         kv.set_ex("session:123", &"token", 1).await.unwrap(); // 1 second TTL

//         // Should exist immediately
//         assert!(kv.exists("session:123").await.unwrap());

//         // Wait for expiration
//         tokio::time::sleep(Duration::from_millis(1100)).await;

//         // Should not exist after expiration
//         assert!(!kv.exists("session:123").await.unwrap());

//         // Test TTL
//         kv.set_ex("temp:key", &"value", 60).await.unwrap();
//         let ttl = kv.ttl("temp:key").await.unwrap();
//         assert!(ttl.is_some());
//         assert!(ttl.unwrap() <= 60);
//     }

//     #[tokio::test]
//     async fn test_capacity_limit() {
//         let config = MemoryConfig {
//             max_entries: 2,
//             eviction_policy: EvictionPolicy::LRU,
//             ..Default::default()
//         };
//         let kv = MemoryDb::with_config("test_capacity", config);

//         // Should be able to set 2 entries
//         kv.set("key1", &"value1").await.unwrap();
//         kv.set("key2", &"value2").await.unwrap();

//         // Third entry should trigger eviction
//         kv.set("key3", &"value3").await.unwrap();

//         // First key should be evicted (LRU)
//         assert!(!kv.exists("key1").await.unwrap());
//         assert!(kv.exists("key2").await.unwrap());
//         assert!(kv.exists("key3").await.unwrap());
//     }

//     #[tokio::test]
//     async fn test_stats() {
//         let kv = MemoryDb::connect("memory://test_stats").await.unwrap();

//         kv.set("key1", &"value1").await.unwrap();
//         kv.set("key2", &"value2").await.unwrap();

//         let stats = kv.stats().await;
//         assert_eq!(stats.total_entries, 2);
//         assert_eq!(stats.expired_entries, 0);
//         assert_eq!(stats.store_name, "test_stats");
//     }

//     #[tokio::test]
//     async fn test_registry() {
//         // Test registry functionality
//         let user_cache = REGISTRY.get_or_create("user_cache").await;
//         let session_store = REGISTRY.get_or_create("sessions").await;

//         // Each store is isolated
//         user_cache.set("user:123", &"Alice").await.unwrap();
//         session_store.set("session:456", &"token").await.unwrap();

//         // Data is separate
//         let user: Option<String> = user_cache.get("user:123").await.unwrap();
//         let session: Option<String> = session_store.get("session:456").await.unwrap();

//         assert_eq!(user, Some("Alice".to_string()));
//         assert_eq!(session, Some("token".to_string()));

//         // User data not in session store
//         let user_in_session: Option<String> = session_store.get("user:123").await.unwrap();
//         assert_eq!(user_in_session, None);

//         // List stores
//         let stores = REGISTRY.list_stores();
//         assert!(stores.contains(&"user_cache".to_string()));
//         assert!(stores.contains(&"sessions".to_string()));
//     }

//     #[tokio::test]
//     async fn test_url_parsing() {
//         // Test URL parsing with parameters
//         let kv = MemoryDb::connect("memory://my_store?max_entries=100&ttl=3600&eviction=lru")
//             .await
//             .unwrap();

//         assert_eq!(kv.name(), "my_store");

//         // Test stats to verify configuration
//         let stats = kv.stats().await;
//         assert_eq!(stats.max_entries, 100);
//     }
// }

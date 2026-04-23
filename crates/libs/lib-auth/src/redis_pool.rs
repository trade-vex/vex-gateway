//! Fan-out Redis connection pool.
//!
//! `redis::aio::ConnectionManager` is a single multiplexed TCP socket; under
//! load every authenticated request queues behind it. This wrapper holds N
//! independent `ConnectionManager` instances (N real TCP sockets) and
//! round-robins requests across them, eliminating the single-socket
//! head-of-line bottleneck.
//!
//! Each `get()` returns a cheap clone of one of the underlying managers, which
//! itself impls `redis::aio::ConnectionLike` + `AsyncCommands`, so callers can
//! use it exactly like they used a `ConnectionManager` before.

use redis::aio::ConnectionManager;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
pub struct RedisPool {
    conns: Arc<Vec<ConnectionManager>>,
    next: Arc<AtomicUsize>,
}

impl RedisPool {
    /// Build a pool of `size` independent connections to `url`.
    pub async fn new(
        url: &str,
        size: usize,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let size = size.max(1);
        let client = redis::Client::open(url)
            .map_err(|e| format!("Failed to create Redis client: {}", e))?;

        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = ConnectionManager::new(client.clone())
                .await
                .map_err(|e| format!("Failed to create Redis connection: {}", e))?;
            conns.push(conn);
        }

        Ok(Self {
            conns: Arc::new(conns),
            next: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Build a single-connection pool (test helper).
    pub async fn single(
        url: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::new(url, 1).await
    }

    /// Hand out a connection. Round-robins across the underlying sockets so
    /// concurrent callers get different TCP connections.
    pub fn get(&self) -> ConnectionManager {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        self.conns[idx].clone()
    }

    pub fn size(&self) -> usize {
        self.conns.len()
    }
}

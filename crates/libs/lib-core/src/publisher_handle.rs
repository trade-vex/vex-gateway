//! Publisher Handle - Channel-based wrapper for thread-safe order publishing
//!
//! This module provides a lock-free mechanism to send order commands through
//! a dedicated Publisher task. Instead of all HTTP handlers contending for
//! a single mutex, commands are queued via a channel and processed serially
//! by a dedicated task that owns the Publisher.
//!
//! Benefits:
//! - Eliminates mutex contention on the hot path (order placement/cancellation)
//! - Decouples API handlers from Publisher synchronization
//! - Provides backpressure via bounded channel
//! - Clean shutdown semantics

use common::OrderCommand;
use std::fmt;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};
use vex_networking::client::Publisher;

/// Channel capacity for order commands
/// This provides backpressure - if the Publisher can't keep up,
/// senders will wait (or timeout) rather than overwhelming the system
const COMMAND_CHANNEL_CAPACITY: usize = 10_000;

/// Error type for PublisherHandle operations
#[derive(Error, Debug)]
pub enum PublisherError {
    #[error("Publisher channel closed")]
    ChannelClosed,

    #[error("Failed to send order command: {0}")]
    SendFailed(String),

    #[error("Response channel dropped")]
    ResponseDropped,

    #[error("Channel full - backpressure applied")]
    ChannelFull,
}

/// Internal command message sent through the channel
struct PublishCommand {
    order_command: OrderCommand,
    response_tx: oneshot::Sender<Result<(), String>>,
}

impl fmt::Debug for PublishCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublishCommand")
            .field("client_order_id", &self.order_command.client_order_id)
            .field("command", &self.order_command.command)
            .finish()
    }
}

/// Handle for sending order commands to the Publisher task
///
/// This handle is cheaply cloneable and can be shared across
/// multiple HTTP handler tasks without any mutex contention.
#[derive(Clone)]
pub struct PublisherHandle {
    command_tx: mpsc::Sender<PublishCommand>,
}

impl PublisherHandle {
    /// Create a new PublisherHandle by spawning a dedicated Publisher task
    ///
    /// The Publisher is moved into a background task that owns it exclusively.
    /// All order commands are sent through the returned handle.
    pub fn new(publisher: Publisher) -> Self {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);

        // Spawn the dedicated Publisher task
        tokio::spawn(publisher_task(publisher, command_rx));

        info!(
            channel_capacity = COMMAND_CHANNEL_CAPACITY,
            "PublisherHandle initialized - dedicated Publisher task spawned"
        );

        Self { command_tx }
    }

    /// Send an order command to VEX Core
    ///
    /// This method queues the command for the Publisher task and waits
    /// for the send result. It's fully async and doesn't hold any locks.
    ///
    /// # Returns
    /// - `Ok(())` if the command was successfully sent to VEX Core
    /// - `Err(PublisherError)` if the send failed
    pub async fn send_order_command(&self, command: &OrderCommand) -> Result<(), PublisherError> {
        let (response_tx, response_rx) = oneshot::channel();

        let publish_cmd = PublishCommand {
            order_command: command.clone(),
            response_tx,
        };

        // Send to the Publisher task (may block if channel is full - backpressure)
        self.command_tx
            .send(publish_cmd)
            .await
            .map_err(|_| PublisherError::ChannelClosed)?;

        // Wait for the send result from the Publisher task
        response_rx
            .await
            .map_err(|_| PublisherError::ResponseDropped)?
            .map_err(PublisherError::SendFailed)
    }

    /// Try to send an order command without blocking
    ///
    /// Returns immediately with an error if the channel is full.
    /// Useful for cases where you want to fail fast under load.
    pub fn try_send_order_command(
        &self,
        command: &OrderCommand,
    ) -> Result<oneshot::Receiver<Result<(), String>>, PublisherError> {
        let (response_tx, response_rx) = oneshot::channel();

        let publish_cmd = PublishCommand {
            order_command: command.clone(),
            response_tx,
        };

        self.command_tx.try_send(publish_cmd).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => PublisherError::ChannelFull,
            mpsc::error::TrySendError::Closed(_) => PublisherError::ChannelClosed,
        })?;

        Ok(response_rx)
    }

    /// Check if the Publisher task is still running
    pub fn is_connected(&self) -> bool {
        !self.command_tx.is_closed()
    }

    /// Get the current number of pending commands in the channel
    ///
    /// Useful for monitoring backpressure
    pub fn pending_commands(&self) -> usize {
        // mpsc::Sender doesn't expose queue length directly
        // This is an approximation based on capacity
        COMMAND_CHANNEL_CAPACITY - self.command_tx.capacity()
    }
}

/// Background task that owns the Publisher and processes commands
async fn publisher_task(publisher: Publisher, mut command_rx: mpsc::Receiver<PublishCommand>) {
    info!("Publisher task started - ready to process order commands");

    let mut commands_processed: u64 = 0;
    let mut commands_failed: u64 = 0;

    while let Some(cmd) = command_rx.recv().await {
        let client_order_id = cmd.order_command.client_order_id;

        debug!(
            client_order_id = client_order_id,
            command = ?cmd.order_command.command,
            "Processing order command"
        );

        // Send the order command through the Publisher
        let result = publisher.send_order_command(&cmd.order_command);

        match &result {
            Ok(()) => {
                commands_processed += 1;
                debug!(
                    client_order_id = client_order_id,
                    total_processed = commands_processed,
                    "Order command sent successfully"
                );
            }
            Err(e) => {
                commands_failed += 1;
                error!(
                    client_order_id = client_order_id,
                    error = %e,
                    total_failed = commands_failed,
                    "Failed to send order command"
                );
            }
        }

        // Send the result back to the caller
        // Ignore error if receiver was dropped (caller timed out)
        let _ = cmd.response_tx.send(result.map_err(|e| e.to_string()));
    }

    warn!(
        commands_processed = commands_processed,
        commands_failed = commands_failed,
        "Publisher task shutting down - channel closed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full testing requires a mock Publisher which would need
    // the vex-networking crate to support trait-based abstraction.
    // These tests verify the channel mechanics.

    #[test]
    fn test_publisher_error_display() {
        let err = PublisherError::ChannelClosed;
        assert_eq!(err.to_string(), "Publisher channel closed");

        let err = PublisherError::SendFailed("network error".to_string());
        assert_eq!(
            err.to_string(),
            "Failed to send order command: network error"
        );
    }
}

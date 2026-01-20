//! Aeron handler that routes responses to correlation service

use lib_core::CorrelationService;
use rusteron_archive::{AeronFragmentHandlerCallback, AeronHeader};
use tracing::{debug, error};

/// Aeron handler that routes responses to correlation service
pub struct CorrelationHandler {
    gateway_id: u8,
    correlation_service: CorrelationService,
}

impl CorrelationHandler {
    pub fn new(gateway_id: u8, correlation_service: CorrelationService) -> Self {
        debug!(gateway_id = gateway_id, "Created CorrelationHandler");
        Self {
            gateway_id,
            correlation_service,
        }
    }
}

impl AeronFragmentHandlerCallback for CorrelationHandler {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        match common::decode_order_command(buffer) {
            Ok(order_command) => {
                debug!(
                    gateway_id = self.gateway_id,
                    client_order_id = order_command.client_order_id,
                    order_id = order_command.order_id,
                    status = ?order_command.status,
                    "Received response from VEX Core"
                );

                self.correlation_service.route_response(order_command);
            }
            Err(e) => {
                error!(
                    gateway_id = self.gateway_id,
                    error = ?e,
                    buffer_len = buffer.len(),
                    "Failed to decode response from VEX Core"
                );
            }
        }
    }
}

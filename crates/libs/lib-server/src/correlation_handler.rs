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
        // vex-core's decoder rejects a short buffer or a foreign SBE header as an error. The
        // unwind guard stays because a panic must never cross rusteron's `extern "C"` callback.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
        }));

        if result.is_err() {
            error!(
                gateway_id = self.gateway_id,
                buffer_len = buffer.len(),
                "Panic while decoding or routing response from VEX Core"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_fragments_are_dropped_without_routing_response() {
        let correlation_service = CorrelationService::new();
        let mut response = correlation_service.register_request(42);
        let mut handler = CorrelationHandler::new(1, correlation_service.clone());

        for buffer_len in [0, 1, 8, 32, common::ORDERCOMMANDSIZE - 1] {
            let buffer = vec![0; buffer_len];
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handler.handle_aeron_fragment_handler(&buffer, AeronHeader::new_zeroed_on_stack());
            }));

            assert!(
                result.is_ok(),
                "short fragment of length {buffer_len} panicked"
            );
        }

        assert_eq!(correlation_service.pending_count(), 1);
        assert!(matches!(
            response.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn fragment_with_a_foreign_sbe_header_is_dropped_without_routing_response() {
        let correlation_service = CorrelationService::new();
        let mut response = correlation_service.register_request(42);
        let mut handler = CorrelationHandler::new(1, correlation_service.clone());
        let mut buffer = [0u8; common::ORDERCOMMANDSIZE];
        common::encode_order_command(
            &common::OrderCommand {
                client_order_id: 42,
                ..common::OrderCommand::default()
            },
            &mut buffer,
        )
        .unwrap();
        let valid = buffer;
        // SBE header bytes 2..4 hold the template id.
        buffer[2..4].copy_from_slice(&u16::MAX.to_le_bytes());

        handler.handle_aeron_fragment_handler(&buffer, AeronHeader::new_zeroed_on_stack());

        assert_eq!(correlation_service.pending_count(), 1);
        assert!(matches!(
            response.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        // The same bytes with the header intact are routed, so the header alone was the reason.
        handler.handle_aeron_fragment_handler(&valid, AeronHeader::new_zeroed_on_stack());

        assert_eq!(response.try_recv().unwrap().client_order_id, 42);
    }

    #[test]
    fn busy_status_survives_cores_codec_and_reaches_the_waiting_request() {
        let correlation_service = CorrelationService::new();
        let mut response = correlation_service.register_request(42);
        let mut handler = CorrelationHandler::new(1, correlation_service.clone());
        let mut buffer = [0u8; common::ORDERCOMMANDSIZE];
        common::encode_order_command(
            &common::OrderCommand {
                client_order_id: 42,
                status: common::Status::Busy,
                ..common::OrderCommand::default()
            },
            &mut buffer,
        )
        .unwrap();

        handler.handle_aeron_fragment_handler(&buffer, AeronHeader::new_zeroed_on_stack());

        assert_eq!(response.try_recv().unwrap().status, common::Status::Busy);
    }
}

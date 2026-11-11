//! The vex-core client behind the engine link supervisor

use crate::correlation_handler::CorrelationHandler;
use lib_core::{CoreConnector, CorrelationService, EngineLink, EngineLinkSupervisor, Publisher};
use std::panic::AssertUnwindSafe;
use tracing::error;
use vex_config::GatewayNetworkingConfig;
use vex_networking::client::{GatewayError, VexGateway};

/// Connects through a fresh [`VexGateway`] per attempt: one that has shut down cannot start
/// again.
struct VexCoreConnector {
    config: GatewayNetworkingConfig,
    correlation_service: CorrelationService,
}

impl CoreConnector for VexCoreConnector {
    type Publisher = Publisher;
    type Session = VexGateway;

    fn connect(&mut self) -> Result<(VexGateway, Publisher), GatewayError> {
        let mut gateway = VexGateway::new(self.config.clone())?;
        let handler =
            CorrelationHandler::new(gateway.gateway_id(), self.correlation_service.clone());
        let publisher = gateway.start(handler)?;
        Ok((gateway, publisher))
    }

    fn close(&mut self, mut gateway: VexGateway, publisher: Publisher) {
        // The client's handles share reference counts that are not atomic, so they are dropped on
        // this thread only, and only after the response poller, which drops its subscription on
        // its own thread, has stopped.
        if let Err(e) = gateway.shutdown() {
            error!(error = %e, "Failed to stop polling the lost vex-core link");
        }
        drop(publisher);
        drop(gateway);
    }
}

/// Starts supervising the link to vex-core on a thread of its own and returns the link, which is
/// not connected until the first handshake succeeds. The gateway serves without it meanwhile.
pub fn spawn_engine_link(
    config: GatewayNetworkingConfig,
    correlation_service: CorrelationService,
) -> std::io::Result<EngineLink> {
    let link = EngineLink::new();
    let supervised = link.clone();
    let connector = VexCoreConnector {
        config,
        correlation_service: correlation_service.clone(),
    };
    std::thread::Builder::new()
        .name("engine-link".to_string())
        .spawn(move || {
            // The supervisor is built on its thread: the clients it connects through never leave
            // it. It runs until it panics; the gateway would then serve without an engine link
            // for good while looking alive, so the process ends instead.
            let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                EngineLinkSupervisor::new(connector, supervised, correlation_service).run()
            }));
            error!("The engine link supervisor stopped; aborting");
            std::process::abort();
        })?;
    Ok(link)
}

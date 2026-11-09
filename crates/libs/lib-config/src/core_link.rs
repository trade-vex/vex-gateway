//! lib-config/src/core_link.rs

use serde::Deserialize;
use vex_config::{GatewayAuthenticationKey, GatewayNetworkingConfig};

/// The gateway's Aeron link to vex-core: the `[core]` section. `APP__CORE__<KEY>` overrides a key,
/// such as `APP__CORE__CONTROL_PORT` for `core.control_port`.
#[derive(Debug, Clone, Deserialize)]
pub struct CoreLinkConfig {
    /// This gateway's id at the core, below `common::MAX_GATEWAYS` (16) and unique among the
    /// gateways connected to it.
    pub gateway_id: u8,
    /// The handshake credential: a signed 32-bit integer equal to vex-core's
    /// `VEX__GATEWAY_NETWORKING__GATEWAY_AUTHENTICATION_KEY`. Empty only for a core running
    /// without authentication; production requires it.
    pub auth_key: GatewayAuthenticationKey,
    /// vex-core's Aeron address.
    pub address: String,
    /// vex-core's handshake port: its `core_networking.initial_port`.
    pub port: u16,
    /// vex-core's control port: its `core_networking.initial_control_port`.
    pub control_port: u16,
    /// This gateway's Aeron media driver directory.
    pub context_dir: String,
}

impl CoreLinkConfig {
    /// The vex-core client settings: these keys, over vex-core's gateway defaults for the rest
    /// (heartbeat, timeouts, buffer size, and `local_address`, which the client never reads).
    pub fn networking(&self) -> GatewayNetworkingConfig {
        GatewayNetworkingConfig {
            context_dir: self.context_dir.clone(),
            core_address: self.address.clone(),
            core_port: self.port,
            core_control_port: self.control_port,
            gateway_id: self.gateway_id,
            gateway_authentication_key: self.auth_key.clone(),
            ..GatewayNetworkingConfig::development_defaults()
        }
    }
}

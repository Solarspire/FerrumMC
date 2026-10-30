use management::ManagementServerConfig;
use proxy::ProxyConfig;
use query::QueryConfig;
use serde::{Deserialize, Serialize};

use crate::LANBroadcastConfig;

use java::JavaConfig;

/// Authentication configuration.
pub mod auth;

/// Packet compression configuration.
pub mod compression;
/// Java protocol networking configuration.
pub mod java;
/// LAN broadcast discovery configuration.
pub mod lan_broadcast;
/// Management server (MSMP) configuration.
pub mod management;
/// Reverse proxy and BungeeCord/Velocity configuration.
pub mod proxy;
/// GS4 Query protocol configuration.
pub mod query;

/// Packet limiter configuration.
pub mod packet_limiter;
pub use packet_limiter::PacketLimiterConfig;

/// Configuration for server networking features.
///
/// Covers authentication, query, management protocol, proxying, packet compression,
/// and LAN broadcast behaviour.
#[derive(Deserialize, Serialize, Default)]
#[serde(default)]
pub struct NetworkingConfig {
    /// Query protocol settings for server status requests.
    pub query: QueryConfig,
    /// Management server (MSMP) configuration.
    pub management: ManagementServerConfig,
    /// Proxy-related networking settings.
    pub proxy: ProxyConfig,
    /// LAN broadcast settings.
    pub lan_broadcast: LANBroadcastConfig,
    /// Java Edition configuration settings.
    pub java: JavaConfig,
}

#[cfg(test)]
mod tests {
    use super::NetworkingConfig;

    #[test]
    fn ignores_legacy_bedrock_settings() {
        let config: NetworkingConfig = toml::from_str(
            r#"
            [java]
            address = "127.0.0.1:25566"
            [bedrock]
            enabled = true
            view_distance = 0
            [bedrock.nethernet]
            enabled = true
            address = "invalid legacy address"
        "#,
        )
        .unwrap();
        assert_eq!(config.java.address.to_string(), "127.0.0.1:25566");
        assert!(!toml::to_string(&config).unwrap().contains("bedrock"));
    }
}

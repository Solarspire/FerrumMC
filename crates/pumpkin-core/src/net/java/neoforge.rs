//! `FerrumMC`: `NeoForge` configuration negotiation for 26.2 clients when the bridge is enabled.
//!
//! The pinned `NeoForge` classes in the bridge JVM run the channel negotiation and modded
//! configuration tasks and return encoded 26.2 configuration packets. Rust sends them as-is,
//! runs the vanilla registry synchronization where `NeoForge` schedules it and finishes.

use base64::{Engine, prelude::BASE64_STANDARD};
use bytes::Bytes;
use pumpkin_protocol::java::client::config::CFinishConfig;
use pumpkin_util::{text::TextComponent, version::JavaMinecraftVersion};
use serde_json::Value;
use tracing::warn;

use super::pending::PendingConnection;
use crate::server::Server;

/// Bound on encoded packets returned by one configuration step.
const MAX_STEP_PACKETS: usize = 4096;
pub(super) const REQUIRED_CLIENT: &str =
    "This server requires NeoForge 26.2 with the server's mods.";

impl PendingConnection {
    #[must_use]
    pub fn neoforge_eligible(&self) -> bool {
        ferrum_neoforge::world::enabled() && self.version.load() == JavaMinecraftVersion::V_26_2
    }

    /// Runs one configuration operation in the bridge JVM and applies its result.
    pub async fn neoforge_step(&mut self, server: &Server, mut request: Value) {
        request["connection"] = self.id.into();
        let opening = request["op"] == "open";
        let reply =
            tokio::task::spawn_blocking(move || ferrum_neoforge::world::configuration(&request))
                .await;
        let reply = match reply {
            Ok(Ok(reply)) => reply,
            Ok(Err(error)) => {
                warn!("NeoForge configuration failed: {error}");
                self.kick(TextComponent::text("NeoForge configuration failed"))
                    .await;
                return;
            }
            Err(error) => {
                warn!("NeoForge configuration task failed: {error}");
                self.kick(TextComponent::text("NeoForge configuration failed"))
                    .await;
                return;
            }
        };
        if opening {
            self.neoforge = true;
        }
        let packets = reply["packets"].as_array().map_or(&[][..], Vec::as_slice);
        if packets.len() > MAX_STEP_PACKETS {
            self.kick(TextComponent::text("NeoForge configuration failed"))
                .await;
            return;
        }
        for packet in packets {
            let Some(Ok(bytes)) = packet.as_str().map(|p| BASE64_STANDARD.decode(p)) else {
                warn!("NeoForge returned a malformed configuration packet");
                self.kick(TextComponent::text("NeoForge configuration failed"))
                    .await;
                return;
            };
            if let Err(error) = self.network_writer.write_packet(Bytes::from(bytes)).await {
                warn!("Failed to send packet to client {}: {}", self.id, error);
                return;
            }
        }
        let _ = self.network_writer.flush().await;
        if let Some(reason) = reply["disconnect"].as_str() {
            warn!(
                "NeoForge configuration rejected client {}: {reason}",
                self.id
            );
            // The disconnect packet was among the encoded packets above.
            self.neoforge = false;
            self.close();
            return;
        }
        match reply["action"].as_str() {
            Some("vanilla_sync") => self.start_vanilla_configuration(server).await,
            Some("finish") if reply["neoforge"] == true => {
                self.neoforge_ready = true;
                self.send_packet_now(&CFinishConfig).await;
            }
            Some("finish") => self.kick(TextComponent::text(REQUIRED_CLIENT)).await,
            _ => {}
        }
    }

    /// Releases the bridge session once configuration ends.
    pub async fn neoforge_close(&mut self) {
        if !std::mem::take(&mut self.neoforge) {
            return;
        }
        let request = serde_json::json!({ "op": "close", "connection": self.id });
        if let Ok(Err(error)) =
            tokio::task::spawn_blocking(move || ferrum_neoforge::world::configuration(&request))
                .await
        {
            warn!("Failed to close NeoForge configuration session: {error}");
        }
    }
}

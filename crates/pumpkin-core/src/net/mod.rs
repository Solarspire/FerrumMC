use crate::{entity::player::ChatMode, net::java::JavaClient, server::Server};
use arc_swap::ArcSwap;
use bytes::Bytes;
use pumpkin_world::level::SyncChunk;
use std::{
    net::SocketAddr,
    num::NonZero,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use pumpkin_data::translation;
use pumpkin_protocol::{ClientPacket, Property};
use pumpkin_util::{Hand, ProfileAction, text::TextComponent, version::JavaMinecraftVersion};
use serde::{Deserialize, Deserializer};
use sha1::Digest;
use sha2::Sha256;
use tokio::task::JoinHandle;

use thiserror::Error;
use uuid::Uuid;
pub mod authentication;

pub mod chat;
pub mod chunk_sender;
pub use chunk_sender::ChunkSender;
pub mod java;
pub mod lan_broadcast;
pub mod packet_limiter;
pub use packet_limiter::PacketRateLimiter;
pub mod management;
mod proxy;
pub mod query;

#[derive(Deserialize, Debug)]
pub struct GameProfile {
    pub id: Uuid,
    pub name: String,
    #[serde(deserialize_with = "from_vec")]
    pub properties: ArcSwap<Vec<Property>>,
    #[serde(rename = "profileActions")]
    pub profile_actions: Option<Vec<ProfileAction>>,
}

impl Clone for GameProfile {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name.clone(),
            properties: ArcSwap::new(self.properties.load().clone()),
            profile_actions: self.profile_actions.clone(),
        }
    }
}

fn from_vec<'de, D>(deserializer: D) -> Result<ArcSwap<Vec<Property>>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Vec::<Property>::deserialize(deserializer)?;
    Ok(ArcSwap::new(Arc::new(v)))
}

pub fn offline_uuid(username: &str) -> Result<Uuid, uuid::Error> {
    Uuid::from_slice(&Sha256::digest(username)[..16])
}

/// Represents a player's configuration settings.
///
/// This struct contains various options that can be customized by the player, affecting their gameplay experience.
///
/// **Usage:**
///
/// This struct is typically used to store and manage a player's preferences. It can be sent to the server when a player joins or when they change their settings.
#[derive(Clone)]
pub struct PlayerConfig {
    /// The player's preferred language.
    pub locale: String, // 16
    /// The maximum distance at which chunks are rendered.
    pub view_distance: NonZero<u8>,
    /// The player's chat mode settings
    pub chat_mode: ChatMode,
    /// Whether chat colors are enabled.
    pub chat_colors: bool,
    /// The player's skin configuration options.
    pub skin_parts: u8,
    /// The player's dominant hand (left or right).
    pub main_hand: Hand,
    /// Whether text filtering is enabled.
    pub text_filtering: bool,
    /// Whether the player wants to appear in the server list.
    pub server_listing: bool,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            locale: "en_us".to_string(),
            view_distance: NonZero::new(8).unwrap_or(NonZero::<u8>::MIN),
            chat_mode: ChatMode::Enabled,
            chat_colors: true,
            skin_parts: 0x7F,
            main_hand: Hand::Right,
            text_filtering: false,
            server_listing: false,
        }
    }
}

pub enum PacketHandlerResult {
    Stop,
    ReadyToPlay(GameProfile, PlayerConfig),
}

/// Maximum payload bytes that may be queued for a client before it is considered stalled/overflowing and disconnected.
pub const MAX_PENDING_BYTES: usize = 64 * 1024 * 1024; // 64 MB

/// Defensively decrement an atomic pending byte counter without underflowing.
#[inline]
pub fn decrement_pending_bytes(pending_bytes: &AtomicUsize, bytes: usize) {
    let _ = pending_bytes.fetch_update(Ordering::Release, Ordering::Relaxed, |val| {
        Some(val.saturating_sub(bytes))
    });
}

#[repr(C)]
pub enum ClientPlatform {
    Java(JavaClient),
}

impl ClientPlatform {
    pub const fn address(&self) -> SocketAddr {
        {
            let Self::Java(java) = self;
            java.address
        }
    }

    /// This function should only be used where you know that the client is java!
    #[inline]
    #[must_use]
    pub const fn java(&self) -> Option<&JavaClient> {
        {
            let Self::Java(client) = self;
            Some(client)
        }
    }

    #[must_use]
    pub fn closed(&self) -> bool {
        {
            let Self::Java(java) = self;
            java.is_closed()
        }
    }

    pub fn java_version(&self) -> JavaMinecraftVersion {
        {
            let Self::Java(java) = self;
            java.version.load()
        }
    }

    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        {
            let Self::Java(java) = self;
            java.pending_bytes.load(Ordering::Relaxed)
        }
    }

    pub fn try_enqueue_packet_data(&self, packet_data: Bytes) {
        {
            let Self::Java(java) = self;
            java.try_enqueue_packet_data(packet_data);
        }
    }

    /// Like [`Self::try_enqueue_packet_data`] without firing `PacketSentEvent` (Java only), for
    /// packets a plugin injected itself.
    pub fn try_enqueue_packet_data_unobserved(&self, packet_data: Bytes) {
        {
            let Self::Java(java) = self;
            java.try_enqueue_packet_data_unobserved(packet_data);
        }
    }

    pub async fn await_close_interrupt(&self) {
        {
            let Self::Java(java) = self;
            java.await_close_interrupt().await;
        }
    }

    pub fn spawn_task<F>(&self, task: F) -> Option<JoinHandle<F::Output>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        {
            let Self::Java(java) = self;
            java.spawn_task(task)
        }
    }

    pub async fn enqueue_packet_editioned<J: ClientPacket>(&self, je_packet: &J) {
        {
            let Self::Java(java) = self;
            {
                if let Ok(data) = java.serialize_packet(je_packet) {
                    java.enqueue_packet(data).await;
                }
            }
        }
    }

    pub fn try_enqueue_packet_editioned<J: ClientPacket>(&self, je_packet: &J) {
        {
            let Self::Java(java) = self;
            {
                if let Ok(data) = java.serialize_packet(je_packet) {
                    java.try_enqueue_packet(data);
                }
            }
        }
    }

    pub async fn enqueue_packet(&self, packet_data: Bytes) {
        {
            let Self::Java(java) = self;
            java.enqueue_packet(packet_data).await;
        }
    }

    pub fn try_enqueue_packet(&self, packet_data: Bytes) {
        {
            let Self::Java(java) = self;
            java.try_enqueue_packet(packet_data);
        }
    }

    pub fn try_enqueue_spawn_packet(&self, entity: &Arc<dyn crate::entity::EntityBase>) {
        self.enqueue_spawn_packet(entity);
    }

    pub fn enqueue_spawn_packet(&self, entity: &Arc<dyn crate::entity::EntityBase>) {
        {
            let Self::Java(java) = self;
            entity.send_java_spawn_packet(java);
        }
    }

    pub async fn send_chunks(&self, chunks: &[SyncChunk]) {
        {
            let Self::Java(java) = self;
            java.send_chunks(chunks).await;
        }
    }

    pub async fn send_packet_now(&self, packet_data: Bytes) {
        {
            let Self::Java(java) = self;
            java.send_packet_now(packet_data).await;
        }
    }

    pub async fn send_packet_now_editioned<J: ClientPacket>(&self, je_packet: &J) {
        {
            let Self::Java(java) = self;
            {
                if let Ok(data) = java.serialize_packet(je_packet) {
                    java.send_packet_now(data).await;
                }
            }
        }
    }

    pub async fn send_packet_now_data(&self, data: Bytes) {
        self.send_packet_now(data).await;
    }

    pub fn try_kick(&self, message: &TextComponent) {
        {
            let Self::Java(java) = self;
            java.try_kick(message);
        }
    }

    pub async fn kick(&self, message: TextComponent) {
        {
            let Self::Java(java) = self;
            java.kick(message).await;
        }
    }
}

pub async fn can_not_join(
    profile: &GameProfile,
    address: &SocketAddr,
    server: &Server,
) -> Option<TextComponent> {
    const FORMAT_DESCRIPTION: &[time::format_description::FormatItem<'static>] = time::macros::format_description!(
        "[year]-[month]-[day] at [hour]:[minute]:[second] [offset_hour sign:mandatory]:[offset_minute]"
    );

    let mut banned_players = server
        .data
        .banned_player_list
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = banned_players.get_entry(profile) {
        let text = TextComponent::translate(
            translation::java::MULTIPLAYER_DISCONNECT_BANNED_REASON,
            [TextComponent::text(entry.reason.clone())],
        );
        return Some(match entry.expires {
            Some(expires) => text.add_child(TextComponent::translate(
                translation::java::MULTIPLAYER_DISCONNECT_BANNED_EXPIRATION,
                [TextComponent::text(
                    expires.format(FORMAT_DESCRIPTION).unwrap_or_default(),
                )],
            )),
            None => text,
        });
    }
    drop(banned_players);

    if server.white_list.load(Ordering::Relaxed) {
        let ops = server
            .data
            .operator_config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let whitelist = server
            .data
            .whitelist_config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if ops.get_entry(&profile.id).is_none() && !whitelist.is_whitelisted(profile) {
            return Some(TextComponent::translate(
                translation::java::MULTIPLAYER_DISCONNECT_NOT_WHITELISTED,
                &[],
            ));
        }
    }

    if let Some(entry) = server
        .data
        .banned_ip_list
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_entry(&address.ip())
    {
        let text = TextComponent::translate(
            translation::java::MULTIPLAYER_DISCONNECT_BANNED_IP_REASON,
            [TextComponent::text(entry.reason.clone())],
        );
        return Some(match entry.expires {
            Some(expires) => text.add_child(TextComponent::translate(
                translation::java::MULTIPLAYER_DISCONNECT_BANNED_IP_EXPIRATION,
                [TextComponent::text(
                    expires.format(FORMAT_DESCRIPTION).unwrap_or_default(),
                )],
            )),
            None => text,
        });
    }

    None
}

#[derive(Error, Debug)]
pub enum EncryptionError {
    #[error("failed to decrypt shared secret")]
    FailedDecrypt,
    #[error("shared secret has the wrong length")]
    SharedWrongLength,
    #[error("encryption is already enabled")]
    AlreadyEncrypted,
    #[error("no encryption request is pending")]
    NoPendingVerifyToken,
    #[error("verify token does not match")]
    VerifyTokenMismatch,
}

fn is_valid_player_name(name: &str) -> bool {
    if name.len() > 16 {
        return false;
    }
    !name.chars().any(|c| c.is_control() || c == ' ')
}

#[cfg(test)]
mod tests {
    use crate::net::is_valid_player_name;

    /// Test case for a standard, valid English name at max length.
    #[test]
    fn valid_max_length_ascii() {
        let name = "player_name_1234"; // 16 characters (16 bytes)
        assert!(
            is_valid_player_name(name),
            "Max length ASCII name should be valid"
        );
    }

    /// Test case for a short, valid ASCII name.
    #[test]
    fn valid_short_ascii() {
        let name = "GamerX";
        assert!(
            is_valid_player_name(name),
            "Short ASCII name should be valid"
        );
    }

    /// Test case for a name containing allowed punctuation (codepoints 33-126).
    #[test]
    fn valid_with_punctuation() {
        let name = "!-@#$%.^&*_+-=";
        assert!(
            is_valid_player_name(name),
            "Name with valid punctuation should be valid"
        );
    }

    /// Test case for allowed high-codepoint Unicode characters (like Chinese/CJK).
    #[test]
    fn valid_unicode_chinese() {
        let name = "玩家一号"; // 4 characters, 12 bytes
        assert!(
            is_valid_player_name(name),
            "Chinese characters should be valid"
        );
    }

    /// Test case for a mix of valid ASCII and Unicode characters.
    #[test]
    fn valid_mixed_chars() {
        let name = "Player_玩家"; // 9 characters
        assert!(
            is_valid_player_name(name),
            "Mixed ASCII and Unicode should be valid"
        );
    }

    /// Test case for a name that exceeds the 16-byte limit (ASCII).
    #[test]
    fn invalid_length_ascii_over() {
        let name = "this_name_is_too_long"; // 21 characters (21 bytes)
        assert!(
            !is_valid_player_name(name),
            "Name over 16 bytes (ASCII) should be invalid"
        );
    }

    /// Test case for a name that exceeds the 16-byte limit (Unicode).
    #[test]
    fn invalid_length_unicode_over() {
        let name = "超长玩家名称哈哈"; // 8 Chinese characters * 3 bytes/char = 24 bytes
        assert!(
            !is_valid_player_name(name),
            "Name over 16 bytes (Unicode) should be invalid by byte count"
        );
    }

    /// Test case for a name containing a standard space (codepoint 32).
    #[test]
    fn invalid_contains_space() {
        let name = "Player Name";
        assert!(
            !is_valid_player_name(name),
            "Name containing a space should be invalid"
        );
    }

    /// Test case for an empty string (length 0, but included for completeness).
    #[test]
    fn invalid_empty_string() {
        let name = "";
        assert!(
            is_valid_player_name(name),
            "Empty string should be valid (length <= 16 and no invalid chars)"
        );
    }

    /// Test case for a name containing a control character (e.g., Null, codepoint 0).
    #[test]
    fn invalid_contains_null() {
        let name = "Player\0Name";
        assert!(
            !is_valid_player_name(name),
            "Name containing a null character should be invalid"
        );
    }

    /// Test case for a name containing a newline character (codepoint 10).
    #[test]
    fn invalid_contains_newline() {
        let name = "Player\nName";
        assert!(
            !is_valid_player_name(name),
            "Name containing a newline should be invalid"
        );
    }

    /// Test case for a name containing the DEL control character (codepoint 127).
    #[test]
    fn invalid_contains_del() {
        // DEL character is char::from_u32(127).unwrap()
        let name = format!("Player{}Name", 127u8 as char);
        assert!(
            !is_valid_player_name(&name),
            "Name containing DEL (127) should be invalid"
        );
    }

    #[test]
    fn decrement_pending_bytes_saturating() {
        use super::decrement_pending_bytes;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let counter = AtomicUsize::new(100);
        decrement_pending_bytes(&counter, 40);
        assert_eq!(counter.load(Ordering::Relaxed), 60);

        // Underflow protection: decrementing more than current value should clamp to 0
        decrement_pending_bytes(&counter, 100);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
}

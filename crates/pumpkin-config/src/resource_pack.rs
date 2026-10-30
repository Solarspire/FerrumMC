use serde::{Deserialize, Serialize};

/// Server resource pack settings for Java clients.
#[derive(Deserialize, Serialize, Default)]
#[serde(default)]
pub struct ResourcePackConfig {
    /// Java Edition client resource pack configuration.
    pub java: JavaResourcePackConfig,
}

/// Java-specific resource pack configuration (Single URL/Hash)
#[derive(Deserialize, Serialize, Default)]
#[serde(default)]
pub struct JavaResourcePackConfig {
    /// Whether the resource pack system is enabled.
    pub enabled: bool,
    /// The URL to the resource pack.
    pub url: String,
    /// The SHA1 hash (40 characters) of the resource pack.
    pub sha1: String,
    /// Custom prompt text component shown to players; leave blank for none.
    pub prompt_message: String,
    /// Whether players are forced to accept the resource pack.
    pub force: bool,
}

use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_data::tracked_data::{self, TrackedData};
use pumpkin_protocol::VarInt;

use pumpkin_protocol::java::client::play::{CSetEntityMetadata, Metadata};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::entity::Entity;
use crate::entity::passive::villager::VillagerData;

/// A single typed Java metadata property ready for binary serialization.
#[derive(Clone)]
pub enum JavaMetadataEntry {
    Bool(TrackedData, bool),
    Byte(TrackedData, i8),
    VarInt(TrackedData, i32),
    Int(TrackedData, i32),
    Float(TrackedData, f32),
    VillagerData(TrackedData, VillagerData),
}

impl JavaMetadataEntry {
    pub fn write(&self, writer: &mut Vec<u8>, version: &JavaMinecraftVersion) {
        match self {
            Self::Bool(tracked, val) => {
                let _ = Metadata::new(*tracked, *val).write(writer, version);
            }
            Self::Byte(tracked, val) => {
                let _ = Metadata::new(*tracked, *val).write(writer, version);
            }
            Self::VarInt(tracked, val) => {
                let _ = Metadata::new(*tracked, VarInt(*val)).write(writer, version);
            }
            Self::Int(tracked, val) => {
                let _ = Metadata::new(*tracked, *val).write(writer, version);
            }
            Self::Float(tracked, val) => {
                let _ = Metadata::new(*tracked, *val).write(writer, version);
            }
            Self::VillagerData(tracked, val) => {
                let _ = Metadata::new(*tracked, *val).write(writer, version);
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct MobMetadata {
    java_entries: Vec<JavaMetadataEntry>,
}

impl MobMetadata {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn baby(mut self, is_baby: bool) -> Self {
        if is_baby {
            self.java_entries.push(JavaMetadataEntry::Bool(
                tracked_data::ageable_mob::DATA_BABY_ID,
                true,
            ));
        }
        self
    }

    /// Sets whether the mob has AI disabled.
    #[must_use]
    pub fn no_ai(mut self, no_ai: bool) -> Self {
        if no_ai {
            self.java_entries.push(JavaMetadataEntry::Byte(
                tracked_data::mob::DATA_MOB_FLAGS,
                1,
            ));
        }
        self
    }

    /// Sets whether the mob is tamed (wolves, cats, parrots).
    #[must_use]
    pub fn tamed(mut self, tamed: bool) -> Self {
        if tamed {
            self.java_entries.push(JavaMetadataEntry::Byte(
                tracked_data::tamable_animal::DATA_FLAGS_ID,
                4,
            ));
        }
        self
    }

    /// Sets whether the mob is interested / begging (wolves).
    #[must_use]
    pub fn interested(mut self, interested: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Bool(
            tracked_data::wolf::INTERESTED_ID,
            interested,
        ));
        self
    }

    /// Sets the collar color for tamed mobs (wolves, cats). Each species keeps it in its own
    /// `tracked` slot.
    #[must_use]
    pub fn collar_color(mut self, tracked: TrackedData, color: DyeColor) -> Self {
        self.java_entries
            .push(JavaMetadataEntry::VarInt(tracked, color.id() as i32));
        self
    }

    /// Sets sheep wool color and sheared state.
    #[must_use]
    pub fn sheep_wool(mut self, color: DyeColor, is_sheared: bool) -> Self {
        let packed_byte = (if is_sheared { 0x10 } else { 0 }) | (color.id() & 0x0F);
        self.java_entries.push(JavaMetadataEntry::Byte(
            tracked_data::sheep::WOOL_ID,
            packed_byte as i8,
        ));
        self
    }

    /// Sets creeper charged/powered state.
    #[must_use]
    pub fn charged(mut self, charged: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Bool(
            tracked_data::creeper::CHARGED,
            charged,
        ));
        self
    }

    /// Sets creeper ignited state.
    #[must_use]
    pub fn ignited(mut self, ignited: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Bool(
            tracked_data::creeper::IS_IGNITED,
            ignited,
        ));
        self
    }

    /// Sets creeper fuse speed (-1 idle, 1 counting down).
    #[must_use]
    pub fn fuse_speed(mut self, fuse_speed: i32) -> Self {
        self.java_entries.push(JavaMetadataEntry::VarInt(
            tracked_data::creeper::DATA_SWELL_DIR,
            fuse_speed,
        ));
        self
    }

    /// Sets slime / magma cube size.
    #[must_use]
    pub fn slime_size(mut self, size: i32) -> Self {
        self.java_entries
            .push(JavaMetadataEntry::Int(tracked_data::slime::ID_SIZE, size));
        self
    }

    /// Sets villager data using villager tracked data.
    #[must_use]
    pub fn villager_data(self, data: VillagerData) -> Self {
        self.villager_data_with_tracked(tracked_data::villager::DATA_VILLAGER_DATA, data)
    }

    /// Sets zombie villager data using zombie villager tracked data.
    #[must_use]
    pub fn zombie_villager_data(self, data: VillagerData) -> Self {
        self.villager_data_with_tracked(tracked_data::zombie_villager::DATA_VILLAGER_DATA, data)
    }

    /// Sets villager / zombie villager data with specified tracked data key.
    #[must_use]
    pub fn villager_data_with_tracked(mut self, tracked: TrackedData, data: VillagerData) -> Self {
        self.java_entries
            .push(JavaMetadataEntry::VillagerData(tracked, data));
        self
    }

    /// Sets whether a zombie villager is converting.
    #[must_use]
    pub fn converting(mut self, converting: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Bool(
            tracked_data::zombie_villager::DATA_CONVERTING_ID,
            converting,
        ));
        self
    }

    /// Sets enderman screaming state.
    #[must_use]
    pub fn screaming(mut self, screaming: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Bool(
            tracked_data::enderman::DATA_CREEPY,
            screaming,
        ));
        self
    }

    /// Sets bat roosting state.
    #[must_use]
    pub fn bat_roosting(mut self, roosting: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Byte(
            tracked_data::bat::DATA_ID_FLAGS,
            i8::from(roosting),
        ));
        self
    }

    /// Sets blaze on fire / charged state.
    #[must_use]
    pub fn blaze_on_fire(mut self, on_fire: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Byte(
            tracked_data::blaze::DATA_FLAGS_ID,
            i8::from(on_fire),
        ));
        self
    }

    /// Sets spider climbing state.
    #[must_use]
    pub fn spider_climbing(mut self, climbing: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Byte(
            tracked_data::spider::DATA_FLAGS_ID,
            i8::from(climbing),
        ));
        self
    }

    /// Sets chested status (donkeys, mules, llamas).
    #[must_use]
    pub fn chested(mut self, chested: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Bool(
            tracked_data::trader_llama::DATA_ID_CHEST,
            chested,
        ));
        self
    }

    /// Sets shivering / shaking status (striders out of lava, skeletons in powder snow).
    #[must_use]
    pub fn shivering(mut self, shivering: bool) -> Self {
        self.java_entries.push(JavaMetadataEntry::Bool(
            tracked_data::strider::SUFFOCATING,
            shivering,
        ));
        self
    }

    /// Raw Java metadata entry helper.
    #[must_use]
    pub fn with_java_entry(mut self, entry: JavaMetadataEntry) -> Self {
        self.java_entries.push(entry);
        self
    }

    /// Serializes all Java metadata entries for the target Java version, ending with 255.
    #[must_use]
    pub fn to_java_spawn_metadata(&self, version: JavaMinecraftVersion) -> Option<Box<[u8]>> {
        if version < JavaMinecraftVersion::V_1_9 || self.java_entries.is_empty() {
            return None;
        }
        let mut buf = Vec::new();
        for entry in &self.java_entries {
            entry.write(&mut buf, &version);
        }
        buf.push(255);
        Some(buf.into_boxed_slice())
    }

    pub fn send_to_viewers(&self, entity: &Entity) {
        if let Some(java_bytes) = self.to_java_spawn_metadata(CURRENT_MC_VERSION) {
            let meta_packet = CSetEntityMetadata::new(entity.entity_id.into(), java_bytes);
            entity
                .world
                .load()
                .broadcast_to_chunk(entity.chunk_pos.load(), &meta_packet);
        }
    }
}

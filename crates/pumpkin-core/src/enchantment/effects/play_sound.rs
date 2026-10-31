use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::float_provider::FloatProvider;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomGenerator, get_seed, xoroshiro128::Xoroshiro};

use super::EnchantmentEntityEffectExt;
use crate::entity::Entity;
use crate::entity::player::Player;
use crate::world::World;

/// Vanilla `PlaySoundEffect`: the sound list is indexed by enchantment level.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaySound {
    pub sounds: &'static [&'static str],
    pub volume: FloatProvider,
    pub pitch: FloatProvider,
}

impl PlaySound {
    #[must_use]
    pub const fn new(
        sounds: &'static [&'static str],
        volume: FloatProvider,
        pitch: FloatProvider,
    ) -> Self {
        Self {
            sounds,
            volume,
            pitch,
        }
    }
}

impl EnchantmentEntityEffectExt for PlaySound {
    fn apply(
        &self,
        world: &Arc<World>,
        enchantment_level: i32,
        _owner: Option<&Arc<Player>>,
        entity: Option<&Entity>,
        position: Vector3<f64>,
    ) {
        if entity.is_some_and(Entity::is_silent) || self.sounds.is_empty() {
            return;
        }
        let index = (enchantment_level - 1).clamp(0, self.sounds.len() as i32 - 1) as usize;
        let name = self.sounds[index];
        let Some(sound) = Sound::from_name(name.strip_prefix("minecraft:").unwrap_or(name)) else {
            return;
        };
        let category = if entity.is_some_and(|e| e.entity_type == &EntityType::PLAYER) {
            SoundCategory::Players
        } else {
            SoundCategory::Neutral
        };
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(get_seed()));
        let volume = self.volume.get(&mut random);
        let pitch = self.pitch.get(&mut random);
        world.play_sound_fine(sound, category, &position, volume, pitch);
    }
}

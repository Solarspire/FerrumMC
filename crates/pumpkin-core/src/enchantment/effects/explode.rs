use std::sync::Arc;

use pumpkin_data::damage::DamageType;
use pumpkin_data::enchantment::LevelBasedValue;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{RegistryKey, Tag, get_latest_map};
use pumpkin_util::math::vector3::Vector3;

use crate::world::explosion::{
    Explosion, ExplosionEffects, ExplosionInteraction, ExplosionSource,
    SimpleExplosionDamageCalculator,
};

/// Vanilla `ExplodeEffect`: an explosion at the affected entity (Wind Burst).
#[derive(Clone, Debug, PartialEq)]
pub struct ExplodeEffect {
    pub attribute_to_user: bool,
    pub damage_type: Option<&'static DamageType>,
    pub knockback_multiplier: Option<LevelBasedValue>,
    pub immune_blocks: Option<&'static str>,
    pub offset: Vector3<f64>,
    pub radius: LevelBasedValue,
    pub create_fire: bool,
    pub block_interaction: ExplosionInteraction,
    pub small_particle: Option<Particle>,
    pub large_particle: Option<Particle>,
    pub sound: Option<Sound>,
}

impl ExplodeEffect {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        attribute_to_user: bool,
        damage_type: Option<&'static DamageType>,
        knockback_multiplier: Option<LevelBasedValue>,
        immune_blocks: Option<&'static str>,
        offset: Vector3<f64>,
        radius: LevelBasedValue,
        create_fire: bool,
        block_interaction: ExplosionInteraction,
        small_particle: Option<Particle>,
        large_particle: Option<Particle>,
        sound: Option<Sound>,
    ) -> Self {
        Self {
            attribute_to_user,
            damage_type,
            knockback_multiplier,
            immune_blocks,
            offset,
            radius,
            create_fire,
            block_interaction,
            small_particle,
            large_particle,
            sound,
        }
    }

    /// Parses the codec's `block_interaction` name (`Level.ExplosionInteraction`).
    #[must_use]
    pub fn parse_interaction(name: &str) -> ExplosionInteraction {
        match name {
            "block" => ExplosionInteraction::Block,
            "mob" => ExplosionInteraction::Mob,
            "tnt" => ExplosionInteraction::Tnt,
            "trigger" => ExplosionInteraction::Trigger,
            _ => ExplosionInteraction::None,
        }
    }

    #[must_use]
    pub fn calculate_radius(&self, level: i32) -> f32 {
        self.radius.calculate(level).max(0.0)
    }

    #[must_use]
    pub fn calculate_knockback(&self, level: i32) -> Option<f32> {
        self.knockback_multiplier
            .as_ref()
            .map(|kb| kb.calculate(level))
    }

    fn immune_tag(&self) -> Option<&'static Tag> {
        let name = self.immune_blocks?;
        let name = name.strip_prefix('#').unwrap_or(name);
        get_latest_map(RegistryKey::Block).get(name).copied()
    }
}

impl super::EnchantmentEntityEffectExt for ExplodeEffect {
    fn apply(
        &self,
        world: &Arc<crate::world::World>,
        enchantment_level: i32,
        _owner: Option<&Arc<crate::entity::player::Player>>,
        entity: Option<&crate::entity::Entity>,
        position: Vector3<f64>,
    ) {
        let center = position + self.offset;
        let calculator = SimpleExplosionDamageCalculator::new(
            self.block_interaction != ExplosionInteraction::None,
            self.damage_type.is_some(),
            self.calculate_knockback(enchantment_level),
            self.immune_tag(),
        );
        // Without `attribute_to_user` the explosion has no source, so it also hits the user.
        let user = entity.filter(|_| self.attribute_to_user);
        let source = ExplosionSource {
            entity_id: user.map(|e| e.entity_id),
            entity_type: user.map(|e| e.entity_type),
            damage_type: self.damage_type.copied().unwrap_or(DamageType::EXPLOSION),
            cause_id: user.map(|e| e.entity_id),
        };
        let defaults = ExplosionEffects::DEFAULT;
        let effects = ExplosionEffects {
            small_particle: self.small_particle.unwrap_or(defaults.small_particle),
            large_particle: self.large_particle.unwrap_or(defaults.large_particle),
            sound: self.sound.unwrap_or(defaults.sound),
        };
        Explosion::new(
            self.calculate_radius(enchantment_level),
            center,
            world.get_block_interaction(self.block_interaction),
        )
        .with_damage_calculator(Arc::new(calculator))
        .with_source(source)
        .explode_with_effects(world, effects);
    }
}

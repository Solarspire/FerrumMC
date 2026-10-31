use pumpkin_data::damage::DamageType;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::effect::MobEffect;
use crate::entity::living::LivingEntity;
use crate::entity::projectile::wind_charge::BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR;
use crate::world::explosion::{Explosion, ExplosionEffects, ExplosionInteraction, ExplosionSource};

pub struct WindChargedMobEffect;

impl MobEffect for WindChargedMobEffect {
    /// Vanilla `WindChargedMobEffect.onMobRemoved` for a killed mob.
    fn on_mob_death(&self, living: &LivingEntity, _amplifier: u8, _damage_type: &DamageType) {
        let world = living.entity.world.load_full();
        let pos = living.entity.pos.load();
        let height = living.entity.height();
        let center = Vector3::new(pos.x, pos.y + f64::from(height) / 2.0, pos.z);
        let gust_strength = rand::random::<f32>().mul_add(2.0, 3.0);

        Explosion::new(
            gust_strength,
            center,
            world.get_block_interaction(ExplosionInteraction::Trigger),
        )
        .with_damage_calculator(BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone())
        .with_source(ExplosionSource {
            entity_id: Some(living.entity.entity_id),
            entity_type: Some(living.entity.entity_type),
            damage_type: DamageType::EXPLOSION,
            cause_id: Some(living.entity.entity_id),
        })
        .explode_with_effects(&world, ExplosionEffects::BREEZE_WIND_CHARGE);
    }
}

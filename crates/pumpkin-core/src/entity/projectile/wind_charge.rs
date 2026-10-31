use pumpkin_data::damage::DamageType;
use pumpkin_data::tag;
use pumpkin_util::math::vector3::Vector3;
use std::sync::LazyLock;
use std::{
    f64,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use crate::{
    entity::{
        Entity, EntityBase,
        living::LivingEntity,
        projectile::{ProjectileHit, ThrownItemEntity},
        projectile_deflection::ProjectileDeflectionType,
    },
    server::Server,
    world::{
        Explosion, ExplosionInteraction, SimpleExplosionDamageCalculator,
        explosion::{ExplosionEffects, ExplosionSource},
    },
};

const DEFAULT_DEFLECT_COOLDOWN: u8 = 5;
pub const WIND_CHARGE_GRAVITY: f64 = 0.0;
/// `AbstractWindCharge.getInertia` and `getLiquidInertia`.
const WIND_CHARGE_INERTIA: f64 = 1.0;
/// `WindCharge.RADIUS`.
const WIND_CHARGE_RADIUS: f32 = 1.2;
/// `BreezeWindCharge.RADIUS`.
const BREEZE_WIND_CHARGE_RADIUS: f32 = 3.0;
/// A block hit explodes this far off the hit face (`AbstractWindCharge.onHitBlock`).
const BLOCK_HIT_OFFSET: f64 = 0.25;
/// Charges this far above the build limit burst on their own (`AbstractWindCharge.tick`).
const MAX_HEIGHT_ABOVE_WORLD: i32 = 30;
/// Damage of a direct hit (`AbstractWindCharge.onHitEntity`).
const DIRECT_HIT_DAMAGE: f32 = 1.0;

enum WindChargeKind {
    Normal { deflect_cooldown: AtomicU8 },
    Breeze,
}

pub struct WindChargeEntity {
    kind: WindChargeKind,
    thrown_item_entity: ThrownItemEntity,
}

pub static WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR: LazyLock<Arc<SimpleExplosionDamageCalculator>> =
    LazyLock::new(|| {
        Arc::new(SimpleExplosionDamageCalculator::new(
            true,
            false,
            Some(1.22),
            Some(&tag::Block::MINECRAFT_BLOCKS_WIND_CHARGE_EXPLOSIONS),
        ))
    });

pub static BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR: LazyLock<
    Arc<SimpleExplosionDamageCalculator>,
> = LazyLock::new(|| {
    Arc::new(SimpleExplosionDamageCalculator::new(
        true,
        false,
        None,
        Some(&tag::Block::MINECRAFT_BLOCKS_WIND_CHARGE_EXPLOSIONS),
    ))
});

impl WindChargeEntity {
    #[must_use]
    pub const fn new_normal(thrown_item_entity: ThrownItemEntity) -> Self {
        Self {
            kind: WindChargeKind::Normal {
                deflect_cooldown: AtomicU8::new(DEFAULT_DEFLECT_COOLDOWN),
            },
            thrown_item_entity,
        }
    }

    #[must_use]
    pub const fn new_breeze(thrown_item_entity: ThrownItemEntity) -> Self {
        Self {
            kind: WindChargeKind::Breeze,
            thrown_item_entity,
        }
    }

    pub const fn deflect_cooldown(&self) -> Option<&AtomicU8> {
        if let WindChargeKind::Normal {
            deflect_cooldown, ..
        } = &self.kind
        {
            Some(deflect_cooldown)
        } else {
            None
        }
    }

    /// `WindCharge.explode` / `BreezeWindCharge.explode`.
    pub fn create_explosion(&self, position: Vector3<f64>) {
        let entity = self.get_entity();
        let (radius, calculator, effects): (_, Arc<SimpleExplosionDamageCalculator>, _) =
            match self.kind {
                WindChargeKind::Normal { .. } => (
                    WIND_CHARGE_RADIUS,
                    WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone(),
                    ExplosionEffects::WIND_CHARGE,
                ),
                WindChargeKind::Breeze => (
                    BREEZE_WIND_CHARGE_RADIUS,
                    BREEZE_WIND_CHARGE_EXPLOSION_DAMAGE_CALCULATOR.clone(),
                    ExplosionEffects::BREEZE_WIND_CHARGE,
                ),
            };
        let world = entity.world.load_full();
        Explosion::new(
            radius,
            position,
            world.get_block_interaction(ExplosionInteraction::Trigger),
        )
        .with_damage_calculator(calculator)
        .with_source(ExplosionSource {
            entity_id: Some(entity.entity_id),
            entity_type: Some(entity.entity_type),
            damage_type: DamageType::EXPLOSION,
            cause_id: self.thrown_item_entity.owner_id,
        })
        .explode_with_effects(&world, effects);
    }

    pub fn deflect(
        &mut self,
        deflection: &ProjectileDeflectionType,
        deflector: Option<&dyn EntityBase>,
    ) -> bool {
        if let Some(cooldown) = self.deflect_cooldown()
            && cooldown.load(Ordering::Relaxed) > 0
        {
            return false;
        }

        deflection.deflect(self, deflector);
        true
    }
}

impl EntityBase for WindChargeEntity {
    fn get_owner_id(&self) -> Option<i32> {
        self.thrown_item_entity.owner_id
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = self.get_entity();
        if entity.block_pos.load().0.y > entity.world.load().get_top_y() + MAX_HEIGHT_ABOVE_WORLD {
            self.create_explosion(entity.pos.load());
            entity.remove();
            return;
        }
        self.thrown_item_entity.process_tick_with_inertia(
            caller,
            WIND_CHARGE_INERTIA,
            WIND_CHARGE_INERTIA,
        );

        if let Some(cooldown) = self.deflect_cooldown() {
            let cooldown_ticks = cooldown.load(Ordering::Relaxed);
            if cooldown_ticks > 0 {
                cooldown.store(cooldown_ticks - 1, Ordering::Relaxed);
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.thrown_item_entity.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        match hit {
            ProjectileHit::Entity { entity, .. } => {
                let world = self.get_entity().world.load();
                let owner = self.thrown_item_entity.owner_id.and_then(|id| {
                    world.get_player_by_id(id).map_or_else(
                        || world.get_entity_by_id(id),
                        |player| Some(player as Arc<dyn EntityBase>),
                    )
                });
                let hurt = entity.damage_with_context(
                    entity.as_ref(),
                    DIRECT_HIT_DAMAGE,
                    DamageType::WIND_CHARGE,
                    None,
                    Some(self),
                    owner.as_deref(),
                );
                // `EnchantmentHelper.doPostAttackEffects` runs the owner's weapon effects.
                if hurt
                    && entity.get_living_entity().is_some()
                    && let Some(owner) = owner.as_ref()
                    && let Some(owner_living) = owner.get_living_entity()
                {
                    crate::enchantment::EnchantmentHelper::on_post_attack(
                        owner.get_entity(),
                        entity.get_entity(),
                        &owner_living.held_item(owner.as_ref()),
                    );
                }
                // Vanilla bursts at the charge itself, not at the hit point.
                self.create_explosion(self.get_entity().pos.load());
            }
            ProjectileHit::Block { face, hit_pos, .. } => {
                let normal = face.to_offset().to_f64();
                self.create_explosion(hit_pos + normal * BLOCK_HIT_OFFSET);
            }
        }
    }
}

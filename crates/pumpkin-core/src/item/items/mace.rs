use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::enchantment::EnchantmentHelper;
use crate::entity::decoration::armor_stand::ArmorStandEntity;
use crate::entity::living::LivingEntity;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::world::World;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::world::WorldEvent;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CParticle;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

pub struct MaceItem;

impl ItemMetadata for MaceItem {
    fn ids() -> Box<[u16]> {
        [Item::MACE.id].into()
    }
}
impl ItemBehaviour for MaceItem {
    fn can_mine(&self, player: &Player) -> bool {
        player.gamemode.load() != GameMode::Creative
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl MaceItem {
    pub const SMASH_ATTACK_FALL_THRESHOLD: f64 = 1.5;
    const SMASH_ATTACK_HEAVY_THRESHOLD: f64 = 5.0;
    pub const SMASH_ATTACK_KNOCKBACK_RADIUS: f64 = 3.5;
    const SMASH_ATTACK_KNOCKBACK_POWER: f32 = 0.7;
    /// Vertical speed a smash leaves the attacker with (`hurtEnemy`).
    const SMASH_ATTACKER_Y_VELOCITY: f32 = 0.01;
    /// `levelEvent(2013, .., 750)`: the smash particle burst size.
    const SMASH_PARTICLES_DATA: i32 = 750;
    /// `getAttackDamageBonus` fall-height breakpoints.
    const FALL_HEIGHT_THRESHOLD_1: f64 = 3.0;
    const FALL_HEIGHT_THRESHOLD_2: f64 = 8.0;

    /// Whether `stack` is a mace. Only `MaceItem` overrides the smash hooks in vanilla.
    #[must_use]
    pub const fn is_mace(stack: &ItemStack) -> bool {
        stack.item.id == Item::MACE.id
    }

    /// Vanilla `MaceItem.canSmashAttack`.
    #[must_use]
    pub fn can_smash_attack(attacker: &LivingEntity) -> bool {
        f64::from(attacker.fall_distance.load()) > Self::SMASH_ATTACK_FALL_THRESHOLD
            && !attacker.entity.is_fall_flying()
    }

    /// Vanilla `MaceItem.getAttackDamageBonus`, including Density's per-block bonus.
    #[must_use]
    pub fn get_attack_damage_bonus(attacker: &LivingEntity, weapon: &ItemStack) -> f32 {
        if !Self::can_smash_attack(attacker) {
            return 0.0;
        }
        let fall_distance = f64::from(attacker.fall_distance.load());
        let damage = if fall_distance <= Self::FALL_HEIGHT_THRESHOLD_1 {
            4.0 * fall_distance
        } else if fall_distance <= Self::FALL_HEIGHT_THRESHOLD_2 {
            12.0 + 2.0 * (fall_distance - Self::FALL_HEIGHT_THRESHOLD_1)
        } else {
            22.0 + fall_distance - Self::FALL_HEIGHT_THRESHOLD_2
        };
        let per_block = f64::from(EnchantmentHelper::modify_fall_based_damage(weapon, 0.0) as f32);
        (damage + per_block * fall_distance) as f32
    }

    /// Vanilla `MaceItem.hurtEnemy`: stops the fall, plays the smash sound and knocks back
    /// the entities around the target.
    pub fn hurt_enemy(player: &Player, victim: &dyn EntityBase) {
        let living = &player.living_entity;
        if !Self::can_smash_attack(living) {
            return;
        }
        let entity = player.get_entity();
        let world = player.world();
        let velocity = entity.velocity.load();
        entity.velocity.store(Vector3::new(
            velocity.x,
            f64::from(Self::SMASH_ATTACKER_Y_VELOCITY),
            velocity.z,
        ));
        let position = entity.pos.load();
        // `calculateImpactPosition`: keep an earlier impulse's impact point if it is lower.
        let impact_y = living
            .current_impulse_impact_y
            .load()
            .filter(|y| *y <= position.y)
            .unwrap_or(position.y);
        living.set_ignore_fall_damage_from_current_impulse(true, impact_y);
        entity.send_velocity();

        let fall_distance = f64::from(living.fall_distance.load());
        let sound = if victim.get_entity().on_ground.load(Ordering::Relaxed) {
            player
                .spawn_extra_particles_on_fall
                .store(true, Ordering::Relaxed);
            if fall_distance > Self::SMASH_ATTACK_HEAVY_THRESHOLD {
                Sound::ItemMaceSmashGroundHeavy
            } else {
                Sound::ItemMaceSmashGround
            }
        } else {
            Sound::ItemMaceSmashAir
        };
        world.play_sound(sound, SoundCategory::Players, &position);

        Self::knockback(&world, player, victim, fall_distance);
    }

    /// Vanilla `MaceItem.postHurtEnemy`.
    pub fn post_hurt_enemy(player: &Player) {
        if Self::can_smash_attack(&player.living_entity) {
            player.living_entity.fall_distance.store(0.0);
        }
    }

    fn knockback(world: &Arc<World>, player: &Player, victim: &dyn EntityBase, fall_distance: f64) {
        let victim_entity = victim.get_entity();
        world.sync_world_event(
            WorldEvent::ParticlesSmashAttack,
            Self::on_pos(victim_entity),
            Self::SMASH_PARTICLES_DATA,
        );
        let victim_pos = victim_entity.pos.load();
        let radius = Self::SMASH_ATTACK_KNOCKBACK_RADIUS;
        let search_box = victim_entity
            .bounding_box
            .load()
            .expand(radius, radius, radius);
        let heavy_multiplier = if fall_distance > Self::SMASH_ATTACK_HEAVY_THRESHOLD {
            2.0
        } else {
            1.0
        };
        for nearby in world.get_all_at_box(&search_box) {
            let Some(nearby_living) = nearby.get_living_entity() else {
                continue;
            };
            if !Self::knockback_predicate(player, victim, nearby.as_ref()) {
                continue;
            }
            let direction = nearby.get_entity().pos.load() - victim_pos;
            let knockback_power = (radius - direction.length())
                * f64::from(Self::SMASH_ATTACK_KNOCKBACK_POWER)
                * heavy_multiplier
                * (1.0 - nearby_living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE));
            if knockback_power > 0.0 {
                let knockback = direction.normalize() * knockback_power;
                nearby.get_entity().add_velocity(Vector3::new(
                    knockback.x,
                    f64::from(Self::SMASH_ATTACK_KNOCKBACK_POWER),
                    knockback.z,
                ));
            }
        }
    }

    /// Vanilla `MaceItem.knockbackPredicate`.
    fn knockback_predicate(
        player: &Player,
        victim: &dyn EntityBase,
        nearby: &dyn EntityBase,
    ) -> bool {
        let nearby_entity = nearby.get_entity();
        let victim_entity = victim.get_entity();
        let attacker: &dyn EntityBase = player;
        if nearby.is_spectator()
            || nearby_entity.entity_id == player.entity_id()
            || nearby_entity.entity_id == victim_entity.entity_id
            || attacker.is_allied_to(nearby)
        {
            return false;
        }
        // Vanilla checks whether the pet belongs to the *target*, not the attacker.
        let owned_by_target = nearby.get_mob().is_some_and(|mob| {
            victim.get_living_entity().is_some()
                && mob.is_tamed()
                && mob.get_owner_uuid() == Some(victim_entity.entity_uuid)
        });
        let marker_stand = nearby
            .cast_any()
            .downcast_ref::<ArmorStandEntity>()
            .is_some_and(ArmorStandEntity::is_marker);
        let creative_flying = nearby
            .get_player()
            .is_some_and(|p| p.is_creative() && p.is_flying());
        !owned_by_target
            && !marker_stand
            && !creative_flying
            && victim_entity
                .pos
                .load()
                .squared_distance_to_vec(&nearby_entity.pos.load())
                <= Self::SMASH_ATTACK_KNOCKBACK_RADIUS.powi(2)
    }

    /// Vanilla `Entity.getOnPos`: the supporting block, else the block just below the feet.
    fn on_pos(entity: &Entity) -> BlockPos {
        entity.get_supporting_block_pos().unwrap_or_else(|| {
            let pos = entity.pos.load();
            BlockPos::floored(pos.x, pos.y - ON_POS_OFFSET, pos.z)
        })
    }
}

/// `Entity.getOnPos()` offset.
const ON_POS_OFFSET: f64 = 1.0e-5;
/// `Entity.getOnPosLegacy()` offset.
const ON_POS_LEGACY_OFFSET: f64 = 0.2;
/// `ServerPlayer.checkFallDamage`: block particles per fallen block, and their cap.
const EXTRA_FALL_PARTICLES_PER_BLOCK: f64 = 50.0;
const EXTRA_FALL_PARTICLES_MAX: f64 = 200.0;
const EXTRA_FALL_PARTICLES_SPREAD: f32 = 0.3;
const EXTRA_FALL_PARTICLES_SPEED: f32 = 0.15;
/// `ServerLevel.sendParticles` range without `overrideLimiter`.
const PARTICLE_SEND_DISTANCE: f64 = 32.0;

/// The block burst `ServerPlayer.checkFallDamage` spawns when a player lands after a
/// grounded mace smash.
pub fn spawn_extra_fall_particles(player: &Player, fall_distance: f32) {
    let entity = player.get_entity();
    let world = player.world();
    let pos = entity.pos.load();
    // `Entity.getOnPosLegacy`: the supporting block, else the block 0.2 below the feet.
    let on_pos = entity
        .supporting_block_pos
        .load()
        .unwrap_or_else(|| BlockPos::floored(pos.x, pos.y - ON_POS_LEGACY_OFFSET, pos.z));
    let state_id = world.get_block_state_id(&on_pos);
    let center = on_pos.to_centered_f64().add_raw(0.0, 0.5, 0.0);
    let count = (EXTRA_FALL_PARTICLES_PER_BLOCK * f64::from(fall_distance))
        .clamp(0.0, EXTRA_FALL_PARTICLES_MAX) as i32;
    let mut data = Vec::new();
    if VarInt(i32::from(state_id.as_u16()))
        .encode(&mut data)
        .is_err()
    {
        return;
    }
    let spread = EXTRA_FALL_PARTICLES_SPREAD;
    let packet = CParticle::new(
        false,
        false,
        center,
        Vector3::new(spread, spread, spread),
        EXTRA_FALL_PARTICLES_SPEED,
        count,
        VarInt(i32::from(Particle::Block.to_id())),
        &data,
    );
    for viewer in world.players.load().iter() {
        if viewer.position().squared_distance_to_vec(&center)
            <= PARTICLE_SEND_DISTANCE * PARTICLE_SEND_DISTANCE
        {
            viewer.try_send_client_packet(&packet);
        }
    }
}

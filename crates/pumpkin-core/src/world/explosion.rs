use std::sync::Arc;

use pumpkin_data::{
    Block, BlockState, BlockStateId,
    damage::DamageType,
    entity::EntityType,
    fluid::Fluid,
    tag::{Tag, Taggable},
};
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};
use pumpkin_world::chunk::ChunkData;
use rustc_hash::FxHashMap;

use pumpkin_data::{attributes::Attributes, particle::Particle, sound::Sound};
use pumpkin_protocol::{IdOr, SoundEvent, codec::var_int::VarInt, java::client::play::CExplosion};

use crate::{
    block::{ExplodeArgs, drop_loot},
    entity::{Entity, EntityBase, player::Player},
    world::loot::LootContextParameters,
};

use super::{BlockFlags, World};

/// Defines the type of explosion interaction with the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplosionInteraction {
    None,
    Block,
    Mob,
    Tnt,
    Trigger,
}

/// Defines how an explosion interacts with blocks in the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockInteraction {
    /// Keeps blocks intact (no block damage, no drops).
    Keep,
    /// Destroys blocks and drops 100% of items without decay.
    Destroy,
    /// Destroys blocks and applies loot decay based on explosion radius.
    DestroyWithDecay,
    /// Triggers block effects without destroying them.
    TriggerBlock,
}

/// Defines how damage and block destruction are calculated for an explosion.
pub trait ExplosionDamageCalculator: Send + Sync {
    /// Returns the block's explosion resistance. If None, the block is treated as air/empty.
    fn get_block_explosion_resistance(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        if block.default_state.is_air() && fluid.is_empty {
            None
        } else {
            Some(fluid.blast_resistance.max(block.blast_resistance))
        }
    }

    /// Returns whether this block should be destroyed / affected by the explosion.
    fn should_block_explode(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        _block: &Block,
        _power: f32,
    ) -> bool {
        true
    }

    /// Returns whether the entity should take damage from the explosion.
    fn should_damage_entity(&self, _explosion: &Explosion, _entity: &dyn EntityBase) -> bool {
        true
    }

    /// Returns knockback multiplier for the given entity (default 1.0).
    fn get_knockback_multiplier(&self, _entity: &dyn EntityBase) -> f32 {
        1.0
    }

    /// Calculates the damage amount to deal to the entity given the exposure.
    fn get_entity_damage_amount(
        &self,
        explosion: &Explosion,
        entity: &dyn EntityBase,
        exposure: f32,
    ) -> f32 {
        // Vanilla `ExplosionDamageCalculator.getEntityDamageAmount` scales by the doubled radius.
        let double_radius = f64::from(explosion.power * 2.0);
        let distance = (entity
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&explosion.pos))
        .sqrt()
            / double_radius;
        let damage_multiplier = (1.0 - distance) * f64::from(exposure);
        (f64::midpoint(damage_multiplier * damage_multiplier, damage_multiplier)
            * 7.0
            * double_radius
            + 1.0) as f32
    }
}

/// Default explosion damage calculator implementing vanilla standard explosion rules.
pub struct DefaultExplosionDamageCalculator;

impl ExplosionDamageCalculator for DefaultExplosionDamageCalculator {}

/// Vanilla `SimpleExplosionDamageCalculator` (wind charges, the Wind Burst enchantment).
pub struct SimpleExplosionDamageCalculator {
    pub explodes_blocks: bool,
    pub damages_entities: bool,
    pub knockback_multiplier: Option<f32>,
    pub immune_blocks: Option<&'static Tag>,
}

impl SimpleExplosionDamageCalculator {
    /// Resistance vanilla gives blocks in the immune set.
    const IMMUNE_BLOCK_RESISTANCE: f32 = 3_600_000.0;

    #[must_use]
    pub const fn new(
        explodes_blocks: bool,
        damages_entities: bool,
        knockback_multiplier: Option<f32>,
        immune_blocks: Option<&'static Tag>,
    ) -> Self {
        Self {
            explodes_blocks,
            damages_entities,
            knockback_multiplier,
            immune_blocks,
        }
    }
}

impl ExplosionDamageCalculator for SimpleExplosionDamageCalculator {
    fn get_block_explosion_resistance(
        &self,
        explosion: &Explosion,
        world: &World,
        pos: &BlockPos,
        block: &Block,
        fluid: &pumpkin_data::fluid::FluidState,
    ) -> Option<f32> {
        // With an immune set, every other block offers no resistance at all.
        if let Some(immune_tag) = self.immune_blocks {
            return block
                .has_tag(immune_tag)
                .then_some(Self::IMMUNE_BLOCK_RESISTANCE);
        }
        DefaultExplosionDamageCalculator
            .get_block_explosion_resistance(explosion, world, pos, block, fluid)
    }

    fn should_block_explode(
        &self,
        _explosion: &Explosion,
        _world: &World,
        _pos: &BlockPos,
        _block: &Block,
        _power: f32,
    ) -> bool {
        self.explodes_blocks
    }

    fn should_damage_entity(&self, _explosion: &Explosion, _entity: &dyn EntityBase) -> bool {
        self.damages_entities
    }

    fn get_knockback_multiplier(&self, entity: &dyn EntityBase) -> f32 {
        if entity.get_player().is_some_and(Player::is_flying) {
            return 0.0;
        }
        self.knockback_multiplier.unwrap_or(1.0)
    }
}

/// `ServerExplosion.LARGE_EXPLOSION_RADIUS`.
const LARGE_EXPLOSION_RADIUS: f32 = 2.0;
/// Players within this squared distance get the explosion packet (`ServerLevel.explode`).
const EXPLOSION_PACKET_DISTANCE_SQUARED: f64 = 4096.0;

/// The entity behind an explosion: vanilla `ServerExplosion.source` and its damage source.
#[derive(Clone, Copy)]
pub struct ExplosionSource {
    /// Direct source entity; it is excluded from the hit entities.
    pub entity_id: Option<i32>,
    pub entity_type: Option<&'static EntityType>,
    pub damage_type: DamageType,
    /// Causing entity of the damage source.
    pub cause_id: Option<i32>,
}

impl ExplosionSource {
    pub const NONE: Self = Self {
        entity_id: None,
        entity_type: None,
        damage_type: DamageType::EXPLOSION,
        cause_id: None,
    };
}

/// Particles and sound of a `Level.explode` call.
#[derive(Clone, Copy)]
pub struct ExplosionEffects {
    pub small_particle: Particle,
    pub large_particle: Particle,
    pub sound: Sound,
}

impl ExplosionEffects {
    pub const DEFAULT: Self = Self {
        small_particle: Particle::Explosion,
        large_particle: Particle::ExplosionEmitter,
        sound: Sound::EntityGenericExplode,
    };
    /// `WindCharge.explode`.
    pub const WIND_CHARGE: Self = Self {
        small_particle: Particle::GustEmitterSmall,
        large_particle: Particle::GustEmitterLarge,
        sound: Sound::EntityWindChargeWindBurst,
    };
    /// `BreezeWindCharge.explode` and `WindChargedMobEffect`.
    pub const BREEZE_WIND_CHARGE: Self = Self {
        small_particle: Particle::GustEmitterSmall,
        large_particle: Particle::GustEmitterLarge,
        sound: Sound::EntityBreezeWindBurst,
    };
}

pub struct Explosion {
    power: f32,
    pos: Vector3<f64>,
    block_interaction: BlockInteraction,
    damage_calculator: Option<Arc<dyn ExplosionDamageCalculator>>,
    preserve_rails: bool,
    source: ExplosionSource,
}

impl Explosion {
    #[must_use]
    pub const fn new(power: f32, pos: Vector3<f64>, block_interaction: BlockInteraction) -> Self {
        Self {
            power,
            pos,
            block_interaction,
            damage_calculator: None,
            preserve_rails: false,
            source: ExplosionSource::NONE,
        }
    }

    #[must_use]
    pub const fn with_source(mut self, source: ExplosionSource) -> Self {
        self.source = source;
        self
    }

    /// Vanilla `ServerExplosion.canTriggerBlocks`.
    fn can_trigger_blocks(&self, world: &World) -> bool {
        self.block_interaction == BlockInteraction::TriggerBlock
            && (self.source.entity_type != Some(&EntityType::BREEZE_WIND_CHARGE)
                || world.level_info.load().game_rules.mob_griefing)
    }

    /// Vanilla `ServerExplosion.isSmall`.
    #[must_use]
    pub fn is_small(&self) -> bool {
        self.power < LARGE_EXPLOSION_RADIUS || self.block_interaction == BlockInteraction::Keep
    }

    #[must_use]
    pub fn with_damage_calculator(
        mut self,
        calculator: Arc<dyn ExplosionDamageCalculator>,
    ) -> Self {
        self.damage_calculator = Some(calculator);
        self
    }

    #[must_use]
    pub const fn preserving_rails(mut self) -> Self {
        self.preserve_rails = true;
        self
    }

    fn protects_rail(&self, world: &World, pos: &BlockPos, block: &Block) -> bool {
        self.preserve_rails && (Self::is_rail(block) || Self::is_rail(world.get_block(&pos.up())))
    }

    fn is_rail(block: &Block) -> bool {
        block.id == Block::RAIL.id
            || block.id == Block::POWERED_RAIL.id
            || block.id == Block::DETECTOR_RAIL.id
            || block.id == Block::ACTIVATOR_RAIL.id
    }

    #[allow(clippy::too_many_lines)]
    fn get_blocks_to_destroy(
        &self,
        world: &World,
    ) -> FxHashMap<BlockPos, (&'static Block, &'static BlockState)> {
        let mut map = FxHashMap::default();

        let mut chunk_cache: FxHashMap<
            pumpkin_util::math::vector2::Vector2<i32>,
            Option<Arc<ChunkData>>,
        > = FxHashMap::default();

        let default_calc = DefaultExplosionDamageCalculator;
        let calc: &dyn ExplosionDamageCalculator = match &self.damage_calculator {
            Some(c) => c.as_ref(),
            None => &default_calc,
        };

        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    if x > 0 && x < 15 && y > 0 && y < 15 && z > 0 && z < 15 {
                        continue;
                    }

                    let mut dir_x = f64::from(x) / 7.5 - 1.0;
                    let mut dir_y = f64::from(y) / 7.5 - 1.0;
                    let mut dir_z = f64::from(z) / 7.5 - 1.0;

                    let length = (dir_x * dir_x + dir_y * dir_y + dir_z * dir_z).sqrt();
                    dir_x /= length;
                    dir_y /= length;
                    dir_z /= length;

                    let mut pos_x = self.pos.x;
                    let mut pos_y = self.pos.y;
                    let mut pos_z = self.pos.z;

                    let random_val = rand::random::<f32>();
                    let mut h = self.power * random_val.mul_add(0.6, 0.7);

                    while h > 0.0 {
                        let block_pos = BlockPos::floored(pos_x, pos_y, pos_z);

                        if !world.is_in_build_limit(block_pos) {
                            break;
                        }

                        let (chunk_pos, relative) = block_pos.chunk_and_chunk_relative_position();

                        let chunk_opt = chunk_cache.entry(chunk_pos).or_insert_with(|| {
                            world
                                .level
                                .read_chunk_sync(&chunk_pos, std::clone::Clone::clone)
                        });

                        let state_id = if let Some(chunk) = chunk_opt {
                            chunk
                                .section
                                .get_block_absolute_y(
                                    relative.x as usize,
                                    block_pos.0.y,
                                    relative.z as usize,
                                )
                                .unwrap_or(Block::AIR.default_state.id)
                        } else {
                            Block::AIR.default_state.id
                        };

                        let (block, state) = BlockState::from_id_with_block(state_id);

                        let (_fluid, fluid_state) = Fluid::from_state_id(state_id).map_or_else(
                            || {
                                if block.is_waterlogged(state_id) {
                                    (&Fluid::FLOWING_WATER, &Fluid::FLOWING_WATER.states[0])
                                } else {
                                    (&Fluid::EMPTY, &Fluid::EMPTY.states[0])
                                }
                            },
                            |raw_fluid| {
                                let f = raw_fluid.to_flowing();
                                (f, &f.states[0])
                            },
                        );

                        if !state.is_air() || !fluid_state.is_empty {
                            let protects_rail = self.protects_rail(world, &block_pos, block);
                            let resistance = if protects_rail {
                                Some(0.0)
                            } else {
                                calc.get_block_explosion_resistance(
                                    self,
                                    world,
                                    &block_pos,
                                    block,
                                    fluid_state,
                                )
                            };

                            if let Some(resistance) = resistance {
                                h -= (resistance + 0.3) * 0.3;
                            }

                            if h > 0.0
                                && !protects_rail
                                && calc.should_block_explode(self, world, &block_pos, block, h)
                            {
                                map.insert(block_pos, (block, state));
                            }
                        }

                        pos_x += dir_x * 0.3;
                        pos_y += dir_y * 0.3;
                        pos_z += dir_z * 0.3;
                        h -= 0.225_000_01;
                    }
                }
            }
        }
        map
    }

    /// Vanilla `ServerExplosion.hurtEntities`. Returns the knockback each hit player gets
    /// through the explosion packet, keyed by entity id.
    #[allow(clippy::too_many_lines)]
    fn damage_entities(&self, world: &Arc<World>) -> FxHashMap<i32, Vector3<f64>> {
        let mut hit_players = FxHashMap::default();
        if self.power < 1.0e-5 {
            return hit_players;
        }

        let double_radius = f64::from(self.power * 2.0);
        let min_x = (self.pos.x - double_radius - 1.0).floor();
        let max_x = (self.pos.x + double_radius + 1.0).floor();
        let min_y = (self.pos.y - double_radius - 1.0).floor();
        let max_y = (self.pos.y + double_radius + 1.0).floor();
        let min_z = (self.pos.z - double_radius - 1.0).floor();
        let max_z = (self.pos.z + double_radius + 1.0).floor();

        let search_box = BoundingBox::new(
            Vector3::new(min_x, min_y, min_z),
            Vector3::new(max_x, max_y, max_z),
        );

        let entities = world.get_all_at_box(&search_box);

        let default_calc = DefaultExplosionDamageCalculator;
        let calc: &dyn ExplosionDamageCalculator = match &self.damage_calculator {
            Some(c) => c.as_ref(),
            None => &default_calc,
        };
        let source_entity = self
            .source
            .entity_id
            .and_then(|id| world.get_entity_by_id(id));
        let cause_entity = self.source.cause_id.and_then(|id| {
            world
                .get_entity_by_id(id)
                .or_else(|| world.get_player_by_id(id).map(|p| p as Arc<dyn EntityBase>))
        });

        for entity_base in entities {
            let entity = entity_base.get_entity();
            if Some(entity.entity_id) == self.source.entity_id
                || entity_base.is_immune_to_explosion()
                || entity_base.is_spectator()
            {
                continue;
            }

            let distance =
                (entity.pos.load().squared_distance_to_vec(&self.pos)).sqrt() / double_radius;
            if distance > 1.0 {
                continue;
            }

            let origin = if entity.entity_type == &EntityType::TNT {
                entity.pos.load()
            } else {
                entity.get_eye_pos()
            };
            let direction = (origin - self.pos).normalize();
            let should_damage = calc.should_damage_entity(self, entity_base.as_ref());
            let knockback_multiplier = calc.get_knockback_multiplier(entity_base.as_ref());

            let exposure = if !should_damage && knockback_multiplier == 0.0 {
                0.0
            } else {
                Self::calculate_exposure(&self.pos, entity, world)
            };

            if should_damage {
                let damage = calc.get_entity_damage_amount(self, entity_base.as_ref(), exposure);
                entity_base.damage_with_context(
                    entity_base.as_ref(),
                    damage,
                    self.source.damage_type,
                    None,
                    source_entity.as_deref(),
                    cause_entity.as_deref(),
                );
            }

            let knockback_resistance = entity_base.get_living_entity().map_or(0.0, |living| {
                living.get_attribute_value(&Attributes::EXPLOSION_KNOCKBACK_RESISTANCE)
            });
            let knockback_power = (1.0 - distance)
                * f64::from(exposure)
                * f64::from(knockback_multiplier)
                * (1.0 - knockback_resistance);
            let knockback = direction * knockback_power;

            if let Some(player) = entity_base.get_player() {
                // The client owns its movement: the knockback rides on the explosion packet.
                entity.velocity.store(entity.velocity.load() + knockback);
                if !(player.is_creative() && player.is_flying()) {
                    hit_players.insert(entity.entity_id, knockback);
                }
                Self::on_player_explosion_hit(player, self.source.entity_type);
            } else if entity.entity_type != &EntityType::WIND_CHARGE
                && entity.entity_type != &EntityType::BREEZE_WIND_CHARGE
            {
                // `AbstractWindCharge.push` ignores pushes.
                entity.add_velocity(knockback);
            }
        }
        hit_players
    }

    /// Vanilla `ServerPlayer.onExplosionHit`: only a player's own wind charge spares the
    /// fall damage of the launch.
    fn on_player_explosion_hit(player: &Player, source_type: Option<&'static EntityType>) {
        let living = &player.living_entity;
        living.set_ignore_fall_damage_from_current_impulse(
            source_type == Some(&EntityType::WIND_CHARGE),
            living.entity.pos.load().y,
        );
    }

    fn calculate_exposure(
        explosion_pos: &Vector3<f64>,
        entity: &Entity,
        world: &Arc<World>,
    ) -> f32 {
        let bbox = entity.bounding_box.load();

        let step_x = 1.0 / ((bbox.max.x - bbox.min.x) * 2.0 + 1.0);
        let step_y = 1.0 / ((bbox.max.y - bbox.min.y) * 2.0 + 1.0);
        let step_z = 1.0 / ((bbox.max.z - bbox.min.z) * 2.0 + 1.0);

        if step_x < 0.0 || step_y < 0.0 || step_z < 0.0 {
            return 0.0;
        }

        let offset_x = (1.0 - (1.0 / step_x).floor() * step_x) / 2.0;
        let offset_z = (1.0 - (1.0 / step_z).floor() * step_z) / 2.0;

        let mut visible_points = 0;
        let mut total_points = 0;

        let mut k = 0.0;
        while k <= 1.0 {
            let mut l = 0.0;
            while l <= 1.0 {
                let mut m = 0.0;
                while m <= 1.0 {
                    let n = bbox.min.x + (bbox.max.x - bbox.min.x) * k;
                    let o = bbox.min.y + (bbox.max.y - bbox.min.y) * l;
                    let p = bbox.min.z + (bbox.max.z - bbox.min.z) * m;

                    let vec3d = Vector3::new(n + offset_x, o, p + offset_z);

                    if world
                        .clip(&pumpkin_world::clip::ClipContext::new(
                            vec3d,
                            *explosion_pos,
                            pumpkin_world::clip::ClipBlock::Collider,
                            pumpkin_world::clip::ClipFluid::None,
                        ))
                        .is_miss
                    {
                        visible_points += 1;
                    }

                    total_points += 1;
                    m += step_z;
                }
                l += step_y;
            }
            k += step_x;
        }

        if total_points == 0 {
            return 0.0;
        }

        visible_points as f32 / total_points as f32
    }

    /// Returns the removed block count. Player knockback is applied as a velocity update;
    /// [`Explosion::explode_with_effects`] sends it in the explosion packet like vanilla.
    pub fn explode(&self, world: &Arc<World>) -> u32 {
        let hit_players = self.damage_entities(world);
        for id in hit_players.keys() {
            if let Some(player) = world.get_player_by_id(*id) {
                player.get_entity().send_velocity();
            }
        }
        self.interact_with_blocks(world)
    }

    /// Vanilla `ServerLevel.explode` after the explosion is built: fires the plugin event,
    /// runs the explosion and sends each nearby player the explosion packet with its knockback.
    pub fn explode_with_effects(&self, world: &Arc<World>, effects: ExplosionEffects) {
        let mut event = crate::plugin::api::events::entity::entity_explode::EntityExplodeEvent::new(
            self.source.entity_id.unwrap_or(0),
            self.pos,
            self.power,
        );
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled {
            return;
        }

        let hit_players = self.damage_entities(world);
        let block_count = self.interact_with_blocks(world);
        let particle = if self.is_small() {
            effects.small_particle
        } else {
            effects.large_particle
        };
        let sound = IdOr::<SoundEvent>::Id(effects.sound as u16);
        for player in world.players.load().iter() {
            if player.position().squared_distance_to_vec(&self.pos)
                >= EXPLOSION_PACKET_DISTANCE_SQUARED
            {
                continue;
            }
            player.try_send_client_packet(&CExplosion::new(
                self.pos,
                self.power,
                block_count as i32,
                hit_players.get(&player.entity_id()).copied(),
                VarInt(i32::from(particle.to_id())),
                sound.clone(),
            ));
        }
    }

    fn interact_with_blocks(&self, world: &Arc<World>) -> u32 {
        match self.block_interaction {
            BlockInteraction::Keep => 0,
            BlockInteraction::TriggerBlock => {
                // Vanilla still reports every reached position to the client.
                let blocks = self.get_blocks_to_destroy(world);
                if self.can_trigger_blocks(world) {
                    for (pos, (block, state)) in &blocks {
                        crate::block::blocks::wind_trigger::on_explosion_hit(
                            world, block, state, pos,
                        );
                    }
                }
                blocks.len() as u32
            }
            BlockInteraction::Destroy | BlockInteraction::DestroyWithDecay => {
                let center_pos = BlockPos::floored(self.pos.x, self.pos.y, self.pos.z);
                let mut event =
                    crate::plugin::api::events::block::block_explode::BlockExplodeEvent::new(
                        center_pos,
                        if self.power > 0.0 {
                            1.0 / self.power
                        } else {
                            1.0
                        },
                    );
                if let Some(server) = world.server.upgrade() {
                    server.plugin_manager.fire_blocking(&server, &mut event);
                }
                if event.cancelled {
                    return 0;
                }

                let blocks = self.get_blocks_to_destroy(world);
                let decay_drops = self.block_interaction == BlockInteraction::DestroyWithDecay;
                let explosion_radius = decay_drops.then_some(self.power);

                for (pos, (block, state)) in &blocks {
                    let pumpkin_block = world.block_registry.get_pumpkin_block(block.id);

                    if pumpkin_block.is_none_or(|s| s.should_drop_items_on_explosion()) {
                        let is_raining = world.is_raining();
                        let is_thundering = world.is_thundering();
                        let params = LootContextParameters {
                            block_state: Some(state),
                            explosion_radius,
                            position: Some(pumpkin_util::math::vector3::Vector3::new(
                                pos.0.x as f64,
                                pos.0.y as f64,
                                pos.0.z as f64,
                            )),
                            world_time: world.level_info.load().day_time as u64,
                            is_raining: Some(is_raining),
                            is_thundering: Some(is_thundering),
                            ..Default::default()
                        };
                        drop_loot(world, block, pos, false, &params);
                    }

                    world.set_block_state(pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL);
                    world.close_container_screens_at(pos);

                    if let Some(pumpkin_block) = pumpkin_block {
                        pumpkin_block.explode(ExplodeArgs {
                            world,
                            block,
                            position: pos,
                        });
                    }
                }
                // TODO: fire
                blocks.len() as u32
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Explosion;
    use pumpkin_data::Block;

    #[test]
    fn tnt_minecart_rail_protection_covers_every_rail_type() {
        for rail in [
            &Block::RAIL,
            &Block::POWERED_RAIL,
            &Block::DETECTOR_RAIL,
            &Block::ACTIVATOR_RAIL,
        ] {
            assert!(Explosion::is_rail(rail));
        }
        assert!(!Explosion::is_rail(&Block::STONE));
    }
}

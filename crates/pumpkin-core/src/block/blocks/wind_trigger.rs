//! Vanilla `onExplosionHit` overrides for `TRIGGER_BLOCK` explosions (wind charges).
//!
//! Covers buttons, levers, bells, doors, trapdoors, fence gates and candles. Callers check
//! `ServerExplosion.canTriggerBlocks` first.

use std::sync::Arc;

use pumpkin_data::block_properties::{
    CandleCakeProperties, CandleLikeProperties, DoubleBlockHalf, LeverLikeProperties,
    OakDoorLikeProperties, OakFenceGateLikeProperties, OakTrapdoorLikeProperties,
};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockState, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::blocks::doors::DoorBlock;
use crate::block::blocks::redstone::bell::ring_bell;
use crate::block::blocks::redstone::buttons::ButtonBlock;
use crate::block::blocks::redstone::lever::LeverBlock;
use crate::block::blocks::{fence_gates, trapdoor};
use crate::world::World;

/// Iron doors and trapdoors are the only `BlockSetType`s with `canOpenByWindCharge = false`.
fn can_open_by_wind_charge(block: &Block) -> bool {
    block != &Block::IRON_DOOR && block != &Block::IRON_TRAPDOOR
}

/// Vanilla door/trapdoor/fence gate sound pitch: `random.nextFloat() * 0.1F + 0.9F`.
fn open_close_pitch() -> f32 {
    rand::random::<f32>().mul_add(0.1, 0.9)
}

pub fn on_explosion_hit(world: &Arc<World>, block: &Block, state: &BlockState, pos: &BlockPos) {
    if block.has_tag(&tag::Block::MINECRAFT_BUTTONS) {
        if !LeverLikeProperties::from_state_id(state.id).powered {
            ButtonBlock::press(world, pos);
        }
    } else if block == &Block::LEVER {
        LeverBlock::pull(world, pos);
    } else if block == &Block::BELL {
        ring_bell(*pos, world, None, None);
    } else if block.has_tag(&tag::Block::MINECRAFT_DOORS) {
        let props = OakDoorLikeProperties::from_state_id(state.id);
        if props.half == DoubleBlockHalf::Lower && can_open_by_wind_charge(block) && !props.powered
        {
            DoorBlock::set_open(world, pos, !props.open);
        }
    } else if block.has_tag(&tag::Block::MINECRAFT_TRAPDOORS) {
        toggle_trapdoor(world, block, state, pos);
    } else if block.has_tag(&tag::Block::MINECRAFT_FENCE_GATES) {
        toggle_fence_gate(world, block, state, pos);
    } else if block.has_tag(&tag::Block::MINECRAFT_CANDLES) {
        let mut props = CandleLikeProperties::from_state_id(state.id);
        if props.lit {
            props.lit = false;
            extinguish(world, pos, props.to_state_id(block));
        }
    } else if block.has_tag(&tag::Block::MINECRAFT_CANDLE_CAKES) {
        let mut props = CandleCakeProperties::from_state_id(state.id);
        if props.lit {
            props.lit = false;
            extinguish(world, pos, props.to_state_id(block));
        }
    }
}

/// `TrapDoorBlock.onExplosionHit` and `toggle`.
fn toggle_trapdoor(world: &Arc<World>, block: &Block, state: &BlockState, pos: &BlockPos) {
    let mut props = OakTrapdoorLikeProperties::from_state_id(state.id);
    if !can_open_by_wind_charge(block) || props.powered {
        return;
    }
    props.open = !props.open;
    world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_LISTENERS);
    world.play_sound_fine(
        trapdoor::get_sound(block, props.open),
        SoundCategory::Blocks,
        &pos.to_centered_f64(),
        1.0,
        open_close_pitch(),
    );
}

/// `FenceGateBlock.onExplosionHit`: unlike a player, the burst keeps the gate's facing.
fn toggle_fence_gate(world: &Arc<World>, block: &Block, state: &BlockState, pos: &BlockPos) {
    let mut props = OakFenceGateLikeProperties::from_state_id(state.id);
    if props.powered {
        return;
    }
    let was_open = props.open;
    props.open = !was_open;
    world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
    world.play_sound_fine(
        fence_gates::get_sound(block, props.open),
        SoundCategory::Blocks,
        &pos.to_centered_f64(),
        1.0,
        open_close_pitch(),
    );
    let event = if was_open {
        GameEvent::BlockClose
    } else {
        GameEvent::BlockOpen
    };
    world.emit_game_event(event.name(), pos.to_centered_f64());
}

/// `AbstractCandleBlock.extinguish` without a player.
fn extinguish(world: &Arc<World>, pos: &BlockPos, unlit_state: BlockStateId) {
    world.set_block_state(pos, unlit_state, BlockFlags::NOTIFY_ALL);
    world.play_sound(
        Sound::BlockCandleExtinguish,
        SoundCategory::Blocks,
        &pos.to_centered_f64(),
    );
    world.emit_game_event(GameEvent::BlockChange.name(), pos.to_centered_f64());
}

use pumpkin_util::math::vector2::Vector2;
use std::{
    num::NonZero,
    sync::{Arc, atomic::Ordering},
};

use pumpkin_protocol::java::client::play::CCenterChunk;
use pumpkin_world::cylindrical_chunk_iterator::Cylindrical;

use crate::{
    entity::{EntityBase, player::Player},
    net::ClientPlatform,
};

pub fn get_view_distance(player: &Player) -> NonZero<u8> {
    let fallback = NonZero::new(2).unwrap_or(NonZero::<u8>::MIN);
    let Some(server) = player.world().server.upgrade() else {
        return fallback;
    };
    let max_view_distance = {
        let ClientPlatform::Java(_) = player.client.as_ref();
        server.advanced_config.networking.java.view_distance
    };
    player
        .config
        .load()
        .view_distance
        .clamp(fallback, max_view_distance)
}

// Checks if the target chunk is within Chebyshev distance (L_infinity) of the center chunk.
#[must_use]
#[inline]
pub fn is_within_chebyshev_distance(
    center: Vector2<i32>,
    target: Vector2<i32>,
    distance: i32,
) -> bool {
    (target.x - center.x).abs().max((target.y - center.y).abs()) <= distance
}

/// Radius the tickets of a joining player start with, so the nearest chunks reach the client first.
pub const CHUNK_TICKET_RAMP_START: u8 = 4;
const CHUNK_TICKET_RAMP_STEP: u8 = 4;

fn simulation_distance(world: &crate::world::World) -> u8 {
    world.server.upgrade().map_or(10, |s| {
        s.advanced_config.networking.java.simulation_distance.get()
    })
}

fn apply_chunk_tickets(player: &Player, center: Vector2<i32>, view_distance: NonZero<u8>) {
    let world = player.world();
    let level = &world.level;
    let ramp = player.chunk_ticket_radius.load(Ordering::Relaxed);
    let mut held_tickets = player
        .held_chunk_tickets
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let is_spectator = player.is_spectator();
    let spectators_generate_chunks = world
        .level_info
        .load()
        .game_rules
        .spectators_generate_chunks;

    let new_view_level = (!is_spectator || spectators_generate_chunks).then(|| {
        pumpkin_world::chunk_system::ChunkLoading::get_level_from_view_distance(
            view_distance.get().min(ramp) + 1,
        )
    });

    let new_sim_level = (!is_spectator || spectators_generate_chunks).then(|| {
        pumpkin_world::chunk_system::ChunkLoading::get_level_from_simulation_distance(
            simulation_distance(&world).min(ramp),
        )
    });

    {
        let mut lock = level
            .chunk_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(view) = new_view_level {
            lock.add_ticket(center, view);
        }
        if let Some(sim) = new_sim_level {
            lock.add_ticket(center, sim);
        }

        if let Some((held_center, held_view, held_sim)) =
            held_tickets.replace((center, new_view_level, new_sim_level))
        {
            if let Some(view) = held_view {
                lock.remove_ticket(held_center, view);
            }
            if let Some(sim) = held_sim {
                lock.remove_ticket(held_center, sim);
            }
        }
        lock.send_change();
    };
}

/// Widens the ticket radius once every chunk inside the current one has finished loading.
pub fn ramp_chunk_tickets(player: &Player) {
    let ramp = player.chunk_ticket_radius.load(Ordering::Relaxed);
    let view_distance = get_view_distance(player);
    let world = player.world();
    if ramp >= view_distance.get().max(simulation_distance(&world)) {
        return;
    }

    let center = player.get_entity().chunk_pos.load();
    let loaded = Cylindrical::get_offsets(ramp).iter().all(|&(dx, dz)| {
        world.level.loaded_chunks.contains_key(&Vector2::new(
            center.x + i32::from(dx),
            center.y + i32::from(dz),
        ))
    });
    if loaded {
        player.chunk_ticket_radius.store(
            ramp.saturating_add(CHUNK_TICKET_RAMP_STEP),
            Ordering::Relaxed,
        );
        apply_chunk_tickets(player, center, view_distance);
    }
}

#[allow(clippy::too_many_lines)]
pub fn update_position(player: &Arc<Player>) {
    let entity = &player.get_entity();
    let new_chunk_center = entity.chunk_pos.load();
    let old_cylindrical = player.watched_section.load();

    // Vanilla `ChunkMap.move` -> re-pair on every move, not only on a view change.
    let world = player.world();
    world.entity_tracker.update_player_position(player, &world);

    // This does break when a new player spawns
    // if old_cylindrical.center == new_chunk_center {
    //     return;
    // }

    let view_distance = get_view_distance(player);
    let new_cylindrical = Cylindrical::new(new_chunk_center, view_distance);

    if old_cylindrical == new_cylindrical {
        return;
    }

    {
        let ClientPlatform::Java(java_client) = player.client.as_ref();
        {
            java_client.try_send_packet(&CCenterChunk {
                chunk_x: new_chunk_center.x.into(),
                chunk_z: new_chunk_center.y.into(),
            });
        }
    }
    let (loading_iter, unloading_iter) =
        Cylindrical::changed_chunks(old_cylindrical, new_cylindrical);
    let loading_chunks: Vec<_> = loading_iter.collect();
    let unloading_chunks: Vec<_> = unloading_iter.collect();

    apply_chunk_tickets(player, new_chunk_center, view_distance);

    {
        let mut sender = player
            .chunk_sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for pos in &unloading_chunks {
            sender.unload_chunk(&player.client, *pos);
        }
        for pos in &loading_chunks {
            sender.enqueue_chunk(*pos);
        }
    }
    player.watched_section.store(new_cylindrical);

    // Make sure the watched section and the chunk watcher updates are async atomic. We want to
    // ensure what we unload when the player disconnects is correct.
    if !loading_chunks.is_empty() || !unloading_chunks.is_empty() {
        let level = world.level.clone();
        let world_clone = world.clone();
        let loading_chunks_clone = loading_chunks.clone();
        let unloading_chunks_clone = unloading_chunks;

        if let Some(server) = world.server.upgrade() {
            server.spawn_task(async move {
                level.mark_chunks_as_newly_watched(&loading_chunks_clone);
                let chunks_to_clean = level.mark_chunks_as_not_watched(&unloading_chunks_clone);

                if !chunks_to_clean.is_empty() {
                    world_clone
                        .remove_entities_in_chunks(&chunks_to_clean)
                        .await;
                    world_clone.level.clean_entity_chunks(&chunks_to_clean);
                }
            });
        }
    }

    if !loading_chunks.is_empty() {
        world.spawn_world_entity_chunks(player.clone(), loading_chunks);
    }
}

use std::{
    any::Any,
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex, atomic::Ordering},
};

use base64::{Engine, prelude::BASE64_STANDARD};
use bytes::Bytes;
use ferrum_neoforge::world::{self as bridge, WorldAccess};
use pumpkin_data::{
    BlockStateId, data_component::DataComponent, data_component_impl::CustomDataImpl,
    entity::EntityType, item::Item, item_stack::ItemStack,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::{RawPacket, java::client::play::CCustomPayload};
use pumpkin_util::{
    Hand,
    math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3},
};
use pumpkin_world::{chunk::io::Dirtiable, world::BlockFlags};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tracing::{error, warn};

use crate::{
    block::entities::BlockEntity,
    entity::{
        Entity, EntityBase, experience_orb::ExperienceOrbEntity, item::ItemEntity,
        living::LivingEntity, player::Player,
    },
    net::{ClientPlatform, java::JavaClient},
    server::Server,
    world::World,
};

pub struct ModBlockEntity {
    id: &'static str,
    position: BlockPos,
    nbt: Mutex<NbtCompound>,
}

impl ModBlockEntity {
    #[must_use]
    pub fn load(nbt: &NbtCompound, position: BlockPos) -> Option<Self> {
        let id = pumpkin_data::runtime::block_entity_by_name(nbt.get_string("id")?)?;
        Some(Self {
            id,
            position,
            nbt: Mutex::new(nbt.clone()),
        })
    }
    #[must_use]
    pub fn new(id: &'static str, position: BlockPos) -> Self {
        let mut nbt = NbtCompound::new();
        nbt.put_string("id", id.to_owned());
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        Self {
            id,
            position,
            nbt: Mutex::new(nbt),
        }
    }
}

impl BlockEntity for ModBlockEntity {
    fn write_nbt(&self, nbt: &mut NbtCompound) {
        *nbt = self
            .nbt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
    }
    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self {
        // The factory validates the type before reaching this trait entry point.
        Self {
            id: pumpkin_data::runtime::block_entity_by_name(nbt.get_string("id").unwrap_or(""))
                .unwrap_or("ferrum:unknown"),
            position,
            nbt: Mutex::new(nbt.clone()),
        }
    }
    fn resource_location(&self) -> &'static str {
        self.id
    }
    fn get_position(&self) -> BlockPos {
        self.position
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct RustWorld(Arc<World>);

#[derive(Deserialize)]
struct Request {
    op: String,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    z: i32,
    #[serde(default)]
    state: u32,
    #[serde(default)]
    flags: u32,
}

impl WorldAccess for RustWorld {
    fn call(&self, request: Value) -> Result<Value, String> {
        if let Some(result) = entity_op(&self.0, &request) {
            return result;
        }
        let request: Request = serde_json::from_value(request).map_err(|e| e.to_string())?;
        let world = &self.0;
        let pos = BlockPos::new(request.x, request.y, request.z);
        match request.op.as_str() {
            "get" => {
                let state = if world.is_in_height_limit(pos.0.y) {
                    world.get_block_state_id_if_loaded(&pos).ok_or_else(|| {
                        format!("NeoForge accessed unloaded Rust chunk at {pos:?}")
                    })?
                } else {
                    BlockStateId::AIR
                };
                Ok(json!(bridge::java_state(state)?))
            }
            "set" => {
                let state =
                    bridge::rust_state(request.state).ok_or("Java state has no Rust mapping")?;
                let flags = translate_flags(request.flags);
                if !world.is_in_build_limit(pos)
                    || !world.level.is_chunk_loaded(&pos.chunk_position())
                {
                    return Ok(json!(false));
                }
                let old = world.set_block_state(&pos, state, flags);
                Ok(json!(old != state))
            }
            "be" => Ok(world.get_block_entity(&pos).map_or(Value::Null, |entity| {
                let mut nbt = NbtCompound::new();
                entity.write_internal(&mut nbt);
                json!(bridge::encode_nbt(&nbt))
            })),
            "dirty" => {
                world
                    .level
                    .read_chunk_sync(&pos.chunk_position(), |c| c.mark_dirty(true));
                Ok(Value::Null)
            }
            "comparator" => {
                world.update_neighbour_for_output_signal(&pos, world.get_block(&pos));
                Ok(Value::Null)
            }
            "update" => {
                let state = world.get_block_state_id(&pos);
                world.send_block_updated(&pos, state, state);
                Ok(Value::Null)
            }
            "loaded" => Ok(json!(world.level.is_chunk_loaded(&pos.chunk_position()))),
            "time" => Ok(json!(world.get_world_age())),
            "seaLevel" => Ok(json!(world.sea_level)),
            "random" => Ok(json!(rand::random::<i32>())),
            _ => Err(format!(
                "unsupported NeoForge world operation: {}",
                request.op
            )),
        }
    }
}

fn translate_flags(flags: u32) -> BlockFlags {
    // Block.java's update flags differ from Pumpkin's internal representation.
    const UPDATE_NEIGHBORS: u32 = 1;
    const UPDATE_CLIENTS: u32 = 2;
    const UPDATE_KNOWN_SHAPE: u32 = 16;
    const UPDATE_SUPPRESS_DROPS: u32 = 32;
    const UPDATE_MOVE_BY_PISTON: u32 = 64;
    const UPDATE_SKIP_SHAPE_UPDATE_ON_WIRE: u32 = 128;
    const UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS: u32 = 256;
    const UPDATE_SKIP_ON_PLACE: u32 = 512;
    let mut result = BlockFlags::empty();
    for (java, rust) in [
        (UPDATE_NEIGHBORS, BlockFlags::NOTIFY_NEIGHBORS),
        (UPDATE_CLIENTS, BlockFlags::NOTIFY_LISTENERS),
        (UPDATE_KNOWN_SHAPE, BlockFlags::UPDATE_KNOWN_SHAPE),
        (UPDATE_SUPPRESS_DROPS, BlockFlags::SKIP_DROPS),
        (UPDATE_MOVE_BY_PISTON, BlockFlags::MOVED),
        (
            UPDATE_SKIP_SHAPE_UPDATE_ON_WIRE,
            BlockFlags::SKIP_REDSTONE_WIRE_STATE_REPLACEMENT,
        ),
        (
            UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS,
            BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK,
        ),
        (UPDATE_SKIP_ON_PLACE, BlockFlags::SKIP_BLOCK_ADDED_CALLBACK),
    ] {
        if flags & java != 0 {
            result.insert(rust);
        }
    }
    result
}

pub fn tick(world: &Arc<World>, entities: &[Arc<dyn BlockEntity>]) {
    if !bridge::enabled() {
        return;
    }
    if let Err(error) = tick_inner(world, entities) {
        tracing::error!("NeoForge block entity batch failed: {error}");
    }
}

fn tick_inner(
    world: &Arc<World>,
    entities: &[Arc<dyn BlockEntity>],
) -> Result<(), ferrum_neoforge::Error> {
    let mut modded: Vec<_> = entities
        .iter()
        .filter(|be| be.as_any().is::<ModBlockEntity>())
        .collect();
    modded.sort_unstable_by_key(|be| {
        let p = be.get_position();
        (p.0.x, p.0.y, p.0.z)
    });
    let mut batch = Vec::new();
    let mut retained = Vec::new();
    for be in modded {
        let pos = be.get_position();
        let chunk = pos.chunk_position();
        // Neighbor queries must not synchronously load chunks from a JNI callback.
        if !(-1..=1).all(|x| {
            (-1..=1).all(|z| {
                world
                    .level
                    .is_chunk_loaded(&pumpkin_util::math::vector2::Vector2::new(
                        chunk.x + x,
                        chunk.y + z,
                    ))
            })
        }) {
            // Not ticked, but still alive: Java must not treat it as removed.
            retained.push(json!({"x":pos.0.x,"y":pos.0.y,"z":pos.0.z}));
            continue;
        }
        let state = bridge::java_state(world.get_block_state_id(&pos))
            .map_err(ferrum_neoforge::Error::Runtime)?;
        let mut nbt = NbtCompound::new();
        be.write_internal(&mut nbt);
        batch.push(json!({"x":pos.0.x,"y":pos.0.y,"z":pos.0.z,"state":state,"nbt":bridge::encode_nbt(&nbt)}));
    }
    let result = bridge::tick(
        Arc::new(RustWorld(world.clone())),
        &json!({"world":world.uuid.to_string(),"minY":world.dimension.min_y,"height":world.dimension.height,"dimension":world.dimension.minecraft_name,"entities":batch,"retained":retained}),
    )?;
    deliver(
        world,
        result["packets"].as_array().map_or(&[][..], Vec::as_slice),
    );
    let result: Vec<Snapshot> = serde_json::from_value(result["entities"].clone())?;
    let mut commits = Vec::new();
    for snapshot in result {
        let pos = BlockPos::new(snapshot.x, snapshot.y, snapshot.z);
        let nbt = bridge::decode_nbt(&snapshot.nbt)?;
        let Some(entity) = world.get_block_entity(&pos) else {
            continue;
        };
        if entity.as_any().is::<ModBlockEntity>()
            && nbt.get_string("id") == Some(entity.resource_location())
            && nbt.get_int("x") == Some(pos.0.x)
            && nbt.get_int("y") == Some(pos.0.y)
            && nbt.get_int("z") == Some(pos.0.z)
        {
            commits.push((entity, nbt));
        } else {
            return Err(ferrum_neoforge::Error::Runtime(
                "invalid block entity snapshot identity".into(),
            ));
        }
    }
    for (entity, nbt) in commits {
        if let Some(be) = entity.as_any().downcast_ref::<ModBlockEntity>() {
            *be.nbt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = nbt.clone();
            world.add_block_entity_nbt(be.position, &nbt);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Snapshot {
    x: i32,
    y: i32,
    z: i32,
    nbt: String,
}

const MAIN_SLOTS: usize = 36;
const OFF_HAND_SLOT: usize = 40;
/// Bound on mod payload packets one player step may return.
const MAX_PLAYER_PACKETS: usize = 256;

fn is_modded(stack: &ItemStack) -> bool {
    let key = stack.item.registry_key;
    key.contains(':') && !key.starts_with("minecraft:")
}

/// Whether the player's hand item belongs to a mod; vanilla items never leave Rust.
#[must_use]
pub fn modded_item_in_hand(player: &Player, hand: Hand) -> bool {
    bridge::enabled() && is_modded(&player.inventory().get_stack_in_hand(hand))
}

fn holds_modded_items(player: &Player) -> bool {
    let inventory = player.inventory();
    let main = inventory
        .main_inventory
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    main.iter().any(is_modded) || is_modded(&inventory.off_hand_item())
}

fn hand_id(hand: Hand) -> i32 {
    i32::from(matches!(hand, Hand::Left))
}

/// Forwards a serverbound mod payload to `NeoForge`'s handler; false when no handler is registered.
pub fn player_payload(player: &Player, channel: &str, data: &[u8]) -> bool {
    if !bridge::enabled() {
        return false;
    }
    let event =
        json!({"type": "payload", "channel": channel, "data": BASE64_STANDARD.encode(data)});
    player_events(player, &[event])
        .first()
        .is_some_and(|result| result["handled"] == true)
}

/// Runs the held modded item's `use`; true when Java consumed the action.
pub fn player_use(player: &Player, hand: Hand) -> bool {
    consumed(&player_events(
        player,
        &[json!({"type": "use", "hand": hand_id(hand)})],
    ))
}

/// Runs the held modded item's `useOn`; block items stay with Rust placement.
pub fn player_use_on(
    player: &Player,
    hand: Hand,
    position: BlockPos,
    face: i32,
    cursor: Vector3<f32>,
    inside: bool,
) -> bool {
    let event = json!({
        "type": "use_on", "hand": hand_id(hand), "x": position.0.x, "y": position.0.y, "z": position.0.z,
        "face": face, "cx": cursor.x, "cy": cursor.y, "cz": cursor.z, "inside": inside,
    });
    consumed(&player_events(player, &[event]))
}

/// Whether the block belongs to a mod; vanilla blocks never leave Rust.
#[must_use]
pub fn modded_block_at(world: &World, position: &BlockPos) -> bool {
    bridge::enabled()
        && pumpkin_data::runtime::state_block(world.get_block_state_id(position)).is_some()
}

/// Runs the modded block's `useItemOn` and `useWithoutItem`; true when Java consumed the action.
pub fn player_use_block(
    player: &Player,
    hand: Hand,
    position: BlockPos,
    face: i32,
    cursor: Vector3<f32>,
    inside: bool,
) -> bool {
    let event = json!({
        "type": "use_block", "hand": hand_id(hand), "x": position.0.x, "y": position.0.y, "z": position.0.z,
        "face": face, "cx": cursor.x, "cy": cursor.y, "cz": cursor.z, "inside": inside,
    });
    consumed(&player_events(player, &[event]))
}

/// Container ids of the menus Java has open, by player; absent when only the inventory is open.
static OPEN_MENUS: LazyLock<Mutex<HashMap<uuid::Uuid, i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn open_menus() -> std::sync::MutexGuard<'static, HashMap<uuid::Uuid, i64>> {
    OPEN_MENUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn menu_open(player: &Player) -> bool {
    open_menus().contains_key(&player.gameprofile.id)
}

/// The chunks of a player's watched cylinder as last reported to Java, with the world and cylinder they came from.
struct WatchedChunks {
    world: uuid::Uuid,
    center: (i32, i32),
    view_distance: u8,
    chunks: HashSet<(i32, i32)>,
}

static WATCHED: LazyLock<Mutex<HashMap<uuid::Uuid, WatchedChunks>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Mods that sync block entities to watchers need to know which chunks a player has; this is the diff since the last report.
fn tracking_event(player: &Player) -> Option<Value> {
    let section = player.watched_section.load();
    let world = player.world().uuid;
    let center = (section.center.x, section.center.y);
    let view_distance = section.view_distance.get();
    let mut watched = WATCHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = watched.get(&player.gameprofile.id);
    if previous.is_some_and(|known| {
        known.world == world && known.center == center && known.view_distance == view_distance
    }) {
        return None;
    }
    let chunks: HashSet<(i32, i32)> = section.all_chunks_within().map(|c| (c.x, c.y)).collect();
    let (added, removed): (Vec<_>, Vec<_>) = match previous {
        Some(known) if known.world == world => (
            chunks.difference(&known.chunks).copied().collect(),
            known.chunks.difference(&chunks).copied().collect(),
        ),
        _ => (chunks.iter().copied().collect(), Vec::new()),
    };
    watched.insert(
        player.gameprofile.id,
        WatchedChunks {
            world,
            center,
            view_distance,
            chunks,
        },
    );
    Some(json!({"type": "tracking", "added": added, "removed": removed}))
}

/// Runs the end-of-tick network flush mods do on `NeoForge`'s server tick event.
pub fn server_tick_end(server: &Server) {
    if !bridge::enabled() {
        return;
    }
    let worlds = server.worlds.load();
    let Some(world) = worlds.first() else {
        return;
    };
    let request = json!({"op": "tick_post"});
    match bridge::play(Arc::new(RustWorld(world.clone())), &request) {
        Ok(reply) => deliver(
            world,
            reply["packets"].as_array().map_or(&[][..], Vec::as_slice),
        ),
        Err(error) => error!("NeoForge server tick failed: {error}"),
    }
}

/// Forwards a serverbound container packet to the menu Java opened; false when Rust handles it.
pub fn player_menu_packet(player: &Player, packet: &RawPacket) -> bool {
    if !bridge::enabled() || !menu_open(player) {
        return false;
    }
    // Java decodes the packet exactly as the client wrote it: id, then body.
    let mut data = Vec::with_capacity(packet.payload.len() + 5);
    let mut id = packet.id as u32;
    loop {
        let byte = (id & 0x7F) as u8;
        id >>= 7;
        if id == 0 {
            data.push(byte);
            break;
        }
        data.push(byte | 0x80);
    }
    data.extend_from_slice(&packet.payload);
    player_events(
        player,
        &[json!({"type": "menu_packet", "data": BASE64_STANDARD.encode(data)})],
    );
    true
}

fn consumed(results: &[Value]) -> bool {
    results
        .first()
        .is_some_and(|result| result["consumed"] == true)
}

/// Vanilla `Player.tick` ticks the inventory and the open menu; modded stacks and menus tick in Java.
pub fn tick_player(player: &Player) {
    if !bridge::enabled() {
        return;
    }
    let perf_t0 = std::time::Instant::now(); // FERRUMPERF
    let mut events = Vec::new();
    events.extend(tracking_event(player));
    let perf_tracking = events.len(); // FERRUMPERF
    if holds_modded_items(player) {
        events.push(json!({"type": "tick"}));
    }
    if menu_open(player) {
        events.push(json!({"type": "menu_tick"}));
    }
    if !events.is_empty() {
        player_events(player, &events);
    }
    if perf_t0.elapsed().as_micros() > 2000 { tracing::info!("FERRUMPERF tick_player ms={:.2} tracking={}", perf_t0.elapsed().as_secs_f64()*1e3, perf_tracking); } // FERRUMPERF
}

pub fn player_left(player: &Player) {
    if !bridge::enabled() {
        return;
    }
    open_menus().remove(&player.gameprofile.id);
    WATCHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&player.gameprofile.id);
    let request = json!({"op": "leave", "uuid": player.gameprofile.id.to_string()});
    // Mods' chunk watcher hooks query the world while the player's tracked chunks are released.
    if let Err(error) = bridge::play(Arc::new(RustWorld(player.world())), &request) {
        warn!("Failed to release NeoForge player view: {error}");
    }
}

/// One synchronous snapshot-in, delta-out exchange with the player's Java view.
fn player_events(player: &Player, events: &[Value]) -> Vec<Value> {
    match exchange(player, events) {
        Ok(results) => results,
        Err(message) => {
            error!(
                "NeoForge player step failed for {}: {message}",
                player.gameprofile.name
            );
            Vec::new()
        }
    }
}

fn exchange(player: &Player, events: &[Value]) -> Result<Vec<Value>, String> {
    let world = player.world();
    let inventory = player.inventory();
    let mut snapshot = Map::new();
    for slot in (0..MAIN_SLOTS).chain([OFF_HAND_SLOT]) {
        let stack = inventory.get_slot(slot);
        if stack.is_empty() {
            continue;
        }
        let mut nbt = NbtCompound::new();
        stack.write_item_stack(&mut nbt);
        snapshot.insert(slot.to_string(), json!(bridge::encode_nbt(&nbt)));
    }
    let (yaw, pitch) = player.rotation();
    let position = player.position();
    let request = json!({
        "op": "sync", "uuid": player.gameprofile.id.to_string(), "name": player.gameprofile.name,
        "entityId": player.entity_id(), "world": world.uuid.to_string(),
        "minY": world.dimension.min_y, "height": world.dimension.height,
        "dimension": world.dimension.minecraft_name,
        "x": position.x, "y": position.y, "z": position.z, "yaw": yaw, "pitch": pitch,
        "gamemode": player.gamemode.load() as i32, "selected": inventory.get_selected_slot(),
        "sneaking": player.get_entity().is_sneaking(),
        "inventory": snapshot, "events": events,
    });
    let reply = bridge::play(Arc::new(RustWorld(world)), &request).map_err(|e| e.to_string())?;
    if let Some(changes) = reply["inventory"].as_object() {
        for (slot, encoded) in changes {
            let slot: usize = slot.parse().map_err(|_| "bad inventory slot".to_owned())?;
            if slot >= MAIN_SLOTS && slot != OFF_HAND_SLOT {
                return Err(format!("Java changed unsupported inventory slot {slot}"));
            }
            let nbt = bridge::decode_nbt(encoded.as_str().unwrap_or_default())
                .map_err(|e| e.to_string())?;
            inventory.set_slot(slot, merged_stack(inventory.get_slot(slot), &nbt));
        }
    }
    let packets = reply["packets"].as_array().map_or(&[][..], Vec::as_slice);
    if packets.len() > MAX_PLAYER_PACKETS {
        return Err("Java returned too many packets".into());
    }
    let mut menus = open_menus();
    if let Some(id) = reply["menu"].as_i64().filter(|id| *id != 0) {
        menus.insert(player.gameprofile.id, id);
    } else {
        menus.remove(&player.gameprofile.id);
    }
    drop(menus);
    deliver(&player.world(), packets);
    Ok(reply["results"].as_array().cloned().unwrap_or_default())
}

/// Applies the parts of a Java stack Rust shares (item, count, custom data, damage) onto Rust's stack.
fn merged_stack(current: ItemStack, java: &NbtCompound) -> ItemStack {
    if java.get_bool("empty") == Some(true) {
        return ItemStack::EMPTY.clone();
    }
    let Some(id) = java.get_string("id") else {
        return current;
    };
    let item = if id.starts_with("minecraft:") {
        Item::from_registry_key(id)
    } else {
        pumpkin_data::runtime::item_by_name(id)
    };
    let (Some(item), Some(count)) = (item, java.get_int("count")) else {
        warn!("Java returned an item Rust does not know: {id}");
        return current;
    };
    let mut stack = if current.item.id == item.id && !current.is_empty() {
        current
    } else {
        ItemStack::new(0, item)
    };
    stack.item_count = u8::try_from(count).unwrap_or(u8::MAX);
    let components = java.get_compound("components");
    match components.and_then(|c| c.get_compound("minecraft:custom_data")) {
        Some(data) => stack.set_data_component(CustomDataImpl::new(data.clone())),
        None => stack.remove_data_component(DataComponent::CustomData),
    }
    stack.set_damage(
        components
            .and_then(|c| c.get_int("minecraft:damage"))
            .unwrap_or(0),
    );
    stack
}

/// A mod entity Java spawned and ticks: Rust owns its identity, position mirror and tracking.
pub struct ModEntity {
    entity: Entity,
    /// Encoded 26.2 set-entity-data packet with the entity's non-default synched data, for new viewers.
    metadata: Mutex<Option<Vec<u8>>>,
}

impl ModEntity {
    fn set_metadata(&self, metadata: Option<Vec<u8>>) {
        *self
            .metadata
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = metadata;
    }
}

impl EntityBase for ModEntity {
    // Java ticks bridged entities in one serial pass, see `tick_entities`.
    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {}

    fn init_data_tracker(&self) {}

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn Any {
        self
    }

    fn is_pushable(&self) -> bool {
        false
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: pumpkin_data::damage::DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        false
    }

    fn send_java_spawn_packet(&self, client: &JavaClient) {
        if let Ok(data) = client.serialize_packet(&self.entity.create_spawn_packet()) {
            client.try_enqueue_packet(data);
        }
        let metadata = self
            .metadata
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(metadata) = metadata {
            client.try_enqueue_native_packet(Bytes::from(metadata));
        }
    }
}

fn as_mod_entity(entity: &dyn EntityBase) -> Option<&ModEntity> {
    entity.cast_any().downcast_ref::<ModEntity>()
}

fn f64_of(request: &Value, key: &str) -> Result<f64, String> {
    request[key]
        .as_f64()
        .ok_or_else(|| format!("entity operation misses {key}"))
}

/// Entity operations Java asks of Rust's world: identity, spawning, queries and damage.
fn entity_op(world: &Arc<World>, request: &Value) -> Option<Result<Value, String>> {
    Some(match request["op"].as_str()? {
        "allocate_entity" => Ok(json!({ "id": Entity::reserve_ids(1) })),
        "spawn_entity" => spawn_mod_entity(world, request),
        "entities_in" => entities_in(world, request),
        "entity" => Ok(find_entity(world, request).map_or(Value::Null, |entity| snapshot(&entity))),
        "damage" => damage_entity(world, request),
        "spawn_item" => spawn_item(world, request),
        "spawn_experience" => spawn_experience(world, request),
        _ => return None,
    })
}

fn spawn_mod_entity(world: &Arc<World>, request: &Value) -> Result<Value, String> {
    let name = request["type"].as_str().unwrap_or_default();
    let entity_type = pumpkin_data::runtime::entity_type_by_name(name)
        .ok_or_else(|| format!("unknown mod entity type {name}"))?;
    let id = request["id"]
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .ok_or("spawn_entity misses id")?;
    let uuid = request["uuid"]
        .as_str()
        .and_then(|uuid| uuid::Uuid::parse_str(uuid).ok())
        .ok_or("spawn_entity misses uuid")?;
    let position = Vector3::new(
        f64_of(request, "x")?,
        f64_of(request, "y")?,
        f64_of(request, "z")?,
    );
    let entity = Entity::from_uuid_with_id(id, uuid, world.clone(), position, entity_type);
    entity.velocity.store(Vector3::new(
        f64_of(request, "vx")?,
        f64_of(request, "vy")?,
        f64_of(request, "vz")?,
    ));
    entity.set_rotation(
        f64_of(request, "yaw")? as f32,
        f64_of(request, "pitch")? as f32,
    );
    let metadata = request["meta"]
        .as_str()
        .map(|meta| BASE64_STANDARD.decode(meta))
        .transpose()
        .map_err(|e| e.to_string())?;
    Ok(json!(world.spawn_entity(Arc::new(ModEntity {
        entity,
        metadata: Mutex::new(metadata),
    }))))
}

fn position_of(request: &Value) -> Result<Vector3<f64>, String> {
    Ok(Vector3::new(
        f64_of(request, "x")?,
        f64_of(request, "y")?,
        f64_of(request, "z")?,
    ))
}

/// Mod code dropped an item: Rust spawns the real item entity with the stack Java built.
fn spawn_item(world: &Arc<World>, request: &Value) -> Result<Value, String> {
    let nbt = bridge::decode_nbt(request["stack"].as_str().ok_or("spawn_item misses stack")?)
        .map_err(|e| e.to_string())?;
    let stack = merged_stack(ItemStack::EMPTY.clone(), &nbt);
    if stack.is_empty() {
        return Ok(json!(false));
    }
    let velocity = Vector3::new(
        f64_of(request, "vx")?,
        f64_of(request, "vy")?,
        f64_of(request, "vz")?,
    );
    // Vanilla's infinite pickup delay (32767) does not fit Rust's counter, so it saturates.
    let delay = request["delay"]
        .as_i64()
        .map_or(ItemEntity::DEFAULT_PICKUP_DELAY, |delay| {
            u8::try_from(delay.clamp(0, i64::from(u8::MAX))).unwrap_or(u8::MAX)
        });
    let entity = Entity::new(world.clone(), position_of(request)?, &EntityType::ITEM);
    Ok(json!(world.spawn_entity(Arc::new(
        ItemEntity::new_with_velocity(entity, stack, velocity, delay)
    ))))
}

fn spawn_experience(world: &Arc<World>, request: &Value) -> Result<Value, String> {
    let amount = request["amount"]
        .as_u64()
        .and_then(|amount| u32::try_from(amount).ok())
        .ok_or("spawn_experience misses amount")?;
    ExperienceOrbEntity::spawn(world, position_of(request)?, amount);
    Ok(json!(true))
}

/// Java's view of one Rust entity (players included): identity, bounds, rotation and health.
fn snapshot(entity: &Arc<dyn EntityBase>) -> Value {
    let base = entity.get_entity();
    let position = base.pos.load();
    let bounds = base.bounding_box.load();
    let name = base.entity_type.resource_name;
    let player = entity.get_player();
    json!({
        "id": base.entity_id,
        "uuid": base.entity_uuid.to_string(),
        "kind": if player.is_some() { "player" } else { "entity" },
        "type": if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") },
        "name": player.map(|player| player.gameprofile.name.clone()),
        "gamemode": player.map_or(0, |player| player.gamemode.load() as i32),
        "x": position.x, "y": position.y, "z": position.z,
        "yaw": base.yaw.load(), "pitch": base.pitch.load(),
        "minX": bounds.min.x, "minY": bounds.min.y, "minZ": bounds.min.z,
        "maxX": bounds.max.x, "maxY": bounds.max.y, "maxZ": bounds.max.z,
        "health": entity.get_living_entity().map_or(1.0, |living| living.health.load()),
    })
}

fn entities_in(world: &Arc<World>, request: &Value) -> Result<Value, String> {
    let area = BoundingBox {
        min: Vector3::new(
            f64_of(request, "minX")?,
            f64_of(request, "minY")?,
            f64_of(request, "minZ")?,
        ),
        max: Vector3::new(
            f64_of(request, "maxX")?,
            f64_of(request, "maxY")?,
            f64_of(request, "maxZ")?,
        ),
    };
    // Bridged entities are Java's own and answered from Java state.
    let found: Vec<Value> = world
        .get_entities_at_box(&area)
        .iter()
        .filter(|entity| as_mod_entity(entity.as_ref()).is_none())
        .map(snapshot)
        .chain(
            world
                .get_players_at_box(&area)
                .into_iter()
                .map(|player| snapshot(&(player as Arc<dyn EntityBase>))),
        )
        .collect();
    Ok(Value::Array(found))
}

fn find_entity(world: &Arc<World>, request: &Value) -> Option<Arc<dyn EntityBase>> {
    let entity = if let Some(id) = request["id"].as_i64() {
        world.get_entity_by_id(i32::try_from(id).ok()?)?
    } else {
        let uuid = uuid::Uuid::parse_str(request["uuid"].as_str()?).ok()?;
        world.get_entity_by_uuid(uuid).or_else(|| {
            world
                .get_player_by_uuid(uuid)
                .map(|player| player as Arc<dyn EntityBase>)
        })?
    };
    as_mod_entity(entity.as_ref()).is_none().then_some(entity)
}

fn damage_entity(world: &Arc<World>, request: &Value) -> Result<Value, String> {
    let id = request["entity"]
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .ok_or("damage misses entity")?;
    let Some(target) = world.get_entity_by_id(id) else {
        return Ok(json!(false));
    };
    let by_id = |key: &str| {
        request[key]
            .as_i64()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(|id| world.get_entity_by_id(id))
    };
    let name = request["type"].as_str().unwrap_or_default();
    // Damage types mods define have no engine entry.
    let damage_type = pumpkin_data::damage::DamageType::from_name(
        name.strip_prefix("minecraft:").unwrap_or(name),
    )
    .unwrap_or(pumpkin_data::damage::DamageType::GENERIC);
    let direct = by_id("direct");
    let attacker = by_id("attacker");
    Ok(json!(target.damage_with_context(
        target.as_ref(),
        f64_of(request, "amount")? as f32,
        damage_type,
        None,
        direct.as_deref(),
        attacker.as_deref(),
    )))
}

/// Bound on entity state Java may return per tick.
const MAX_ENTITY_STATES: usize = 1 << 16;

/// Ticks the world's bridged entities in Java in one serial pass, after Rust's own entities,
/// then mirrors what Java changed. Their reach is unknown, so nothing runs beside them.
pub fn tick_entities(world: &Arc<World>) {
    if !bridge::enabled() {
        return;
    }
    if let Err(error) = tick_entities_inner(world) {
        error!("NeoForge entity pass failed: {error}");
    }
}

fn tick_entities_inner(world: &Arc<World>) -> Result<(), String> {
    let active = world
        .active_chunks
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut alive = Vec::new();
    let mut ticking = Vec::new();
    for entity in world.entities.load().iter() {
        let Some(modded) = as_mod_entity(entity.as_ref()) else {
            continue;
        };
        let base = &modded.entity;
        let chunk = base.chunk_pos.load();
        alive.push(base.entity_id);
        if active.contains(&chunk) && world.level.is_chunk_loaded(&chunk) {
            ticking.push(base.entity_id);
        }
    }
    if alive.is_empty() {
        return Ok(());
    }
    alive.sort_unstable();
    ticking.sort_unstable();
    let request = json!({
        "op": "entities", "world": world.uuid.to_string(),
        "minY": world.dimension.min_y, "height": world.dimension.height,
        "dimension": world.dimension.minecraft_name,
        "alive": alive, "ticking": ticking,
    });
    let reply = bridge::play(Arc::new(RustWorld(world.clone())), &request)
        .map_err(|error| error.to_string())?;
    let states = reply["states"].as_array().map_or(&[][..], Vec::as_slice);
    if states.len() > MAX_ENTITY_STATES {
        return Err("Java returned too many entity states".into());
    }
    for state in states {
        let Some(id) = state["id"].as_i64().and_then(|id| i32::try_from(id).ok()) else {
            continue;
        };
        let Some(entity) = world.get_entity_by_id(id) else {
            continue;
        };
        let Some(modded) = as_mod_entity(entity.as_ref()) else {
            continue;
        };
        let get = |key: &str| state[key].as_f64().unwrap_or_default();
        let base = &modded.entity;
        base.set_pos(Vector3::new(get("x"), get("y"), get("z")));
        base.velocity
            .store(Vector3::new(get("vx"), get("vy"), get("vz")));
        base.set_rotation(get("yaw") as f32, get("pitch") as f32);
        base.on_ground.store(
            state["ground"].as_bool().unwrap_or(false),
            Ordering::Relaxed,
        );
        if let Some(meta) = state["meta"].as_str() {
            modded.set_metadata(BASE64_STANDARD.decode(meta).ok());
        }
    }
    for id in reply["removed"].as_array().into_iter().flatten() {
        if let Some(entity) = id
            .as_i64()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(|id| world.get_entity_by_id(id))
            .filter(|entity| as_mod_entity(entity.as_ref()).is_some())
        {
            world.remove_entity(entity.as_ref());
        }
    }
    deliver(
        world,
        reply["packets"].as_array().map_or(&[][..], Vec::as_slice),
    );
    Ok(())
}

/// Sends what Java queued: mod payloads by channel, vanilla packets as the 26.2 bytes Java encoded.
fn deliver(world: &Arc<World>, packets: &[Value]) {
    for packet in packets {
        let Some(data) = packet["data"]
            .as_str()
            .and_then(|data| BASE64_STANDARD.decode(data).ok())
        else {
            warn!("Java returned a malformed clientbound packet");
            continue;
        };
        if packet["kind"] == "vanilla" {
            let data = Bytes::from(data);
            for target in vanilla_targets(world, packet) {
                let ClientPlatform::Java(client) = target.client.as_ref();
                client.try_enqueue_native_packet(data.clone());
            }
        } else if let (Some(target), Some(channel)) = (
            packet["uuid"]
                .as_str()
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .and_then(|id| player_by_uuid(world, id)),
            packet["channel"].as_str(),
        ) {
            let ClientPlatform::Java(client) = target.client.as_ref();
            match client.serialize_packet(&CCustomPayload::new(channel, &data)) {
                Ok(bytes) => client.try_enqueue_packet(bytes),
                Err(error) => warn!("Cannot send Java payload {channel}: {error}"),
            }
        }
    }
}

/// A player of the world, or of any other world of its server.
fn player_by_uuid(world: &Arc<World>, id: uuid::Uuid) -> Option<Arc<Player>> {
    world.get_player_by_uuid(id).or_else(|| {
        world
            .server
            .upgrade()
            .and_then(|server| server.get_player_by_uuid(id))
    })
}

/// Players a Java packet is meant for: one player, an area of a world, or the viewers of an entity.
fn vanilla_targets(world: &Arc<World>, packet: &Value) -> Vec<Arc<Player>> {
    match packet["to"].as_str() {
        Some("player") => packet["uuid"]
            .as_str()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .and_then(|id| player_by_uuid(world, id))
            .into_iter()
            .collect(),
        Some("area") => {
            let target = packet["world"]
                .as_str()
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .and_then(|id| {
                    if id == world.uuid {
                        return Some(world.clone());
                    }
                    let server = world.server.upgrade()?;
                    server.worlds.load().iter().find(|w| w.uuid == id).cloned()
                });
            let (Some(target), Some(x), Some(y), Some(z), Some(radius)) = (
                target,
                packet["x"].as_f64(),
                packet["y"].as_f64(),
                packet["z"].as_f64(),
                packet["radius"].as_f64(),
            ) else {
                return Vec::new();
            };
            let except = packet["except"].as_i64().unwrap_or(-1);
            target
                .players
                .load()
                .iter()
                .filter(|player| i64::from(player.entity_id()) != except)
                .filter(|player| {
                    let position = player.position();
                    let (dx, dy, dz) = (position.x - x, position.y - y, position.z - z);
                    dx * dx + dy * dy + dz * dz < radius * radius
                })
                .cloned()
                .collect()
        }
        Some("tracking") => {
            let Some(id) = packet["entity"]
                .as_i64()
                .and_then(|id| i32::try_from(id).ok())
            else {
                return Vec::new();
            };
            let Some(tracked) = world.entity_tracker.get_tracked_entity(id) else {
                return Vec::new();
            };
            let include_self = packet["self"] == true;
            world
                .players
                .load()
                .iter()
                .filter(|player| {
                    tracked.is_seen_by(&player.gameprofile.id)
                        || (include_self && player.entity_id() == id)
                })
                .cloned()
                .collect()
        }
        _ => Vec::new(),
    }
}

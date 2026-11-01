use std::{
    any::Any,
    collections::{HashMap, HashSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
    /// Whether Java's block entity holds this entity's current data, so tick batches need not send it.
    java_synced: AtomicBool,
}

impl ModBlockEntity {
    #[must_use]
    pub fn load(nbt: &NbtCompound, position: BlockPos) -> Option<Self> {
        let id = pumpkin_data::runtime::block_entity_by_name(nbt.get_string("id")?)?;
        Some(Self {
            id,
            position,
            nbt: Mutex::new(nbt.clone()),
            java_synced: AtomicBool::new(false),
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
            java_synced: AtomicBool::new(false),
        }
    }
}

impl ModBlockEntity {
    /// Takes data Java produced: Java already holds it, and the chunk saves it.
    fn commit(&self, world: &World, nbt: NbtCompound) {
        world.add_block_entity_nbt(self.position, &nbt);
        *self
            .nbt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = nbt;
        self.java_synced.store(true, Ordering::Relaxed);
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
            java_synced: AtomicBool::new(false),
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

struct RustWorld {
    world: Arc<World>,
    handle: i32,
}

/// Small integer ids Java uses to name a world on typed calls, in order of first use.
static WORLD_HANDLES: LazyLock<Mutex<Vec<uuid::Uuid>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn world_handle(world: &World) -> i32 {
    let mut handles = WORLD_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = handles
        .iter()
        .position(|id| *id == world.uuid)
        .unwrap_or_else(|| {
            handles.push(world.uuid);
            handles.len() - 1
        });
    i32::try_from(index).unwrap_or(i32::MAX)
}

impl RustWorld {
    fn leased(world: &Arc<World>) -> Arc<dyn WorldAccess> {
        Arc::new(Self {
            world: world.clone(),
            handle: world_handle(world),
        })
    }

    /// The world a call names, which may be another dimension than the leased one.
    fn resolve(&self, handle: i32) -> Result<std::borrow::Cow<'_, Arc<World>>, String> {
        if handle == self.handle {
            return Ok(std::borrow::Cow::Borrowed(&self.world));
        }
        let id = usize::try_from(handle)
            .ok()
            .and_then(|index| {
                WORLD_HANDLES
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(index)
                    .copied()
            })
            .ok_or("unknown bridge world handle")?;
        let server = self.world.server.upgrade().ok_or("server has stopped")?;
        server
            .worlds
            .load()
            .iter()
            .find(|world| world.uuid == id)
            .cloned()
            .map(std::borrow::Cow::Owned)
            .ok_or_else(|| "unknown bridge world".to_owned())
    }
}

/// Adds what Java caches on its level view: the world's identity, fixed dimension values and this tick's clocks.
fn with_world(world: &World, request: &mut Value) {
    let day_time = world
        .level_time
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .time_of_day;
    request["world"] = json!(world.uuid.to_string());
    request["handle"] = json!(world_handle(world));
    request["minY"] = json!(world.dimension.min_y);
    request["height"] = json!(world.dimension.height);
    request["dimension"] = json!(world.dimension.minecraft_name);
    request["seed"] = json!(world.level.seed.0 as i64);
    request["seaLevel"] = json!(world.sea_level);
    request["time"] = json!(world.get_world_age());
    request["dayTime"] = json!(day_time);
    request["dataFolder"] = json!(
        world
            .level
            .level_folder
            .root_folder
            .join("data")
            .join("neoforge")
            .to_string_lossy()
    );
}

/// Runs a world request on the calling tick thread with the world leased to Java.
fn play_in(world: &Arc<World>, mut request: Value) -> Result<Value, ferrum_neoforge::Error> {
    with_world(world, &mut request);
    bridge::play(RustWorld::leased(world), &request)
}

/// Chunk custom data holding Java's data attachments, saved as `neoforge:attachments`.
const CHUNK_ATTACHMENTS_NAMESPACE: &str = "neoforge";
const CHUNK_ATTACHMENTS_KEY: &str = "attachments";

#[derive(Deserialize)]
struct Request {
    op: String,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    z: i32,
}

impl WorldAccess for RustWorld {
    fn call(&self, request: Value) -> Result<Value, String> {
        let handle = request["handle"]
            .as_i64()
            .and_then(|handle| i32::try_from(handle).ok())
            .unwrap_or(self.handle);
        let world = self.resolve(handle)?;
        let world: &Arc<World> = &world;
        if let Some(result) = entity_op(world, &request) {
            return result;
        }
        let raw = request;
        let request = Request::deserialize(&raw).map_err(|e| e.to_string())?;
        let pos = BlockPos::new(request.x, request.y, request.z);
        match request.op.as_str() {
            "be" => Ok(world.get_block_entity(&pos).map_or(Value::Null, |entity| {
                let mut nbt = NbtCompound::new();
                entity.write_internal(&mut nbt);
                // Java builds its block entity from this, so later batches need not resend it.
                if let Some(be) = entity.as_any().downcast_ref::<ModBlockEntity>() {
                    be.java_synced.store(true, Ordering::Relaxed);
                }
                json!(bridge::encode_nbt(&nbt))
            })),
            "commit_be" => {
                commit_vanilla_block_entity(world, pos, &raw);
                Ok(Value::Null)
            }
            "dirty" => {
                world
                    .level
                    .read_chunk_sync(&pos.chunk_position(), |c| c.mark_dirty(true));
                Ok(Value::Null)
            }
            "chunk_attachments" => Ok(world
                .level
                .read_chunk_sync(&pos.chunk_position(), |chunk| {
                    chunk
                        .get_custom_data(CHUNK_ATTACHMENTS_NAMESPACE, CHUNK_ATTACHMENTS_KEY)
                        .and_then(|tag| tag.extract_compound().map(bridge::encode_nbt))
                })
                .flatten()
                .map_or(Value::Null, |nbt| json!(nbt))),
            "set_chunk_attachments" => {
                let nbt = bridge::decode_nbt(raw["nbt"].as_str().unwrap_or_default())
                    .map_err(|e| e.to_string())?;
                world.level.read_chunk_sync(&pos.chunk_position(), |chunk| {
                    chunk.set_custom_data(
                        CHUNK_ATTACHMENTS_NAMESPACE,
                        CHUNK_ATTACHMENTS_KEY,
                        pumpkin_nbt::tag::NbtTag::Compound(nbt.clone()),
                    );
                });
                Ok(Value::Null)
            }
            // `Level.explode`: Rust's vanilla explosion damages entities and removes blocks, so Java's own would double up.
            "explode" => {
                use crate::world::explosion::ExplosionInteraction;
                let f = |key: &str| f64_of(&raw, key);
                let interaction = match raw["interaction"].as_str().unwrap_or_default() {
                    "NONE" => ExplosionInteraction::None,
                    "BLOCK" => ExplosionInteraction::Block,
                    "MOB" => ExplosionInteraction::Mob,
                    "TRIGGER" => ExplosionInteraction::Trigger,
                    _ => ExplosionInteraction::Tnt,
                };
                world.explode(
                    Vector3::new(f("ex")?, f("ey")?, f("ez")?),
                    f("power")? as f32,
                    interaction,
                );
                Ok(Value::Null)
            }
            "neighbor_update" | "shape_update" => neighbor_op(world, &request.op, pos, &raw),
            "chunk_palettes" => Ok(chunk_palettes(world, pos)),
            "comparator" => {
                world.update_neighbour_for_output_signal(&pos, world.get_block(&pos));
                Ok(Value::Null)
            }
            "update" => {
                let state = world.get_block_state_id(&pos);
                world.send_block_updated(&pos, state, state);
                Ok(Value::Null)
            }
            // Java resolves a biome's name once per Rust biome id, then caches it.
            "biome" => Ok(json!(world.get_biome(&pos).registry_id)),
            _ => Err(format!(
                "unsupported NeoForge world operation: {}",
                request.op
            )),
        }
    }

    fn block_state(&self, world: i32, x: i32, y: i32, z: i32) -> Result<i32, String> {
        let world = self.resolve(world)?;
        let pos = BlockPos::new(x, y, z);
        let state = if world.is_in_height_limit(y) {
            world
                .get_block_state_id_if_loaded(&pos)
                .ok_or_else(|| format!("NeoForge accessed unloaded Rust chunk at {pos:?}"))?
        } else {
            BlockStateId::AIR
        };
        bridge::java_state_raw(state.as_u16())
            .ok_or_else(|| format!("state has no 26.2 mapping: {state}"))
    }

    fn set_block_state(
        &self,
        world: i32,
        (x, y, z): (i32, i32, i32),
        state: i32,
        flags: i32,
    ) -> Result<bool, String> {
        let world = self.resolve(world)?;
        let pos = BlockPos::new(x, y, z);
        let state = u32::try_from(state)
            .ok()
            .and_then(bridge::rust_state)
            .ok_or("Java state has no Rust mapping")?;
        if !world.is_in_build_limit(pos) || !world.level.is_chunk_loaded(&pos.chunk_position()) {
            return Ok(false);
        }
        let flags = translate_flags(u32::try_from(flags).unwrap_or(0));
        Ok(world.set_block_state(&pos, state, flags) != state)
    }

    fn chunk_loaded(&self, world: i32, x: i32, z: i32) -> Result<bool, String> {
        let world = self.resolve(world)?;
        Ok(world
            .level
            .is_chunk_loaded(&pumpkin_util::math::vector2::Vector2::new(x, z)))
    }

    fn height(&self, world: i32, heightmap: i32, x: i32, z: i32) -> Result<i32, String> {
        let world = self.resolve(world)?;
        let map = pumpkin_world::chunk::ChunkHeightmapType::try_from(
            usize::try_from(heightmap).map_err(|e| e.to_string())?,
        )?;
        Ok(world.get_heightmap_height(map, x, z) + 1)
    }

    fn biome(&self, world: i32, x: i32, y: i32, z: i32) -> Result<i32, String> {
        let world = self.resolve(world)?;
        Ok(i32::from(world.get_biome(&BlockPos::new(x, y, z)).id))
    }

    fn light(&self, world: i32, layer: i32, x: i32, y: i32, z: i32) -> Result<i32, String> {
        let world = self.resolve(world)?;
        let pos = BlockPos::new(x, y, z);
        let sky = layer == 1;
        if !world.is_in_height_limit(y) {
            // Above the world is fully sky lit, below it is dark.
            return Ok(i32::from(sky && y >= world.dimension.min_y) * 15);
        }
        let light = if sky {
            world.get_sky_light_level(&pos)
        } else {
            world.get_block_light_level(&pos).unwrap_or(0)
        };
        Ok(i32::from(light))
    }

    fn sky_darken(&self, world: i32) -> Result<i32, String> {
        Ok(self.resolve(world)?.get_sky_darken())
    }

    fn random(&self) -> i32 {
        rand::random::<i32>()
    }
}

/// The distinct block states of each section of the chunk at `pos`, as Java state ids; `null` when it is not loaded.
fn chunk_palettes(world: &World, pos: BlockPos) -> Value {
    world
        .level
        .read_chunk_sync(&pos.chunk_position(), |chunk| {
            let sections = chunk
                .section
                .block_sections
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sections
                .iter()
                .map(|section| {
                    let mut states: Vec<BlockStateId> = Vec::new();
                    for state in section {
                        if !states.contains(&state) {
                            states.push(state);
                        }
                    }
                    states
                        .into_iter()
                        .filter_map(|state| bridge::java_state(state).ok())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .map_or(Value::Null, |sections| json!(sections))
}

/// Level's `NeighborUpdater`: Rust runs neighbour and shape updates, calling back into Java for mod blocks.
fn neighbor_op(world: &Arc<World>, op: &str, pos: BlockPos, raw: &Value) -> Result<Value, String> {
    if op == "neighbor_update" {
        let source = raw["source"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .and_then(bridge::rust_state)
            .ok_or("Java state has no Rust mapping")?;
        world.update_neighbor(&pos, pumpkin_data::Block::from_state_id(source));
    } else {
        let direction = raw["direction"]
            .as_u64()
            .and_then(|index| u8::try_from(index).ok())
            .and_then(pumpkin_data::BlockDirection::from_index)
            .ok_or("bad direction")?;
        let flags = raw["flags"]
            .as_u64()
            .and_then(|flags| u32::try_from(flags).ok())
            .unwrap_or(0);
        world.replace_with_state_for_neighbor_update(&pos, direction, translate_flags(flags));
    }
    Ok(Value::Null)
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
        let mut entry = json!({"x":pos.0.x,"y":pos.0.y,"z":pos.0.z,"state":state});
        // Java keeps its block entity between batches; it only needs data Rust replaced since.
        if let Some(be) = be.as_any().downcast_ref::<ModBlockEntity>()
            && !be.java_synced.swap(true, Ordering::Relaxed)
        {
            let mut nbt = NbtCompound::new();
            be.write_internal(&mut nbt);
            entry["nbt"] = json!(bridge::encode_nbt(&nbt));
        }
        batch.push(entry);
    }
    let mut request = json!({"entities":batch,"retained":retained});
    with_world(world, &mut request);
    let result = bridge::tick(RustWorld::leased(world), &request)?;
    deliver(
        world,
        result["packets"].as_array().map_or(&[][..], Vec::as_slice),
    );
    // Only block entities whose data changed in Java come back.
    let result: Vec<Snapshot> = serde_json::from_value(result["entities"].clone())?;
    for snapshot in result {
        let pos = BlockPos::new(snapshot.x, snapshot.y, snapshot.z);
        let nbt = bridge::decode_nbt(&snapshot.nbt)?;
        commit_block_entity(world, pos, nbt);
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

/// Bound on cached mod crafting lookups; the cache starts over when full.
const MAX_RECIPE_CACHE: usize = 1024;

type CraftReply = Option<(String, Vec<String>)>;

static CRAFTS: LazyLock<Mutex<HashMap<String, CraftReply>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A grid no engine recipe matches goes to Java's `RecipeManager`, where mod recipes live.
pub fn external_craft(
    grid: &dyn pumpkin_inventory::crafting::recipes::RecipeInputInventory,
) -> Option<pumpkin_inventory::crafting::crafting_screen_handler::ExternalCraft> {
    if !bridge::enabled() {
        return None;
    }
    let slots: Vec<String> = (0..grid.size())
        .map(|slot| {
            slot_nbt(&grid.get_stack(slot)).map_or_else(String::new, |nbt| bridge::encode_nbt(&nbt))
        })
        .collect();
    if slots.iter().all(String::is_empty) {
        return None;
    }
    let key = format!("{}|{}", grid.get_width(), slots.join(","));
    let cached = CRAFTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned();
    let reply = if let Some(reply) = cached {
        reply
    } else {
        let request = json!({"op": "craft", "width": grid.get_width(), "height": grid.get_height(), "grid": slots});
        let reply = match bridge::play_unleased(&request) {
            Ok(reply) => reply,
            Err(error) => {
                warn!("NeoForge recipe lookup failed: {error}");
                return None;
            }
        };
        let entry = reply["result"].as_str().map(|result| {
            let remainders = reply["remainders"]
                .as_array()
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|slot| slot.as_str().unwrap_or_default().to_owned())
                .collect();
            (result.to_owned(), remainders)
        });
        let mut cache = CRAFTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() >= MAX_RECIPE_CACHE {
            cache.clear();
        }
        cache.insert(key, entry.clone());
        entry
    };
    let (result, remainders) = reply?;
    let stack = |encoded: &str| {
        bridge::decode_nbt(encoded).map_or_else(
            |_| ItemStack::EMPTY.clone(),
            |nbt| merged_stack(ItemStack::EMPTY.clone(), &nbt),
        )
    };
    let result = stack(&result);
    if result.is_empty() {
        return None;
    }
    Some(
        pumpkin_inventory::crafting::crafting_screen_handler::ExternalCraft {
            result,
            remainders: remainders
                .iter()
                .map(|slot| {
                    if slot.is_empty() {
                        ItemStack::EMPTY.clone()
                    } else {
                        stack(slot)
                    }
                })
                .collect(),
        },
    )
}

/// The item a recipe result names, including mod items.
#[must_use]
pub fn recipe_item(id: &str) -> Option<&'static Item> {
    Item::from_registry_key(id.strip_prefix("minecraft:").unwrap_or(id))
        .or_else(|| pumpkin_data::runtime::item_by_name(id))
}

type CookingCache = HashMap<(u16, u8), Option<&'static pumpkin_data::recipes::CookingRecipe>>;

static COOKING: LazyLock<Mutex<CookingCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// `RecipeManager.getRecipeFor` for one furnace kind: the engine's recipes, then mods' from Java, once per item.
#[must_use]
pub fn cooking_recipe(
    stack: &ItemStack,
    kind: pumpkin_data::recipes::CookingRecipeKind,
) -> Option<&'static pumpkin_data::recipes::CookingRecipe> {
    use pumpkin_data::recipes::CookingRecipeKind as Kind;
    let (index, name) = match kind {
        Kind::Blasting => (0, "blasting"),
        Kind::Smelting => (1, "smelting"),
        Kind::Smoking => (2, "smoking"),
        Kind::CampfireCooking => (3, "campfire_cooking"),
    };
    if let Some(recipe) =
        pumpkin_data::recipes::get_cooking_recipe_with_ingredient(stack.item, kind)
    {
        return Some(recipe);
    }
    if !bridge::enabled() || stack.is_empty() {
        return None;
    }
    let key = (stack.item.id, index);
    if let Some(cached) = COOKING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return *cached;
    }
    let request = json!({"op": "cook", "kind": name, "stack": slot_nbt(stack).map(|nbt| bridge::encode_nbt(&nbt))});
    let reply = match bridge::play_unleased(&request) {
        Ok(reply) => reply,
        Err(error) => {
            warn!("NeoForge cooking recipe lookup failed: {error}");
            return None;
        }
    };
    // Made once per item and kind, so what is leaked is bounded by the item registry.
    let recipe = reply["result"]
        .as_str()
        .filter(|id| recipe_item(id).is_some())
        .map(|result| {
            &*Box::leak(Box::new(pumpkin_data::recipes::CookingRecipe {
                recipe_id: Box::leak(
                    reply["id"]
                        .as_str()
                        .unwrap_or(result)
                        .to_owned()
                        .into_boxed_str(),
                ),
                category: pumpkin_data::recipes::RecipeCategoryTypes::Misc,
                group: None,
                ingredient: pumpkin_data::recipes::RecipeIngredientTypes::Simple(
                    stack.item.registry_key,
                ),
                cookingtime: reply["time"]
                    .as_i64()
                    .and_then(|time| i32::try_from(time).ok())
                    .unwrap_or(200)
                    .max(1),
                experience: reply["experience"].as_f64().unwrap_or(0.0) as f32,
                result: pumpkin_data::recipes::RecipeResultStruct {
                    id: Box::leak(result.to_owned().into_boxed_str()),
                    count: reply["count"]
                        .as_u64()
                        .and_then(|count| u8::try_from(count).ok())
                        .unwrap_or(1),
                },
            }))
        });
    COOKING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, recipe);
    recipe
}

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

/// `ServerPlayerGameMode.releaseUsingItem` for an item Java started using; true when Java had one in use.
pub fn player_release(player: &Player) -> bool {
    if !bridge::enabled() {
        return false;
    }
    player_events(player, &[json!({"type": "release"})])
        .first()
        .is_some_and(|result| result["using"] == true)
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

pub fn right_click(
    player: &Player,
    hand: Hand,
    position: BlockPos,
    face: i32,
    cursor: Vector3<f32>,
    inside: bool,
) -> (bool, bool) {
    if !bridge::enabled() {
        return (true, true);
    }
    let results = player_events(
        player,
        &[
            json!({"type":"right_click", "hand":hand_id(hand), "x":position.0.x,"y":position.0.y,"z":position.0.z,
        "face":face,"cx":cursor.x,"cy":cursor.y,"cz":cursor.z,"inside":inside}),
        ],
    );
    let Some(result) = results.first() else {
        return (true, true);
    };
    if result["canceled"] == true {
        return (false, false);
    }
    (result["block"] != false, result["item"] != false)
}

pub fn toss_item(player: &Player, stack: &ItemStack) -> Option<ItemStack> {
    if !bridge::enabled() {
        return Some(stack.clone());
    }
    let mut nbt = NbtCompound::new();
    stack.write_item_stack(&mut nbt);
    let results = player_events(
        player,
        &[json!({"type":"toss","stack":bridge::encode_nbt(&nbt)})],
    );
    let Some(result) = results.first() else {
        return Some(stack.clone());
    };
    if result["canceled"] == true {
        return None;
    }
    Some(
        result["stack"]
            .as_str()
            .and_then(|encoded| bridge::decode_nbt(encoded).ok())
            .map_or_else(|| stack.clone(), |nbt| merged_stack(stack.clone(), &nbt)),
    )
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

/// Bound on the container slots one vanilla block entity write-back may carry.
const MAX_CONTAINER_SLOTS: usize = 256;

/// Takes what a mod changed in a vanilla block entity Java borrowed. Container slots are written into Rust's own
/// inventory, which open screens share; other data replaces the block entity. A type mismatch is skipped.
fn commit_vanilla_block_entity(world: &Arc<World>, position: BlockPos, request: &Value) {
    let Some(entity) = world.get_block_entity(&position) else {
        return;
    };
    let id = request["id"].as_str().unwrap_or_default();
    if entity.resource_location() != id || entity.as_any().is::<ModBlockEntity>() {
        warn!(
            "Ignoring NeoForge write to a {id} block entity at {position:?}; Rust holds another type"
        );
        return;
    }
    if let Some(slots) = request["slots"].as_array()
        && slots.len() <= MAX_CONTAINER_SLOTS
        && let Some(inventory) = entity.clone().get_inventory()
        && inventory.size() == slots.len()
    {
        let mut changed = false;
        for (slot, encoded) in slots.iter().enumerate() {
            let encoded = encoded.as_str().unwrap_or_default();
            let current = inventory.get_stack(slot);
            let stack = if encoded.is_empty() {
                ItemStack::EMPTY.clone()
            } else {
                match bridge::decode_nbt(encoded) {
                    Ok(nbt) => merged_stack(current.clone(), &nbt),
                    Err(error) => {
                        warn!("Skipping a NeoForge container slot at {position:?}: {error}");
                        continue;
                    }
                }
            };
            if slot_nbt(&stack) != slot_nbt(&current) {
                inventory.set_stack(slot, stack);
                changed = true;
            }
        }
        if changed {
            inventory.mark_dirty();
            world.update_neighbour_for_output_signal(&position, world.get_block(&position));
            world
                .level
                .read_chunk_sync(&position.chunk_position(), |c| c.mark_dirty(true));
        }
        return;
    }
    let Some(nbt) = request["nbt"]
        .as_str()
        .and_then(|nbt| bridge::decode_nbt(nbt).ok())
    else {
        return;
    };
    if nbt.get_string("id") != Some(id) {
        return;
    }
    match crate::block::entities::block_entity_from_nbt(&nbt) {
        Some(replacement) if replacement.get_position() == position => {
            world.add_block_entity(replacement);
        }
        _ => warn!("NeoForge wrote {id} data Rust cannot load at {position:?}"),
    }
}

/// Replaces a Rust block entity's data with what Java holds after it changed it.
fn commit_block_entity(world: &World, position: BlockPos, nbt: NbtCompound) {
    let Some(entity) = world.get_block_entity(&position) else {
        return;
    };
    let Some(be) = entity.as_any().downcast_ref::<ModBlockEntity>() else {
        return;
    };
    if nbt.get_string("id") == Some(be.id)
        && nbt.get_int("x") == Some(position.0.x)
        && nbt.get_int("y") == Some(position.0.y)
        && nbt.get_int("z") == Some(position.0.z)
    {
        be.commit(world, nbt);
    } else {
        warn!("Ignoring NeoForge block entity data for another identity at {position:?}");
    }
}

/// `BlockItem.place` for a modded block Rust just set: the stack's block entity data and components reach the Java
/// block entity, and the block's `setPlacedBy` runs.
pub fn block_placed(player: &Player, position: BlockPos, stack: &ItemStack) {
    let world = player.world();
    if !modded_block_at(&world, &position) {
        return;
    }
    let mut nbt = NbtCompound::new();
    stack.write_item_stack(&mut nbt);
    let entity = player.get_entity();
    let request = json!({
        "op": "block_placed", "uuid": player.gameprofile.id.to_string(),
        "x": position.0.x, "y": position.0.y, "z": position.0.z, "stack": bridge::encode_nbt(&nbt),
        "yaw": entity.yaw.load(), "pitch": entity.pitch.load(),
    });
    match play_in(&world, request) {
        Ok(reply) => {
            if let Some(nbt) = reply["nbt"]
                .as_str()
                .and_then(|nbt| bridge::decode_nbt(nbt).ok())
            {
                commit_block_entity(&world, position, nbt);
            }
        }
        Err(error) => warn!("NeoForge block placement failed at {position:?}: {error}"),
    }
}

/// Tool rules of a mod item (or any tool on a modded block) live in Java: its `isCorrectToolForDrops` and
/// `getDestroySpeed` overrides do not need a tool component. `None` when Rust decides.
fn tool_rules(
    world: &Arc<World>,
    state: &pumpkin_data::BlockState,
    tool: &ItemStack,
) -> Option<Value> {
    if !bridge::enabled()
        || tool.is_empty()
        || !(is_modded(tool) || pumpkin_data::runtime::state_block(state.id).is_some())
    {
        return None;
    }
    let java_state = bridge::java_state(state.id).ok()?;
    let mut nbt = NbtCompound::new();
    tool.write_item_stack(&mut nbt);
    let request = json!({
        "op": "correct_tool",
        "state": java_state, "tool": bridge::encode_nbt(&nbt),
    });
    match play_in(world, request) {
        Ok(reply) => Some(reply),
        Err(error) => {
            warn!("NeoForge tool check failed: {error}");
            None
        }
    }
}

/// Whether the tool is the right one for the block: `None` for vanilla tools on vanilla blocks.
pub fn correct_tool_for_drops(
    world: &Arc<World>,
    state: &pumpkin_data::BlockState,
    tool: &ItemStack,
) -> Option<bool> {
    tool_rules(world, state, tool)?["correct"].as_bool()
}

/// Mining speed of a mod tool on a block, from its Java `getDestroySpeed`.
pub fn destroy_speed(
    world: &Arc<World>,
    state: &pumpkin_data::BlockState,
    tool: &ItemStack,
) -> Option<f32> {
    if !is_modded(tool) {
        return None;
    }
    tool_rules(world, state, tool)?["speed"]
        .as_f64()
        .map(|speed| speed as f32)
}

/// The items a modded block drops when broken, computed by Java from the block, its block entity and the tool;
/// `None` for blocks Rust drops itself.
pub fn block_drops(
    world: &Arc<World>,
    block: &pumpkin_data::Block,
    position: &BlockPos,
    tool: Option<&ItemStack>,
) -> Option<Vec<ItemStack>> {
    if !bridge::enabled() || pumpkin_data::runtime::block(block.id).is_none() {
        return None;
    }
    let mut request = json!({
        "op": "block_drops",
        "x": position.0.x, "y": position.0.y, "z": position.0.z,
    });
    if let Some(tool) = tool {
        let mut nbt = NbtCompound::new();
        tool.write_item_stack(&mut nbt);
        request["tool"] = json!(bridge::encode_nbt(&nbt));
    }
    let reply = match play_in(world, request) {
        Ok(reply) => reply,
        Err(error) => {
            warn!("NeoForge block drops failed at {position:?}: {error}");
            return None;
        }
    };
    Some(
        reply["drops"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|encoded| bridge::decode_nbt(encoded.as_str()?).ok())
            .map(|nbt| merged_stack(ItemStack::EMPTY.clone(), &nbt))
            .filter(|stack| !stack.is_empty())
            .collect(),
    )
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
    server_tick_event(server, "tick_post");
}

pub fn server_tick_start(server: &Server) {
    if !bridge::enabled() {
        return;
    }
    pumpkin_inventory::crafting::crafting_screen_handler::set_external_crafting(external_craft);
    for world in server.worlds.load().iter() {
        let request = json!({"op":"world",});
        if let Err(error) = play_in(world, request) {
            error!("NeoForge world initialization failed: {error}");
        }
    }
    server_tick_event(server, "tick_pre");
}

fn server_tick_event(server: &Server, op: &str) {
    if !bridge::enabled() {
        return;
    }
    let worlds = server.worlds.load();
    let Some(world) = worlds.first() else {
        return;
    };
    let request = json!({"op": op});
    match play_in(world, request) {
        Ok(reply) => deliver(
            world,
            reply["packets"].as_array().map_or(&[][..], Vec::as_slice),
        ),
        Err(error) => error!("NeoForge server tick failed: {error}"),
    }
}

pub fn level_tick_event(world: &Arc<World>, pre: bool) {
    if !bridge::enabled() {
        return;
    }
    let mut request = json!({"op": if pre { "level_pre" } else { "level_post" },});
    if pre {
        let chunks: Vec<_> = world
            .level
            .loaded_chunks
            .iter()
            .map(|entry| [entry.key().x, entry.key().y])
            .collect();
        request["chunks"] = json!(chunks);
    }
    match play_in(world, request) {
        Ok(reply) => deliver(
            world,
            reply["packets"].as_array().map_or(&[][..], Vec::as_slice),
        ),
        Err(error) => error!("NeoForge level tick failed: {error}"),
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
    let mut events = Vec::new();
    events.extend(tracking_event(player));
    events.push(json!({"type": "tick"}));
    if menu_open(player) {
        events.push(json!({"type": "menu_tick"}));
    }
    if !events.is_empty() {
        player_events(player, &events);
    }
}

/// Rust opens its own screen: the menu Java has open closes first, as vanilla's `openMenu` closes the current one.
pub fn close_java_menu(player: &Player) {
    if bridge::enabled() && menu_open(player) {
        player_events(player, &[json!({"type": "close_menu"})]);
    }
}

pub fn player_left(player: &Player) {
    if !bridge::enabled() {
        return;
    }
    // `ServerPlayer.disconnect`: the open menu closes, returning carried and crafting items to the inventory Rust
    // saves next, and mods see the logout.
    player_events(player, &[json!({"type": "leave"})]);
    open_menus().remove(&player.gameprofile.id);
    SYNCED_SLOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&player.gameprofile.id);
    WATCHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&player.gameprofile.id);
    let request = json!({"op": "leave", "uuid": player.gameprofile.id.to_string()});
    // Mods' chunk watcher hooks query the world while the player's tracked chunks are released.
    if let Err(error) = bridge::play(RustWorld::leased(&player.world()), &request) {
        warn!("Failed to release NeoForge player view: {error}");
    }
}

/// `NeoForge` data attachments of players (mod state such as radiation), as Java last saved them, by player.
static PLAYER_ATTACHMENTS: LazyLock<Mutex<HashMap<uuid::Uuid, NbtCompound>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Key of `AttachmentHolder.ATTACHMENTS_NBT_KEY` in Java's entity data and in the player file.
const ATTACHMENTS_KEY: &str = "neoforge:attachments";

fn player_attachments() -> std::sync::MutexGuard<'static, HashMap<uuid::Uuid, NbtCompound>> {
    PLAYER_ATTACHMENTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn write_player_attachments(player: uuid::Uuid, nbt: &mut NbtCompound) {
    if let Some(attachments) = player_attachments().get(&player) {
        nbt.put_compound(ATTACHMENTS_KEY, attachments.clone());
    }
}

pub fn read_player_attachments(player: uuid::Uuid, nbt: &NbtCompound) {
    let mut attachments = player_attachments();
    match nbt.get_compound(ATTACHMENTS_KEY) {
        Some(saved) => attachments.insert(player, saved.clone()),
        None => attachments.remove(&player),
    };
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

/// One player step at a time: network threads (mod payloads, item use) and the tick thread each snapshot the
/// inventory, run Java and write the result back, and interleaved steps would overwrite each other with stale stacks.
static EXCHANGE: Mutex<()> = Mutex::new(());

thread_local! {
    static IN_EXCHANGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn exchange(player: &Player, events: &[Value]) -> Result<Vec<Value>, String> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            IN_EXCHANGE.set(false);
        }
    }
    if IN_EXCHANGE.get() {
        return exchange_locked(player, events);
    }
    let _guard = EXCHANGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    IN_EXCHANGE.set(true);
    let _reset = Reset;
    exchange_locked(player, events)
}

/// Per player, each inventory slot's NBT as Java last saw it (`None` when empty); absent until Java holds a full
/// copy. Compared as NBT because a compound's encoding has no stable key order.
static SYNCED_SLOTS: LazyLock<Mutex<HashMap<uuid::Uuid, Vec<Option<NbtCompound>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn slot_nbt(stack: &ItemStack) -> Option<NbtCompound> {
    if stack.is_empty() {
        return None;
    }
    let mut nbt = NbtCompound::new();
    stack.write_item_stack(&mut nbt);
    Some(nbt)
}

/// The inventory slots Java does not hold yet, by slot, with "" for an empty slot. `full` sends every non-empty
/// slot for a Java view that has no copy; either way Java holds the current inventory afterwards.
fn inventory_delta(player: &Player, full: bool) -> Map<String, Value> {
    let inventory = player.inventory();
    let mut synced = SYNCED_SLOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let known = synced
        .entry(player.gameprofile.id)
        .or_insert_with(|| vec![None; OFF_HAND_SLOT + 1]);
    let mut delta = Map::new();
    for (slot, known) in known.iter_mut().enumerate() {
        let nbt = slot_nbt(&inventory.get_slot(slot));
        if (full && nbt.is_some()) || nbt != *known {
            let data = nbt.as_ref().map_or_else(String::new, bridge::encode_nbt);
            delta.insert(slot.to_string(), json!(data));
            *known = nbt;
        }
    }
    delta
}

/// Full inventory for a Java player view that lost its copy, see `PlayerBridge.sync`.
fn player_inventory(world: &Arc<World>, request: &Value) -> Result<Value, String> {
    let id = request["uuid"]
        .as_str()
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .ok_or("player inventory request without a player")?;
    let player = player_by_uuid(world, id).ok_or("unknown player")?;
    Ok(Value::Object(inventory_delta(&player, true)))
}

fn exchange_locked(player: &Player, events: &[Value]) -> Result<Vec<Value>, String> {
    let world = player.world();
    let inventory = player.inventory();
    let full = !SYNCED_SLOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key(&player.gameprofile.id);
    let snapshot = inventory_delta(player, full);
    let (yaw, pitch) = player.rotation();
    let position = player.position();
    let request = json!({
        "op": "sync", "uuid": player.gameprofile.id.to_string(), "name": player.gameprofile.name,
        "entityId": player.entity_id(),
        "x": position.x, "y": position.y, "z": position.z, "yaw": yaw, "pitch": pitch,
        "gamemode": player.gamemode.load() as i32, "selected": inventory.get_selected_slot(),
        "sneaking": player.get_entity().is_sneaking(),
        "health": player.living_entity.health.load(), "fallDistance":player.living_entity.fall_distance.load(),
        "ground": player.get_entity().on_ground.load(Ordering::Relaxed),
        "attackTicks": player.last_attacked_ticks.load(Ordering::Relaxed),
        "vx": player.get_entity().velocity.load().x, "vy":player.get_entity().velocity.load().y, "vz":player.get_entity().velocity.load().z,
        "abilities": abilities_json(player),
        "inventory": snapshot, "full": full, "events": events,
    });
    let mut request = request;
    if full && let Some(attachments) = player_attachments().get(&player.gameprofile.id) {
        // A new Java view starts with the attachments the player file holds.
        let mut wrapped = NbtCompound::new();
        wrapped.put_compound(ATTACHMENTS_KEY, attachments.clone());
        request["attachments"] = json!(bridge::encode_nbt(&wrapped));
    }
    let reply = play_in(&world, request).map_err(|e| {
        // Java may not have taken the slots sent; the next step sends all of them.
        SYNCED_SLOTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&player.gameprofile.id);
        e.to_string()
    })?;
    if let Some(saved) = reply["attachments"]
        .as_str()
        .and_then(|saved| bridge::decode_nbt(saved).ok())
    {
        read_player_attachments(player.gameprofile.id, &saved);
    }
    if let Some(velocity) = reply["velocity"]
        .as_array()
        .filter(|velocity| velocity.len() == 3)
    {
        let values: Option<Vec<_>> = velocity
            .iter()
            .map(|value| value.as_f64().filter(|value| value.is_finite()))
            .collect();
        if let Some(values) = values {
            player.set_velocity(Vector3::new(values[0], values[1], values[2]));
        }
    }
    if let Some(ignore) = reply["impulse"]["ignore"].as_bool() {
        player
            .living_entity
            .current_impulse_impact_y
            .store(if ignore {
                reply["impulse"]["y"].as_f64().filter(|y| y.is_finite())
            } else {
                None
            });
        player.living_entity.impulse_grace_ticks.store(
            reply["impulse"]["grace"].as_i64().unwrap_or(0).clamp(0, 40) as i32,
            Ordering::Relaxed,
        );
    }
    if let Some(changes) = reply["inventory"].as_object() {
        for (slot, encoded) in changes {
            let slot: usize = slot.parse().map_err(|_| "bad inventory slot".to_owned())?;
            if slot > OFF_HAND_SLOT {
                return Err(format!("Java changed unsupported inventory slot {slot}"));
            }
            let nbt = bridge::decode_nbt(encoded.as_str().unwrap_or_default())
                .map_err(|e| e.to_string())?;
            let stack = merged_stack(inventory.get_slot(slot), &nbt);
            // Java holds its own copy of the changed stack, so this is not news to it next step.
            if let Some(known) = SYNCED_SLOTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_mut(&player.gameprofile.id)
            {
                known[slot] = slot_nbt(&stack);
            }
            inventory.set_slot(slot, stack);
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

/// Run the transformed Java attack for mod weapons, including their item hooks and damage source.
pub fn player_attack(player: &Player, target: &dyn EntityBase) -> bool {
    if !bridge::enabled() {
        return false;
    }
    // Mod weapons, and any hit on a mod entity, whose health only Java holds.
    let modded = modded_item_in_hand(player, Hand::Right) || as_mod_entity(target).is_some();
    let results = player_events(
        player,
        &[
            json!({"type": if modded { "attack" } else { "attack_event" }, "target":target.get_entity().entity_id}),
        ],
    );
    modded
        || results
            .first()
            .is_some_and(|result| result["canceled"] == true)
}

/// Runs vanilla's `LivingEntity.postPiercingAttack` after a mod spear's jab, where mods hook their lunge.
pub fn player_piercing_attack(player: &Player) {
    if modded_item_in_hand(player, Hand::Right) {
        player_events(player, &[json!({"type": "piercing"})]);
    }
}

fn living_event(caller: &dyn EntityBase, mut event: Value) -> Option<Value> {
    if !bridge::enabled() {
        return None;
    }
    if !bridge::in_lease()
        && let Some(player) = caller.get_player()
    {
        return player_events(player, &[event]).into_iter().next();
    }
    let world = caller.get_entity().world.load_full();
    event["event"] = event["type"].clone();
    event["op"] = json!("living_event");
    event["entity"] = json!(caller.get_entity().entity_id);
    with_world(&world, &mut event);
    let result = if bridge::in_lease() {
        bridge::play_unleased(&event)
    } else {
        bridge::play(RustWorld::leased(&world), &event)
    };
    match result {
        Ok(reply) => {
            deliver(
                &world,
                reply["packets"].as_array().map_or(&[][..], Vec::as_slice),
            );
            Some(reply)
        }
        Err(error) => {
            error!("NeoForge living event failed: {error}");
            None
        }
    }
}

fn damage_event(
    kind: &str,
    damage_type: pumpkin_data::damage::DamageType,
    source: Option<&dyn EntityBase>,
    cause: Option<&dyn EntityBase>,
) -> Value {
    let name = pumpkin_data::registry::REGISTRY_V_26_2
        .iter()
        .find(|registry| registry.registry_id == "damage_type")
        .and_then(|registry| registry.entries.get(usize::from(damage_type.id)))
        .map_or("generic", |entry| entry.name);
    json!({"type":kind,"damageType":format!("minecraft:{name}"),
        "direct":source.map_or(-1,|entity|entity.get_entity().entity_id),
        "attacker":cause.map_or(-1,|entity|entity.get_entity().entity_id)})
}

thread_local! {
    static DEATH_DROPS: std::cell::RefCell<Vec<Vec<Arc<dyn EntityBase>>>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub struct DeathDrops;

impl DeathDrops {
    #[must_use]
    pub fn capture() -> Option<Self> {
        if !bridge::enabled() {
            return None;
        }
        DEATH_DROPS.with(|drops| drops.borrow_mut().push(Vec::new()));
        Some(Self)
    }

    pub fn finish(
        self,
        caller: &dyn EntityBase,
        damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
        recently_hit: bool,
    ) {
        let drops = DEATH_DROPS.with(|drops| drops.borrow_mut().pop().unwrap_or_default());
        std::mem::forget(self);
        let mut event = damage_event("drops", damage_type, source, cause);
        let items: Vec<_> = drops.iter().filter_map(|entity| {
            let item = entity.cast_any().downcast_ref::<ItemEntity>()?;
            let mut nbt = NbtCompound::new();
            item.get_item_stack().lock().unwrap_or_else(std::sync::PoisonError::into_inner).write_item_stack(&mut nbt);
            let position = entity.get_entity().pos.load();
            let velocity = entity.get_entity().velocity.load();
            Some(json!({"stack":bridge::encode_nbt(&nbt), "x":position.x,"y":position.y,"z":position.z,
                "vx":velocity.x,"vy":velocity.y,"vz":velocity.z,"delay":item.get_pickup_delay()}))
        }).collect();
        event["drops"] = json!(items);
        event["recentlyHit"] = json!(recently_hit);
        let world = caller.get_entity().world.load_full();
        if let Some(reply) = living_event(caller, event)
            && let Some(changes) = reply["drops"].as_array()
        {
            for drop in changes {
                if let Err(error) = spawn_item(&world, drop) {
                    warn!("NeoForge death drop failed: {error}");
                }
            }
        } else {
            for drop in drops {
                world.spawn_entity(drop);
            }
        }
    }
}

impl Drop for DeathDrops {
    fn drop(&mut self) {
        DEATH_DROPS.with(|drops| {
            drops.borrow_mut().pop();
        });
    }
}

pub fn capture_death_drop(entity: &Arc<dyn EntityBase>) -> bool {
    if !entity.cast_any().is::<ItemEntity>() {
        return false;
    }
    DEATH_DROPS.with(|drops| {
        let mut drops = drops.borrow_mut();
        drops.last_mut().is_some_and(|capture| {
            capture.push(entity.clone());
            true
        })
    })
}

pub fn incoming_damage(
    caller: &dyn EntityBase,
    amount: f32,
    damage_type: pumpkin_data::damage::DamageType,
    source: Option<&dyn EntityBase>,
    cause: Option<&dyn EntityBase>,
) -> Option<f32> {
    let mut event = damage_event("hurt", damage_type, source, cause);
    event["amount"] = json!(amount);
    let Some(result) = living_event(caller, event) else {
        return Some(amount);
    };
    if result["canceled"] == true {
        return None;
    }
    Some(
        result["amount"]
            .as_f64()
            .filter(|amount| amount.is_finite() && *amount >= 0.0)
            .unwrap_or(f64::from(amount)) as f32,
    )
}

pub fn prevent_death(
    caller: &dyn EntityBase,
    damage_type: pumpkin_data::damage::DamageType,
    source: Option<&dyn EntityBase>,
    cause: Option<&dyn EntityBase>,
) -> bool {
    let Some(result) = living_event(caller, damage_event("death", damage_type, source, cause))
    else {
        return false;
    };
    if result["canceled"] != true {
        return false;
    }
    if let Some(living) = caller.get_living_entity() {
        living.set_health(result["health"].as_f64().unwrap_or(1.0).max(1.0) as f32);
    }
    true
}

pub fn player_break(player: &Player, position: BlockPos, state: BlockStateId, pre: bool) -> bool {
    if !bridge::enabled() {
        return false;
    }
    let Ok(state) = bridge::java_state(state) else {
        return false;
    };
    let results = player_events(
        player,
        &[
            json!({"type":if pre { "break_pre" } else { "mine" }, "x":position.0.x,"y":position.0.y,"z":position.0.z,"state":state}),
        ],
    );
    results
        .first()
        .is_some_and(|result| result["canceled"] == true)
}

/// Runs a mod block callback in Java, nested in the lease when one is active.
fn mod_block_op(world: &Arc<World>, mut request: Value) -> Option<Value> {
    with_world(world, &mut request);
    let result = if bridge::in_lease() {
        bridge::play_unleased(&request)
    } else {
        bridge::play(RustWorld::leased(world), &request)
    };
    match result {
        Ok(reply) => {
            deliver(
                world,
                reply["packets"].as_array().map_or(&[][..], Vec::as_slice),
            );
            Some(reply)
        }
        Err(error) => {
            warn!("NeoForge block callback {} failed: {error}", request["op"]);
            None
        }
    }
}

fn block_request(op: &str, position: BlockPos) -> Value {
    json!({"op": op, "x": position.0.x, "y": position.0.y, "z": position.0.z})
}

fn modded_state(state: BlockStateId) -> bool {
    pumpkin_data::runtime::state_block(state).is_some()
}

/// `LevelChunk.setBlockState`: `BlockEntity.preRemoveSideEffects` of a mod block that is being replaced, such as a
/// machine dropping its inventory, while Rust still holds the block entity's data.
pub fn block_replacing(
    world: &Arc<World>,
    position: BlockPos,
    old: BlockStateId,
    flags: BlockFlags,
) {
    if !bridge::enabled()
        || !modded_state(old)
        || flags.contains(BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK)
        || pumpkin_data::Block::from_state_id(old)
            == pumpkin_data::Block::from_state_id(world.get_block_state_id(&position))
    {
        return;
    }
    let Ok(old) = bridge::java_state(old) else {
        return;
    };
    let mut request = block_request("block_removing", position);
    request["old"] = json!(old);
    mod_block_op(world, request);
}

/// What `Level.setBlock` runs for a mod block after the block changed: `onPlace` of the new one and
/// `affectNeighborsAfterRemoval` of the old one.
pub fn block_replaced(
    world: &Arc<World>,
    position: BlockPos,
    old: BlockStateId,
    new: BlockStateId,
    flags: BlockFlags,
) {
    if !bridge::enabled()
        || pumpkin_data::Block::from_state_id(old) == pumpkin_data::Block::from_state_id(new)
    {
        return;
    }
    let moved = flags.contains(BlockFlags::MOVED);
    let neighbors = modded_state(old) && (flags.contains(BlockFlags::NOTIFY_NEIGHBORS) || moved);
    let place = modded_state(new) && !flags.contains(BlockFlags::SKIP_BLOCK_ADDED_CALLBACK);
    if !(neighbors || place) {
        return;
    }
    let (Ok(old), Ok(new)) = (bridge::java_state(old), bridge::java_state(new)) else {
        return;
    };
    let mut request = block_request("block_changed", position);
    request["old"] = json!(old);
    request["new"] = json!(new);
    request["neighbors"] = json!(neighbors);
    request["place"] = json!(place);
    request["moved"] = json!(moved);
    mod_block_op(world, request);
}

/// `BlockState.handleNeighborChanged` of a mod block.
pub fn neighbor_changed(world: &Arc<World>, position: BlockPos, source: &pumpkin_data::Block) {
    if !bridge::enabled() {
        return;
    }
    let Ok(source) = bridge::java_state(source.default_state.id) else {
        return;
    };
    let mut request = block_request("neighbor_changed", position);
    request["source"] = json!(source);
    mod_block_op(world, request);
}

/// Offers one item to a mod block's item capability on `side` (a Java direction index): `Some(true)` when it took
/// the item, `None` when the block has no item handler there.
pub fn insert_item(
    world: &Arc<World>,
    position: BlockPos,
    side: u8,
    stack: &ItemStack,
) -> Option<bool> {
    let mut nbt = NbtCompound::new();
    stack.write_item_stack(&mut nbt);
    let mut request = block_request("item_insert", position);
    request["side"] = json!(side);
    request["stack"] = json!(bridge::encode_nbt(&nbt));
    let reply = mod_block_op(world, request)?;
    (reply["handler"] == true).then(|| reply["inserted"].as_i64().unwrap_or(0) > 0)
}

/// Takes one item out of a mod block's item capability on its bottom side. Without `take` it only names the item
/// that would come out; with it, it removes one of that item and returns it.
pub fn extract_item(
    world: &Arc<World>,
    position: BlockPos,
    take: Option<&ItemStack>,
) -> Option<ItemStack> {
    let mut request = block_request("item_extract", position);
    request["side"] = json!(0);
    if let Some(take) = take {
        let mut nbt = NbtCompound::new();
        take.write_item_stack(&mut nbt);
        request["stack"] = json!(bridge::encode_nbt(&nbt));
    }
    let reply = mod_block_op(world, request)?;
    let nbt = bridge::decode_nbt(reply["stack"].as_str()?).ok()?;
    let stack = merged_stack(ItemStack::EMPTY.clone(), &nbt);
    (!stack.is_empty()).then_some(stack)
}

/// `BlockBehaviour.updateShape` of a mod block after its neighbour in `direction` changed; `None` keeps the state.
pub fn update_shape(
    world: &Arc<World>,
    position: BlockPos,
    state: BlockStateId,
    direction: pumpkin_data::BlockDirection,
    neighbor: BlockStateId,
) -> Option<BlockStateId> {
    if !bridge::enabled() {
        return None;
    }
    let mut request = block_request("update_shape", position);
    request["state"] = json!(bridge::java_state(state).ok()?);
    request["neighbor"] = json!(bridge::java_state(neighbor).ok()?);
    request["direction"] = json!(direction.to_index());
    let reply = mod_block_op(world, request)?;
    reply["state"]
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .and_then(bridge::rust_state)
}

/// `BlockItem.getPlacementState` for a mod block: Java's `getStateForPlacement` (facing, connections) and
/// `canSurvive`. `Some(None)` when the block refuses the spot, `None` when Java gave no answer.
pub fn placement_state(
    player: &Player,
    block: &pumpkin_data::Block,
    use_item_on: &pumpkin_protocol::java::server::play::SUseItemOn,
    target: BlockPos,
    stack: &ItemStack,
) -> Option<Option<BlockStateId>> {
    if !bridge::enabled() || pumpkin_data::runtime::block(block.id).is_none() {
        return None;
    }
    let world = player.world();
    let mut nbt = NbtCompound::new();
    stack.write_item_stack(&mut nbt);
    let clicked = use_item_on.position;
    let entity = player.get_entity();
    let request = json!({
        "op": "placement_state", "uuid": player.gameprofile.id.to_string(),
        "x": clicked.0.x, "y": clicked.0.y, "z": clicked.0.z,
        "px": target.0.x, "py": target.0.y, "pz": target.0.z,
        "face": use_item_on.face.0, "hand": use_item_on.hand.0, "inside": use_item_on.inside_block,
        "cx": use_item_on.cursor_pos.x, "cy": use_item_on.cursor_pos.y, "cz": use_item_on.cursor_pos.z,
        "yaw": entity.yaw.load(), "pitch": entity.pitch.load(),
        "block": bridge::java_state(block.default_state.id).ok()?,
        "stack": bridge::encode_nbt(&nbt),
    });
    let reply = match play_in(&world, request) {
        Ok(reply) => reply,
        Err(error) => {
            warn!("NeoForge placement state failed at {target:?}: {error}");
            return None;
        }
    };
    let state = reply["state"].as_i64()?;
    if state < 0 {
        return Some(None);
    }
    u32::try_from(state)
        .ok()
        .and_then(bridge::rust_state)
        .map(Some)
}

/// `BlockState.tick` of a mod block whose scheduled tick came due.
pub fn scheduled_tick(world: &Arc<World>, position: BlockPos) {
    if bridge::enabled() {
        mod_block_op(world, block_request("scheduled_tick", position));
    }
}

/// Random tick samples that landed on mod blocks; Java runs them in one call.
pub fn random_ticks(world: &Arc<World>, positions: &[BlockPos]) {
    if bridge::enabled() && !positions.is_empty() {
        let positions: Vec<_> = positions
            .iter()
            .map(|p| json!([p.0.x, p.0.y, p.0.z]))
            .collect();
        mod_block_op(world, json!({"op": "random_ticks", "positions": positions}));
    }
}

/// A block tick Java code scheduled, kept in Rust's tick queue.
fn schedule_tick(world: &Arc<World>, request: &Value) -> Result<Value, String> {
    let position = BlockPos::new(
        i32::try_from(request["x"].as_i64().unwrap_or_default()).map_err(|e| e.to_string())?,
        i32::try_from(request["y"].as_i64().unwrap_or_default()).map_err(|e| e.to_string())?,
        i32::try_from(request["z"].as_i64().unwrap_or_default()).map_err(|e| e.to_string())?,
    );
    let state = request["state"]
        .as_u64()
        .and_then(|state| u32::try_from(state).ok())
        .and_then(bridge::rust_state)
        .ok_or("scheduled tick for a block Rust does not know")?;
    let block = pumpkin_data::Block::from_state_id(state);
    if request["query"] == true {
        return Ok(json!(world.is_block_tick_scheduled(&position, block)));
    }
    // Rust's queue holds delays up to 255 ticks.
    let delay =
        u8::try_from(request["delay"].as_i64().unwrap_or(1).clamp(1, 255)).unwrap_or(u8::MAX);
    let priority = pumpkin_world::tick::TickPriority::values()
        .into_iter()
        .find(|priority| *priority as i64 == request["priority"].as_i64().unwrap_or(0))
        .unwrap_or(pumpkin_world::tick::TickPriority::Normal);
    world.schedule_block_tick(block, position, delay, priority);
    Ok(Value::Null)
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
    if count <= 0 {
        return ItemStack::EMPTY.clone();
    }
    let mut stack = if current.item.id == item.id && !current.is_empty() {
        current
    } else {
        ItemStack::new(0, item)
    };
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
    if let Some(components) = components {
        for (name, data) in &components.child_tags {
            let removed = name.starts_with('!');
            let name = name.trim_start_matches('!');
            let Some(id) = DataComponent::try_from_name(name) else {
                continue;
            };
            if matches!(id, DataComponent::CustomData | DataComponent::Damage) {
                continue;
            }
            if removed {
                stack.remove_data_component(id);
            } else if let Some(value) = pumpkin_data::data_component_impl::read_data(id, data) {
                stack.patch.retain(|(existing, _)| *existing != id);
                stack.patch.push((id, Some(value)));
            }
        }
    }
    stack.item_count = u8::try_from(count)
        .unwrap_or(u8::MAX)
        .min(stack.get_max_stack_size());
    stack
}

/// A mod entity Java spawned and ticks: Rust owns its identity, position mirror and tracking.
pub struct ModEntity {
    entity: Entity,
    /// Encoded 26.2 set-entity-data packet with the entity's non-default synched data, for new viewers.
    metadata: Mutex<Option<Vec<u8>>>,
    /// Java's latest save of the entity (`saveWithoutId`), written into the chunk with Rust's own fields.
    java_nbt: Mutex<Option<NbtCompound>>,
    /// Whether Java holds an instance; an entity loaded from a chunk gets one on its first entity pass.
    in_java: AtomicBool,
}

/// Where a mod entity's Java data sits in its chunk NBT.
const JAVA_ENTITY_DATA: &str = "neoforge:java";

/// A mod entity read from chunk NBT; Java rebuilds its instance from the saved data on the next entity pass.
#[must_use]
pub fn load_mod_entity(
    entity_type: &'static EntityType,
    position: Vector3<f64>,
    nbt: &NbtCompound,
    world: &Arc<World>,
) -> Arc<dyn EntityBase> {
    let uuid = nbt.get_uuid("UUID").unwrap_or_else(uuid::Uuid::new_v4);
    let entity = Entity::from_uuid(uuid, world.clone(), position, entity_type);
    entity.read_nbt_non_mut(nbt);
    Arc::new(ModEntity {
        entity,
        metadata: Mutex::new(None),
        java_nbt: Mutex::new(nbt.get_compound(JAVA_ENTITY_DATA).cloned()),
        in_java: AtomicBool::new(false),
    })
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
    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.write_nbt(nbt);
        nbt.put_string("id", self.entity.entity_type.resource_name.to_owned());
        if let Some(java) = self
            .java_nbt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            nbt.put_compound(JAVA_ENTITY_DATA, java);
        }
    }

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

    /// Java owns the entity's health and hurt logic: `hurtServer` runs on its instance.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: pumpkin_data::damage::DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let world = self.entity.world.load_full();
        let mut request = damage_event("hurt_entity", damage_type, source, cause);
        request["op"] = json!("hurt_entity");
        request["entity"] = json!(self.entity.entity_id);
        request["amount"] = json!(amount);
        with_world(&world, &mut request);
        let result = if bridge::in_lease() {
            bridge::play_unleased(&request)
        } else {
            bridge::play(RustWorld::leased(&world), &request)
        };
        match result {
            Ok(reply) => {
                deliver(
                    &world,
                    reply["packets"].as_array().map_or(&[][..], Vec::as_slice),
                );
                reply["hurt"] == true
            }
            Err(error) => {
                warn!("NeoForge entity damage failed: {error}");
                false
            }
        }
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
        "player_inventory" => player_inventory(world, request),
        "schedule_tick" => schedule_tick(world, request),
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
        java_nbt: Mutex::new(None),
        in_java: AtomicBool::new(true),
    }))))
}

fn position_of(request: &Value) -> Result<Vector3<f64>, String> {
    Ok(Vector3::new(
        f64_of(request, "x")?,
        f64_of(request, "y")?,
        f64_of(request, "z")?,
    ))
}

/// Creates a mod entity through its Java type, as `/summon` would; the entity id when it spawned.
pub fn summon(
    world: &Arc<World>,
    entity_type: &str,
    position: Vector3<f64>,
) -> Result<Option<i32>, String> {
    let request = json!({"op": "summon", "type": entity_type, "x": position.x, "y": position.y, "z": position.z});
    let reply = play_in(world, request).map_err(|error| error.to_string())?;
    deliver(
        world,
        reply["packets"].as_array().map_or(&[][..], Vec::as_slice),
    );
    Ok(reply["id"].as_i64().and_then(|id| i32::try_from(id).ok()))
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
    let mut load = Vec::new();
    for entity in world.entities.load().iter() {
        let Some(modded) = as_mod_entity(entity.as_ref()) else {
            continue;
        };
        let base = &modded.entity;
        let chunk = base.chunk_pos.load();
        if !modded.in_java.load(Ordering::Relaxed) {
            let java = modded
                .java_nbt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map(bridge::encode_nbt);
            let position = base.pos.load();
            load.push(
                json!({"id": base.entity_id, "uuid": base.entity_uuid.to_string(),
                "type": base.entity_type.resource_name, "nbt": java,
                "x": position.x, "y": position.y, "z": position.z}),
            );
            modded.in_java.store(true, Ordering::Relaxed);
        }
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
        "op": "entities",
        "alive": alive, "ticking": ticking, "load": load,
    });
    let reply = play_in(world, request).map_err(|error| error.to_string())?;
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
        if let Some(java) = state["nbt"]
            .as_str()
            .and_then(|nbt| bridge::decode_nbt(nbt).ok())
        {
            *modded
                .java_nbt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(java);
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

/// Rust's canonical abilities, mirrored into the Java player each sync.
fn abilities_json(player: &Player) -> Value {
    let abilities = *player
        .abilities
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    json!({
        "invulnerable": abilities.invulnerable, "flying": abilities.flying, "mayfly": abilities.allow_flying,
        "instabuild": abilities.creative, "mayBuild": abilities.allow_modify_world,
        "flySpeed": abilities.fly_speed, "walkSpeed": abilities.walk_speed,
    })
}

/// Java changed flight (armor, potions); Rust takes mayfly, flying and speed and sends its own packet.
fn apply_java_abilities(world: &Arc<World>, packet: &Value) {
    let Some(player) = packet["uuid"]
        .as_str()
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .and_then(|id| player_by_uuid(world, id))
    else {
        return;
    };
    {
        let mut abilities = player
            .abilities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Creative and spectator always keep mayfly.
        let keeps_mayfly =
            abilities.creative || player.gamemode.load() == pumpkin_util::GameMode::Spectator;
        abilities.allow_flying = keeps_mayfly || packet["mayfly"] == true;
        abilities.flying = abilities.allow_flying && packet["flying"] == true;
        if let Some(speed) = packet["flySpeed"].as_f64() {
            abilities.fly_speed = speed as f32;
        }
    }
    player.send_abilities_update();
}

/// Sends what Java queued: mod payloads by channel, vanilla packets as the 26.2 bytes Java encoded.
fn deliver(world: &Arc<World>, packets: &[Value]) {
    for packet in packets {
        if packet["kind"] == "abilities" {
            apply_java_abilities(world, packet);
            continue;
        }
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
        // Players that have the chunk loaded, as vanilla's ChunkHolder broadcasts block entity updates.
        Some("chunk") => {
            let (Some(x), Some(z)) = (
                packet["cx"].as_i64().and_then(|x| i32::try_from(x).ok()),
                packet["cz"].as_i64().and_then(|z| i32::try_from(z).ok()),
            ) else {
                return Vec::new();
            };
            world
                .players
                .load()
                .iter()
                .filter(|player| player.watched_section.load().is_within_distance(x, z))
                .cloned()
                .collect()
        }
        _ => Vec::new(),
    }
}

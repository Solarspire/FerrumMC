//! Frozen process-lifetime registry additions. Vanilla IDs are never reassigned.
use std::collections::BTreeMap;
use std::sync::OnceLock;

use pumpkin_util::math::{boundingbox::BoundingBox, vector3::Vector3};

use crate::block_properties::COLLISION_SHAPES;
use crate::{Block, BlockId, BlockState, BlockStateId, entity::EntityType, item::Item};

pub struct StateDefinition {
    pub properties: BTreeMap<String, String>,
    pub default: bool,
    pub hardness: f32,
    pub luminance: u8,
    pub solid: bool,
    /// Java's `requiresCorrectToolForDrops`; the cloned stone state would otherwise require a tool for every modded block.
    pub tool_required: bool,
    pub block_entity: Option<String>,
    /// Java's `isRandomlyTicking`.
    pub random_ticks: bool,
    /// `getCollisionShape` boxes as `[min x, y, z, max x, y, z]`; `None` keeps the cloned template's shape.
    pub collision_shapes: Option<Vec<[f64; 6]>>,
    /// `getShape` boxes, same layout.
    pub outline_shapes: Option<Vec<[f64; 6]>>,
}

pub struct BlockDefinition {
    pub name: String,
    pub states: Vec<StateDefinition>,
}

struct Registry {
    blocks: Vec<&'static Block>,
    state_ids: BTreeMap<&'static str, BTreeMap<BTreeMap<String, String>, BlockStateId>>,
    states: Vec<(
        &'static Block,
        &'static BlockState,
        BTreeMap<String, String>,
    )>,
    items: Vec<&'static Item>,
    types: Vec<&'static str>,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();
/// Mod shapes; their indices follow the generated shape table.
static SHAPES: OnceLock<Vec<BoundingBox>> = OnceLock::new();

/// The shape for an index in `BlockState::collision_shapes` or `outline_shapes`, vanilla or mod.
#[must_use]
pub fn shape(id: u16) -> BoundingBox {
    let vanilla = COLLISION_SHAPES.len();
    match usize::from(id).checked_sub(vanilla) {
        None => COLLISION_SHAPES[usize::from(id)],
        Some(index) => SHAPES
            .get()
            .and_then(|shapes| shapes.get(index))
            .copied()
            .unwrap_or(COLLISION_SHAPES[0]),
    }
}

/// `has_random_ticks` that also knows mod block states.
#[must_use]
pub fn has_random_ticks(id: BlockStateId) -> bool {
    crate::block_properties::has_random_ticks(id)
        || state(id).is_some_and(BlockState::has_random_ticks)
}

pub fn installed() -> bool {
    REGISTRY.get().is_some()
}

/// Installs once, before any worlds or clients are created.
pub fn install(
    mut definitions: Vec<BlockDefinition>,
    mut item_names: Vec<String>,
    mut types: Vec<String>,
) -> Result<(), String> {
    if installed() {
        return Err("runtime registries already installed".into());
    }
    definitions.sort_by(|a, b| a.name.cmp(&b.name));
    item_names.sort();
    types.sort();
    let item_start = (0..=u16::MAX)
        .rev()
        .find(|&id| Item::from_id(id).is_some())
        .map_or(0, |id| usize::from(id) + 1);
    let state_count: usize = definitions.iter().map(|b| b.states.len()).sum();
    if usize::from(BlockStateId::COUNT) + state_count > usize::from(u16::MAX) + 1
        || usize::from(BlockId::COUNT) + definitions.len() > usize::from(u16::MAX) + 1
        || item_start + item_names.len() > usize::from(u16::MAX) + 1
    {
        return Err("runtime registries exceed the engine's 16-bit ID capacity".into());
    }
    let types: Vec<&'static str> = types
        .into_iter()
        .map(|s| &*Box::leak(s.into_boxed_str()))
        .collect();
    let items: Vec<&'static Item> = item_names
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let mut item = Item::STONE.clone();
            item.id = (item_start + i) as u16;
            item.registry_key = Box::leak(name.into_boxed_str());
            &*Box::leak(Box::new(item))
        })
        .collect();
    let mut blocks = Vec::new();
    let mut states = Vec::new();
    let mut shapes: Vec<BoundingBox> = Vec::new();
    let mut intern = |boxes: &[[f64; 6]]| -> &'static [u16] {
        let ids: Vec<u16> = boxes
            .iter()
            .map(|b| {
                let shape = BoundingBox::new(
                    Vector3::new(b[0], b[1], b[2]),
                    Vector3::new(b[3], b[4], b[5]),
                );
                shapes.push(shape);
                (COLLISION_SHAPES.len() + shapes.len() - 1) as u16
            })
            .collect();
        &*Box::leak(ids.into_boxed_slice())
    };
    for definition in definitions {
        let mut block = Block::STONE.clone();
        block.id = BlockId::runtime(BlockId::COUNT + blocks.len() as u16);
        block.name = Box::leak(definition.name.into_boxed_str());
        block.item_id = items
            .iter()
            .find(|i| i.registry_key == block.name)
            .map_or(Item::AIR.id, |i| i.id);
        block.loot_table = None;
        let start = states.len();
        let mut default = 0;
        let mut block_states = Vec::new();
        for (i, definition) in definition.states.iter().enumerate() {
            if definition.default {
                default = i;
            }
            let mut state = if definition.solid {
                Block::STONE.default_state.clone()
            } else {
                Block::GLASS.default_state.clone()
            };
            state.id = BlockStateId::runtime((usize::from(BlockStateId::COUNT) + start + i) as u16);
            if definition.tool_required {
                state.state_flags |= crate::block_state::TOOL_REQUIRED;
            } else {
                state.state_flags &= !crate::block_state::TOOL_REQUIRED;
            }
            if definition.random_ticks {
                state.state_flags |= crate::block_state::HAS_RANDOM_TICKS;
            }
            if let Some(boxes) = &definition.collision_shapes {
                state.collision_shapes = intern(boxes);
            }
            if let Some(boxes) = &definition.outline_shapes {
                state.outline_shapes = intern(boxes);
            }
            state.hardness = definition.hardness;
            state.luminance = definition.luminance;
            state.block_entity_type = definition
                .block_entity
                .as_ref()
                .and_then(|name| types.iter().position(|t| *t == name))
                .map_or(u16::MAX, |i| {
                    (crate::block_properties::BLOCK_ENTITY_TYPES.len() + i) as u16
                });
            block_states.push(state);
        }
        let block_states = Box::leak(block_states.into_boxed_slice());
        block.states = block_states;
        block.default_state = &block_states[default];
        block.hardness = block.default_state.hardness;
        let block = &*Box::leak(Box::new(block));
        for (state, definition) in block_states.iter().zip(definition.states) {
            states.push((block, state, definition.properties));
        }
        blocks.push(block);
    }
    let mut state_ids: BTreeMap<_, BTreeMap<_, _>> = BTreeMap::new();
    for (block, state, properties) in &states {
        state_ids
            .entry(block.name)
            .or_default()
            .insert(properties.clone(), state.id);
    }
    drop(intern);
    SHAPES
        .set(shapes)
        .map_err(|_| "runtime shapes already installed".to_owned())?;
    REGISTRY
        .set(Registry {
            blocks,
            state_ids,
            states,
            items,
            types,
        })
        .map_err(|_| "runtime registries already installed".into())
}

pub fn block(id: BlockId) -> Option<&'static Block> {
    REGISTRY
        .get()?
        .blocks
        .get(usize::from(id.as_u16().checked_sub(BlockId::COUNT)?))
        .copied()
}
pub fn state(id: BlockStateId) -> Option<&'static BlockState> {
    Some(
        REGISTRY
            .get()?
            .states
            .get(usize::from(id.as_u16().checked_sub(BlockStateId::COUNT)?))?
            .1,
    )
}
pub fn state_block(id: BlockStateId) -> Option<&'static Block> {
    Some(
        REGISTRY
            .get()?
            .states
            .get(usize::from(id.as_u16().checked_sub(BlockStateId::COUNT)?))?
            .0,
    )
}
pub fn block_by_name(name: &str) -> Option<&'static Block> {
    REGISTRY
        .get()?
        .blocks
        .iter()
        .copied()
        .find(|b| b.name == name)
}
pub fn block_by_item(id: u16) -> Option<&'static Block> {
    REGISTRY
        .get()?
        .blocks
        .iter()
        .copied()
        .find(|b| b.item_id == id)
}
pub fn item(id: u16) -> Option<&'static Item> {
    let registry = REGISTRY.get()?;
    registry
        .items
        .get(usize::from(id.checked_sub(registry.items.first()?.id)?))
        .copied()
}
pub fn item_by_name(name: &str) -> Option<&'static Item> {
    REGISTRY
        .get()?
        .items
        .iter()
        .copied()
        .find(|i| i.registry_key == name)
}
pub fn block_entity_name(id: u16) -> Option<&'static str> {
    REGISTRY
        .get()?
        .types
        .get(usize::from(id.checked_sub(
            crate::block_properties::BLOCK_ENTITY_TYPES.len() as u16,
        )?))
        .copied()
}
pub fn properties(id: BlockStateId) -> Option<&'static BTreeMap<String, String>> {
    Some(
        &REGISTRY
            .get()?
            .states
            .get(usize::from(id.as_u16().checked_sub(BlockStateId::COUNT)?))?
            .2,
    )
}
pub fn resolve(name: &str, properties: &BTreeMap<String, String>) -> Option<BlockStateId> {
    REGISTRY
        .get()?
        .state_ids
        .get(name)?
        .get(properties)
        .copied()
}

pub fn block_entity_by_name(name: &str) -> Option<&'static str> {
    REGISTRY.get()?.types.iter().copied().find(|t| *t == name)
}
pub fn block_entity_type_id(name: &str) -> Option<u16> {
    let index = REGISTRY.get()?.types.iter().position(|t| *t == name)?;
    u16::try_from(crate::block_properties::BLOCK_ENTITY_TYPES.len() + index).ok()
}

/// A `NeoForge` entity type the bridge mirrors: Java owns its simulation, Rust its identity and tracking.
pub struct EntityTypeDefinition {
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub tracking_range: u32,
    pub update_interval: u32,
    pub track_deltas: bool,
}

static ENTITY_TYPES: OnceLock<Vec<&'static EntityType>> = OnceLock::new();

/// Installs once, after [`install`]. Runtime entity types are never saved or summoned by the engine.
pub fn install_entity_types(mut definitions: Vec<EntityTypeDefinition>) -> Result<(), String> {
    definitions.sort_by(|a, b| a.name.cmp(&b.name));
    let start = (0..=u16::MAX)
        .rev()
        .find(|&id| EntityType::from_raw(id).is_some())
        .map_or(0, |id| usize::from(id) + 1);
    if start + definitions.len() > usize::from(u16::MAX) + 1 {
        return Err("runtime entity types exceed the engine's 16-bit ID capacity".into());
    }
    let types = definitions
        .into_iter()
        .enumerate()
        .map(|(index, definition)| {
            let mut entity_type = EntityType::ARROW.clone();
            entity_type.id = (start + index) as u16;
            entity_type.resource_name = Box::leak(definition.name.into_boxed_str());
            entity_type.attackable = Some(true);
            entity_type.saveable = false;
            entity_type.summonable = false;
            entity_type.client_tracking_range = definition.tracking_range;
            entity_type.update_interval = definition.update_interval;
            entity_type.track_deltas = definition.track_deltas;
            entity_type.dimension = [definition.width, definition.height];
            entity_type.eye_height = definition.eye_height;
            &*Box::leak(Box::new(entity_type))
        })
        .collect();
    ENTITY_TYPES
        .set(types)
        .map_err(|_| "runtime entity types already installed".into())
}

pub fn entity_type_by_name(name: &str) -> Option<&'static EntityType> {
    ENTITY_TYPES
        .get()?
        .iter()
        .copied()
        .find(|t| t.resource_name == name)
}

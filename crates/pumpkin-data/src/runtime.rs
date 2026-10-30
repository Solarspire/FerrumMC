//! Frozen process-lifetime registry additions. Vanilla IDs are never reassigned.
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::{Block, BlockId, BlockState, BlockStateId, entity::EntityType, item::Item};

pub struct StateDefinition {
    pub properties: BTreeMap<String, String>,
    pub default: bool,
    pub hardness: f32,
    pub luminance: u8,
    pub solid: bool,
    pub block_entity: Option<String>,
}

pub struct BlockDefinition {
    pub name: String,
    pub states: Vec<StateDefinition>,
}

struct Registry {
    blocks: Vec<&'static Block>,
    states: Vec<(
        &'static Block,
        &'static BlockState,
        BTreeMap<String, String>,
    )>,
    items: Vec<&'static Item>,
    types: Vec<&'static str>,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

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
    REGISTRY
        .set(Registry {
            blocks,
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
        .states
        .iter()
        .find(|(block, _, props)| block.name == name && props == properties)
        .map(|(_, state, _)| state.id)
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

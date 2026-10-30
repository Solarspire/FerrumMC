use pumpkin_data::{
    Block, BlockStateId,
    item::Item,
    runtime::{self, BlockDefinition, StateDefinition},
};
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::chunk::{ChunkData, format::anvil::SingleChunkDataSerializer};
use std::collections::BTreeMap;

#[test]
fn runtime_palette_persists_names_and_custom_properties() -> Result<(), Box<dyn std::error::Error>>
{
    let properties = BTreeMap::from([("custom_phase".to_owned(), "running".to_owned())]);
    runtime::install(
        vec![BlockDefinition {
            name: "probe:machine".into(),
            states: vec![StateDefinition {
                properties: properties.clone(),
                default: true,
                hardness: 2.0,
                luminance: 0,
                solid: true,
                block_entity: Some("probe:machine".into()),
            }],
        }],
        vec!["probe:machine".into()],
        vec!["probe:machine".into()],
    )?;
    let state = runtime::resolve("probe:machine", &properties).ok_or("state missing")?;
    assert!(state.as_u16() >= BlockStateId::COUNT);
    assert_eq!(
        Block::from_name("probe:machine")
            .ok_or("block missing")?
            .default_state
            .id,
        state
    );
    let item = Item::from_registry_key("probe:machine").ok_or("item missing")?;
    assert_eq!(Item::from_id(item.id), Some(item));
    let stack = pumpkin_data::item_stack::ItemStack::new(3, item);
    let mut nbt = pumpkin_nbt::compound::NbtCompound::new();
    stack.write_item_stack(&mut nbt);
    assert_eq!(nbt.get_string("id"), Some("probe:machine"));
    assert_eq!(
        pumpkin_data::item_stack::ItemStack::read_item_stack(&nbt)
            .ok_or("item stack missing")?
            .item,
        item
    );
    let chunk = ChunkData::empty(0, 0);
    chunk.set_block_absolute_y(0, 100, 0, state);
    let bytes = chunk.to_pump_bytes()?;
    assert!(!bytes.starts_with(b"PCHK"));
    let mut cursor = std::io::Cursor::new(bytes.as_ref());
    let nbt = pumpkin_nbt::Nbt::read(&mut pumpkin_nbt::deserializer::NbtReadHelperJava::new(
        &mut cursor,
    ))?;
    let text = nbt.root_tag.to_string();
    assert!(text.contains("probe:machine"));
    assert!(text.contains("custom_phase"));
    let loaded = ChunkData::internal_from_bytes(&bytes, Vector2::new(0, 0))?;
    assert_eq!(loaded.section.get_block_absolute_y(0, 100, 0), Some(state));
    let unknown = bytes
        .windows(b"probe:machine".len())
        .position(|s| s == b"probe:machine")
        .ok_or("palette missing")?;
    let mut bytes = bytes.to_vec();
    bytes[unknown..unknown + 13].copy_from_slice(b"probe:missing");
    assert!(ChunkData::internal_from_bytes(&bytes, Vector2::new(0, 0)).is_err());
    Ok(())
}

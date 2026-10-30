use proc_macro2::TokenStream;
use quote::quote;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, fs};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Deserialize)]
struct Id {
    id: usize,
}
#[derive(Deserialize)]
struct PacketId {
    protocol_id: i32,
}
#[derive(Deserialize)]
struct Registry {
    entries: BTreeMap<String, PacketId>,
}
#[derive(Deserialize)]
struct Block {
    name: String,
    id: usize,
    properties: Vec<i64>,
    states: Vec<Id>,
}
#[derive(Deserialize)]
struct Blocks {
    blocks: Vec<Block>,
    block_entity_types: Vec<String>,
}
#[derive(Deserialize)]
struct OldState {
    id: i32,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct OldBlock {
    states: Vec<OldState>,
}
#[derive(Deserialize)]
struct Property {
    hash_key: i64,
    serialized_name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    min: i64,
    #[serde(default)]
    max: i64,
    #[serde(default)]
    values: Vec<String>,
}
fn read<T: DeserializeOwned>(path: &str) -> Result<T> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}
fn names(base: &str, registry: &str) -> Result<Vec<String>> {
    let path = std::path::Path::new(base).join(registry);
    if !path.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            entries.push(
                path.file_stem()
                    .ok_or("Missing registry entry name")?
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    entries.sort();
    if registry == "chat_type" {
        entries.push("raw".into());
    }
    Ok(entries)
}

pub(crate) fn build() -> TokenStream {
    match generate() {
        Ok(output) => output,
        Err(error) => {
            let message = error.to_string();
            quote! {compile_error!(#message);}
        }
    }
}

fn generate() -> Result<TokenStream> {
    type OldPackets = BTreeMap<String, BTreeMap<String, BTreeMap<String, PacketId>>>;
    #[derive(Deserialize)]
    struct Packets {
        clientbound: BTreeMap<String, BTreeMap<String, i32>>,
        serverbound: BTreeMap<String, BTreeMap<String, i32>>,
    }
    let old: OldPackets = read("../../assets/compat/26_2/packets.json")?;
    let current: Packets = read("../../assets/packets.json")?;
    let mut packets = Vec::new();
    for direction in ["clientbound", "serverbound"] {
        for phase in ["configuration", "play"] {
            let new_phase = if phase == "configuration" {
                "config"
            } else {
                phase
            };
            let phases = if direction == "clientbound" {
                &current.clientbound
            } else {
                &current.serverbound
            };
            for (name, &id) in &phases[new_phase] {
                let old_name = if name == "punch" { "swing" } else { name };
                let old_id = old[phase][direction]
                    .get(&format!("minecraft:{old_name}"))
                    .map_or(-1, |id| id.protocol_id);
                packets.push(quote! {(#direction,#new_phase,#id,#old_id)});
            }
        }
    }
    let registries: BTreeMap<String, Registry> = read("../../assets/compat/26_2/registries.json")?;
    let blocks: Blocks = read("../../assets/blocks.json")?;
    let items: BTreeMap<String, Id> = read("../../assets/items.json")?;
    let entities: BTreeMap<String, Id> = read("../../assets/entities.json")?;
    let lookup = |registry: &str, name: &str| {
        registries
            .get(&format!("minecraft:{registry}"))
            .and_then(|r| {
                r.entries.get(&format!(
                    "minecraft:{}",
                    name.trim_start_matches("minecraft:")
                ))
            })
            .map_or(-1, |id| id.protocol_id)
    };
    let mut maps = BTreeMap::<String, Vec<i32>>::new();
    for (registry, entries) in [
        (
            "block",
            blocks
                .blocks
                .iter()
                .map(|b| (b.name.clone(), b.id))
                .collect::<Vec<_>>(),
        ),
        (
            "item",
            items
                .iter()
                .map(|(name, id)| (name.clone(), id.id))
                .collect(),
        ),
        (
            "entity_type",
            entities
                .iter()
                .map(|(name, id)| (name.clone(), id.id))
                .collect(),
        ),
        (
            "block_entity_type",
            blocks
                .block_entity_types
                .iter()
                .enumerate()
                .map(|(id, name)| (name.clone(), id))
                .collect(),
        ),
    ] {
        let mut ids = vec![-1; entries.len()];
        for (name, id) in entries {
            ids[id] = lookup(registry, &name);
        }
        maps.insert(registry.into(), ids);
    }
    for (registry, file) in [
        ("attribute", "attributes"),
        ("sound_event", "sounds"),
        ("particle_type", "particles"),
        ("fluid", "fluids"),
        ("game_event", "game_event"),
        ("data_component_type", "data_component"),
        ("potion", "potion"),
        ("mob_effect", "effect"),
    ] {
        let entries: Value = read(&format!("../../assets/{file}.json"))?;
        let mut pairs = Vec::new();
        match entries {
            Value::Array(entries) => {
                for (id, entry) in entries.iter().enumerate() {
                    let name = entry
                        .as_str()
                        .or_else(|| entry["name"].as_str())
                        .ok_or("Missing registry entry name")?;
                    pairs.push((id, name.to_owned()));
                }
            }
            Value::Object(entries) => {
                for (name, entry) in entries {
                    pairs.push((
                        usize::try_from(entry["id"].as_u64().ok_or("Missing registry entry ID")?)?,
                        name,
                    ));
                }
            }
            _ => return Err("Invalid registry data shape".into()),
        }
        let mut ids = vec![-1; pairs.len()];
        for (id, name) in pairs {
            ids[id] = lookup(registry, &name);
        }
        maps.insert(registry.into(), ids);
    }
    let old_blocks: BTreeMap<String, OldBlock> = read("../../assets/compat/26_2/blocks.json")?;
    let properties: Vec<Property> = read("../../assets/properties.json")?;
    let properties: BTreeMap<_, _> = properties.iter().map(|p| (p.hash_key, p)).collect();
    let mut states = vec![-1; blocks.blocks.iter().map(|b| b.states.len()).sum()];
    for block in &blocks.blocks {
        let Some(old) = old_blocks.get(&format!("minecraft:{}", block.name)) else {
            continue;
        };
        for (index, state) in block.states.iter().enumerate() {
            let mut index = index;
            let mut values = BTreeMap::new();
            for key in block.properties.iter().rev() {
                let property = properties.get(key).ok_or("Missing block property")?;
                let variants: Vec<_> = match property.kind.as_str() {
                    "boolean" => vec!["true".into(), "false".into()],
                    "int" => (property.min..=property.max)
                        .map(|n| n.to_string())
                        .collect(),
                    _ => property.values.clone(),
                };
                if variants.is_empty() {
                    return Err("Empty property value list".into());
                }
                values.insert(
                    property.serialized_name.clone(),
                    variants[index % variants.len()].clone(),
                );
                index /= variants.len();
            }
            if let Some(old) = old.states.iter().find(|s| s.properties == values) {
                states[state.id] = old.id;
            }
        }
    }
    for &name in crate::registry::SYNCED_REGISTRIES {
        let old = names("../../assets/compat/26_2/data", name)?;
        let new = names("../../assets/datapack/data/minecraft", name)?;
        maps.insert(
            name.into(),
            new.iter()
                .map(|n| old.iter().position(|o| o == n).map_or(-1, |i| i as i32))
                .collect(),
        );
    }
    let old_state_count = old_blocks
        .values()
        .flat_map(|block| block.states.iter())
        .map(|state| state.id)
        .max()
        .ok_or("Empty block report")?
        + 1;
    let registry_maps = maps.iter().map(|(name, ids)| quote! {(#name,&[#(#ids),*])});
    Ok(quote! {
        pub const PACKETS_26_2:&[(&str,&str,i32,i32)]=&[#(#packets),*];
        pub const REGISTRIES_26_2:&[(&str,&[i32])]=&[#(#registry_maps),*];
        pub const BLOCK_STATE_COUNT_26_2: i32 = #old_state_count;
        pub const BLOCK_STATES_26_2:&[i32]=&[#(#states),*];
    })
}

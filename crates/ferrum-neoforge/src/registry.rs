use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::Error;

/// A disk identity remains valid when `NeoForge` registration order changes.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct StateKey {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(flatten)]
    pub key: StateKey,
    pub java_id: u32,
    #[serde(rename = "default")]
    pub is_default: bool,
    #[serde(deserialize_with = "hardness")]
    pub hardness: f32,
    pub luminance: u8,
    pub solid: bool,
    pub block_entity: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub name: String,
    pub java_id: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityTypeInfo {
    pub java_id: u32,
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub tracking_range: u32,
    pub update_interval: u32,
    pub track_deltas: bool,
}

/// The frozen 26.2 registry manifest; Java IDs are a separate namespace from Pumpkin IDs.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub states: Vec<State>,
    pub items: Vec<Item>,
    pub block_entity_types: Vec<String>,
    #[serde(default)]
    pub block_entity_type_ids: BTreeMap<String, u32>,
    #[serde(default)]
    pub block_ids: BTreeMap<String, u32>,
    /// Mod entity types by name; Java owns their simulation.
    #[serde(default)]
    pub entity_types: BTreeMap<String, EntityTypeInfo>,
    /// Entry names of each synchronized registry in the order the client receives them.
    #[serde(default)]
    pub synced_registries: BTreeMap<String, Vec<String>>,
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Self, Error> {
        let manifest: Self = serde_json::from_str(json)?;
        let mut keys = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut blocks = BTreeSet::new();
        let mut defaults = BTreeSet::new();
        for state in &manifest.states {
            validate_name(&state.key.name)?;
            if !keys.insert(&state.key) || !ids.insert(state.java_id) {
                return Err(Error::Runtime(format!(
                    "duplicate block state: {:?}",
                    state.key
                )));
            }
            blocks.insert(&state.key.name);
            if state.is_default && !defaults.insert(&state.key.name) {
                return Err(Error::Runtime(format!(
                    "multiple default states: {}",
                    state.key.name
                )));
            }
        }
        if blocks.is_empty() || blocks != defaults {
            return Err(Error::Runtime(
                "each registered block must have one default state".into(),
            ));
        }
        let mut names = BTreeSet::new();
        ids.clear();
        for item in &manifest.items {
            validate_name(&item.name)?;
            if !names.insert(&item.name) || !ids.insert(item.java_id) {
                return Err(Error::Runtime(format!("duplicate item: {}", item.name)));
            }
        }
        names.clear();
        for name in &manifest.block_entity_types {
            validate_name(name)?;
            if !names.insert(name) {
                return Err(Error::Runtime(format!(
                    "duplicate block entity type: {name}"
                )));
            }
        }
        Ok(manifest)
    }
}

fn validate_name(name: &str) -> Result<(), Error> {
    if let Some((namespace, path)) = name.split_once(':')
        && !namespace.is_empty()
        && !path.is_empty()
        && namespace
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c))
        && path
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"/_.-".contains(&c))
    {
        return Ok(());
    }
    Err(Error::Runtime(format!("invalid registry name: {name}")))
}

impl Manifest {
    pub fn install(&self) -> Result<(), Error> {
        let mut blocks: BTreeMap<String, Vec<pumpkin_data::runtime::StateDefinition>> =
            BTreeMap::new();
        for state in &self.states {
            if state.key.name.starts_with("minecraft:") {
                continue;
            }
            blocks.entry(state.key.name.clone()).or_default().push(
                pumpkin_data::runtime::StateDefinition {
                    properties: state.key.properties.clone(),
                    default: state.is_default,
                    hardness: state.hardness,
                    luminance: state.luminance,
                    solid: state.solid,
                    block_entity: state.block_entity.clone(),
                },
            );
        }
        pumpkin_data::runtime::install_entity_types(
            self.entity_types
                .iter()
                .map(|(name, info)| pumpkin_data::runtime::EntityTypeDefinition {
                    name: name.clone(),
                    width: info.width,
                    height: info.height,
                    eye_height: info.eye_height,
                    tracking_range: info.tracking_range,
                    update_interval: info.update_interval,
                    track_deltas: info.track_deltas,
                })
                .collect(),
        )
        .map_err(Error::Runtime)?;
        pumpkin_data::runtime::install(
            blocks
                .into_iter()
                .map(|(name, states)| pumpkin_data::runtime::BlockDefinition { name, states })
                .collect(),
            self.items
                .iter()
                .filter(|i| !i.name.starts_with("minecraft:"))
                .map(|i| i.name.clone())
                .collect(),
            self.block_entity_types
                .iter()
                .filter(|i| !i.starts_with("minecraft:"))
                .cloned()
                .collect(),
        )
        .map_err(Error::Runtime)
    }
}

fn hardness<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Hardness {
        Number(f32),
        Special(String),
    }
    match Hardness::deserialize(deserializer)? {
        Hardness::Number(value) => Ok(value),
        Hardness::Special(value) => match value.as_str() {
            "Infinity" => Ok(f32::INFINITY),
            "-Infinity" => Ok(f32::NEG_INFINITY),
            "NaN" => Ok(f32::NAN),
            _ => Err(serde::de::Error::custom("invalid hardness")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::Manifest;

    #[test]
    fn rejects_ambiguous_disk_identities_and_wire_ids() {
        let state = r#"{"name":"probe:counter","properties":{},"javaId":1,"default":true,"hardness":1.0,"luminance":0,"solid":true}"#;
        let duplicate =
            format!(r#"{{"states":[{state},{state}],"items":[],"blockEntityTypes":[]}}"#);
        assert!(Manifest::parse(&duplicate).is_err());
        let other = state.replace("probe:counter", "probe:other");
        let duplicate_id =
            format!(r#"{{"states":[{state},{other}],"items":[],"blockEntityTypes":[]}}"#);
        assert!(Manifest::parse(&duplicate_id).is_err());
        let valid = format!(r#"{{"states":[{state}],"items":[],"blockEntityTypes":[]}}"#);
        assert!(Manifest::parse(&valid).is_ok());
    }
}

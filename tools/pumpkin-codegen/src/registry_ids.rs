use proc_macro2::TokenStream;
use quote::quote;
use serde_json::Value;
use std::{collections::BTreeMap, fs};

/// Native registry names in engine ID order, used by the NeoForge adapter.
pub(crate) fn build() -> TokenStream {
    let read = |file: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(format!("../../assets/{file}.json")).unwrap())
            .unwrap()
    };
    let mut registries = BTreeMap::<String, Vec<String>>::new();
    let blocks = read("blocks");
    for (registry, entries) in [
        (
            "block",
            blocks["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| {
                    (
                        b["id"].as_u64().unwrap() as usize,
                        b["name"].as_str().unwrap().to_owned(),
                    )
                })
                .collect::<Vec<_>>(),
        ),
        (
            "block_entity_type",
            blocks["block_entity_types"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(id, n)| (id, n.as_str().unwrap().to_owned()))
                .collect(),
        ),
    ] {
        let mut names = vec![String::new(); entries.len()];
        for (id, name) in entries {
            names[id] = name;
        }
        registries.insert(registry.into(), names);
    }
    for (registry, file) in [
        ("item", "items"),
        ("entity_type", "entities"),
        ("attribute", "attributes"),
        ("sound_event", "sounds"),
        ("particle_type", "particles"),
        ("fluid", "fluids"),
        ("game_event", "game_event"),
        ("data_component_type", "data_component"),
        ("potion", "potion"),
        ("mob_effect", "effect"),
    ] {
        let data = read(file);
        let pairs: Vec<(usize, String)> = match data {
            Value::Array(entries) => entries
                .iter()
                .enumerate()
                .map(|(id, e)| {
                    (
                        id,
                        e.as_str()
                            .or_else(|| e["name"].as_str())
                            .unwrap()
                            .to_owned(),
                    )
                })
                .collect(),
            Value::Object(entries) => entries
                .into_iter()
                .map(|(name, e)| {
                    (
                        e.as_u64().or_else(|| e["id"].as_u64()).unwrap() as usize,
                        name,
                    )
                })
                .collect(),
            _ => panic!("Invalid registry {registry}"),
        };
        let mut names = vec![String::new(); pairs.len()];
        for (id, name) in pairs {
            names[id] = name.trim_start_matches("minecraft:").to_owned();
        }
        registries.insert(registry.into(), names);
    }
    for &registry in crate::registry::SYNCED_REGISTRIES {
        let path = std::path::Path::new("../../assets/compat/26_2/data").join(registry);
        if !path.is_dir() {
            continue;
        }
        let mut names: Vec<_> = fs::read_dir(path)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        if registry == "chat_type" {
            names.push("raw".into());
        }
        registries.insert(registry.into(), names);
    }
    let registries = registries
        .iter()
        .map(|(registry, names)| quote! {(#registry,&[#(#names),*])});
    quote! {pub const REGISTRY_ENTRIES: &[(&str,&[&str])]=&[#(#registries),*];}
}

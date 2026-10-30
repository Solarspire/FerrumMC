use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use base64::{Engine, prelude::BASE64_STANDARD};
use jni::{
    EnvUnowned, JavaVM, NativeMethod,
    objects::{JClass, JString},
};
use pumpkin_data::BlockStateId;
use pumpkin_nbt::{Nbt, compound::NbtCompound};
use serde_json::Value;

use crate::{
    Error,
    registry::{Manifest, StateKey},
};

pub trait WorldAccess: Send + Sync {
    fn call(&self, request: Value) -> Result<Value, String>;
}

struct Bridge {
    vm: JavaVM,
    to_java: BTreeMap<BlockStateId, u32>,
    to_rust: BTreeMap<u32, BlockStateId>,
    /// Wire IDs of runtime (modded) content; vanilla IDs use the generated 26.2 tables.
    items_to_java: BTreeMap<u16, u32>,
    items_to_rust: BTreeMap<u32, u16>,
    blocks_to_java: BTreeMap<u16, u32>,
    block_entity_types_to_java: BTreeMap<u16, u32>,
    entity_types_to_java: BTreeMap<u16, u32>,
    state_count: usize,
    /// Client ID of each synchronized registry entry by registry and name, mod entries included.
    synced_registries: BTreeMap<String, BTreeMap<String, i32>>,
    /// Java 26.2 state ID indexed by raw Rust state ID, -1 when unmapped.
    wire_states: Vec<i32>,
}
static BRIDGE: OnceLock<Bridge> = OnceLock::new();
thread_local! { static WORLD: RefCell<Option<Arc<dyn WorldAccess>>> = const { RefCell::new(None) }; }

pub(crate) fn install(vm: &JavaVM, manifest: &Manifest) -> Result<(), Error> {
    manifest.install()?;
    let mut keys = BTreeMap::new();
    for raw in 0..BlockStateId::COUNT {
        let Some(id) = BlockStateId::new(raw) else {
            continue;
        };
        let block = id.to_block();
        let properties = block
            .properties(id)
            .map(|p| {
                p.to_props()
                    .iter()
                    .map(|p| (p.key_str().to_owned(), p.value_str().to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        keys.insert(
            StateKey {
                name: format!("minecraft:{}", block.name),
                properties,
            },
            id,
        );
    }
    let mut to_java = BTreeMap::new();
    let mut to_rust = BTreeMap::new();
    for state in &manifest.states {
        let id = keys
            .get(&state.key)
            .copied()
            .or_else(|| pumpkin_data::runtime::resolve(&state.key.name, &state.key.properties));
        if let Some(id) = id {
            to_java.insert(id, state.java_id);
            to_rust.insert(state.java_id, id);
        }
    }
    let mut wire_states = Vec::new();
    for (id, java) in &to_java {
        let index = usize::from(id.as_u16());
        if wire_states.len() <= index {
            wire_states.resize(index + 1, -1);
        }
        wire_states[index] = i32::try_from(*java).unwrap_or(-1);
    }
    let mut items_to_java = BTreeMap::new();
    let mut items_to_rust = BTreeMap::new();
    for item in &manifest.items {
        if let Some(rust) = pumpkin_data::runtime::item_by_name(&item.name) {
            items_to_java.insert(rust.id, item.java_id);
            items_to_rust.insert(item.java_id, rust.id);
        }
    }
    let mut blocks_to_java = BTreeMap::new();
    for (name, java) in &manifest.block_ids {
        if let Some(block) = pumpkin_data::runtime::block_by_name(name) {
            blocks_to_java.insert(block.id.as_u16(), *java);
        }
    }
    let mut block_entity_types_to_java = BTreeMap::new();
    for (name, java) in &manifest.block_entity_type_ids {
        if let Some(rust) = pumpkin_data::runtime::block_entity_type_id(name) {
            block_entity_types_to_java.insert(rust, *java);
        }
    }
    vm.attach_current_thread(|env| -> Result<(), Error> {
        // SAFETY: world_callback has the exact static (String)->String JNI signature.
        let method = unsafe {
            NativeMethod::from_raw_parts(
                jni::jni_str!("world"),
                jni::jni_str!("(Ljava/lang/String;)Ljava/lang/String;"),
                world_callback as *mut _,
            )
        };
        // SAFETY: the registered function and descriptor remain valid for the process lifetime.
        unsafe {
            env.register_native_methods(
                jni::jni_str!("net/solarspire/ferrum/runtime/BridgeRuntime"),
                &[method],
            )
        }?;
        Ok(())
    })?;
    BRIDGE
        .set(Bridge {
            vm: vm.clone(),
            to_java,
            to_rust,
            items_to_java,
            items_to_rust,
            blocks_to_java,
            block_entity_types_to_java,
            entity_types_to_java: entity_types_to_java(manifest),
            state_count: manifest.states.len(),
            synced_registries: synced_ids(manifest),
            wire_states,
        })
        .map_err(|_| Error::Runtime("world bridge already installed".into()))
}

fn entity_types_to_java(manifest: &Manifest) -> BTreeMap<u16, u32> {
    manifest
        .entity_types
        .iter()
        .filter_map(|(name, info)| {
            let rust = pumpkin_data::runtime::entity_type_by_name(name)?;
            Some((rust.id, info.java_id))
        })
        .collect()
}

fn synced_ids(manifest: &Manifest) -> BTreeMap<String, BTreeMap<String, i32>> {
    manifest
        .synced_registries
        .iter()
        .map(|(registry, names)| {
            let ids = names.iter().cloned().zip(0..).collect();
            (
                registry
                    .strip_prefix("minecraft:")
                    .unwrap_or(registry)
                    .to_owned(),
                ids,
            )
        })
        .collect()
}

pub fn enabled() -> bool {
    BRIDGE.get().is_some()
}
pub fn java_state(id: BlockStateId) -> Result<u32, String> {
    BRIDGE
        .get()
        .and_then(|b| b.to_java.get(&id).copied())
        .ok_or_else(|| format!("state has no 26.2 mapping: {id}"))
}
pub fn rust_state(id: u32) -> Option<BlockStateId> {
    BRIDGE.get()?.to_rust.get(&id).copied()
}
pub fn java_state_raw(id: u16) -> Option<i32> {
    BRIDGE
        .get()?
        .wire_states
        .get(usize::from(id))
        .copied()
        .filter(|id| *id >= 0)
}
/// Number of 26.2 block states the synced client holds, which sets the direct palette width.
pub fn java_state_count() -> Option<usize> {
    Some(BRIDGE.get()?.state_count)
}
/// Client ID of a synchronized registry entry; `None` for registries or entries the bridge does not sync.
pub fn synced_registry_id(registry: &str, name: &str) -> Option<i32> {
    BRIDGE
        .get()?
        .synced_registries
        .get(registry)?
        .get(name)
        .copied()
}
pub fn java_item(id: u16) -> Option<u32> {
    BRIDGE.get()?.items_to_java.get(&id).copied()
}
pub fn rust_item(id: u32) -> Option<u16> {
    BRIDGE.get()?.items_to_rust.get(&id).copied()
}
pub fn java_block(id: u16) -> Option<u32> {
    BRIDGE.get()?.blocks_to_java.get(&id).copied()
}
pub fn java_block_entity_type(id: u16) -> Option<u32> {
    BRIDGE.get()?.block_entity_types_to_java.get(&id).copied()
}
pub fn java_entity_type(id: u16) -> Option<u32> {
    BRIDGE.get()?.entity_types_to_java.get(&id).copied()
}

/// One step of `NeoForge`'s configuration phase for a connection, see `ConfigBridge.java`.
pub fn configuration(request: &Value) -> Result<Value, Error> {
    let bridge = BRIDGE
        .get()
        .ok_or_else(|| Error::Runtime("NeoForge bridge is not installed".into()))?;
    bridge.vm.attach_current_thread(|env| {
        let input = env.new_string(request.to_string())?;
        let result = env.call_static_method(
            jni::jni_str!("net/solarspire/ferrum/runtime/BridgeRuntime"),
            jni::jni_str!("configuration"),
            jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
            &[(&input).into()],
        );
        let object = crate::checked(env, result)?.l()?;
        let output = env.cast_local::<JString>(object)?.try_to_string(env)?;
        Ok(serde_json::from_str(&output)?)
    })
}

#[must_use]
pub fn encode_nbt(nbt: &NbtCompound) -> String {
    BASE64_STANDARD.encode(Nbt::from(nbt.clone()).write())
}
pub fn decode_nbt(value: &str) -> Result<NbtCompound, Error> {
    if value.len() > 16 * 1024 * 1024 {
        return Err(Error::Runtime("block entity NBT exceeds 12 MiB".into()));
    }
    let bytes = BASE64_STANDARD
        .decode(value)
        .map_err(|e| Error::Runtime(e.to_string()))?;
    Nbt::read(&mut pumpkin_nbt::deserializer::NbtReadHelperJava::new(
        std::io::Cursor::new(bytes),
    ))
    .map(|n| n.root_tag)
    .map_err(|e| Error::Runtime(e.to_string()))
}

/// Executes a batch synchronously on the calling Rust simulation thread.
pub fn tick(world: Arc<dyn WorldAccess>, batch: &Value) -> Result<Value, Error> {
    let Some(bridge) = BRIDGE.get() else {
        return Ok(Value::Array(Vec::new()));
    };
    leased(&bridge.vm, world, jni::jni_str!("tick"), batch)
}

/// Runs mod player events for one player on the calling tick thread, see `PlayerBridge.java`.
pub fn play(world: Arc<dyn WorldAccess>, request: &Value) -> Result<Value, Error> {
    let bridge = BRIDGE
        .get()
        .ok_or_else(|| Error::Runtime("NeoForge bridge is not installed".into()))?;
    leased(&bridge.vm, world, jni::jni_str!("play"), request)
}

/// A bridge call that needs no world access, such as item component conversion.
pub fn play_unleased(request: &Value) -> Result<Value, Error> {
    let bridge = BRIDGE
        .get()
        .ok_or_else(|| Error::Runtime("NeoForge bridge is not installed".into()))?;
    bridge
        .vm
        .attach_current_thread(|env| call_static(env, jni::jni_str!("play"), request))
}

/// Drops a player's Java view; needs no world access.
pub fn play_leave(request: &Value) -> Result<(), Error> {
    let Some(bridge) = BRIDGE.get() else {
        return Ok(());
    };
    bridge
        .vm
        .attach_current_thread(|env| call_static(env, jni::jni_str!("play"), request).map(|_| ()))
}

fn leased(
    vm: &JavaVM,
    world: Arc<dyn WorldAccess>,
    method: &jni::strings::JNIStr,
    request: &Value,
) -> Result<Value, Error> {
    struct Lease;
    impl Drop for Lease {
        fn drop(&mut self) {
            WORLD.with(|w| *w.borrow_mut() = None);
        }
    }
    WORLD.with(|w| {
        if w.borrow().is_some() {
            return Err(Error::Runtime("reentrant NeoForge batch".into()));
        }
        *w.borrow_mut() = Some(world);
        Ok(())
    })?;
    let _lease = Lease;
    vm.attach_current_thread(|env| call_static(env, method, request))
}

fn call_static(
    env: &mut jni::Env<'_>,
    method: &jni::strings::JNIStr,
    request: &Value,
) -> Result<Value, Error> {
    let input = env.new_string(request.to_string())?;
    let result = env.call_static_method(
        jni::jni_str!("net/solarspire/ferrum/runtime/BridgeRuntime"),
        method,
        jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
        &[(&input).into()],
    );
    let object = crate::checked(env, result)?.l()?;
    let output = env.cast_local::<JString>(object)?.try_to_string(env)?;
    Ok(serde_json::from_str(&output)?)
}

extern "system" fn world_callback<'local>(
    mut unowned: EnvUnowned<'local>,
    _class: JClass<'local>,
    input: JString<'local>,
) -> JString<'local> {
    unowned
        .with_env(|env| -> Result<_, Error> {
            let input = input.try_to_string(env)?;
            if input.len() > 1024 * 1024 {
                return Err(Error::Runtime("world callback exceeds 1 MiB".into()));
            }
            let request = serde_json::from_str(&input)?;
            let world = WORLD
                .with(|w| w.borrow().clone())
                .ok_or_else(|| Error::Runtime("world access outside a Rust tick lease".into()))?;
            let value = world.call(request).map_err(Error::Runtime)?;
            Ok(env.new_string(value.to_string())?)
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// Custom data key under which a stack carries the components only mods define, as `NeoForge` stores them.
pub const MOD_COMPONENTS: &str = "ferrum_mod_components";
/// Bound on distinct mod component sets whose client encoding is kept.
const MAX_CACHED_COMPONENT_SETS: usize = 1024;

/// A mod component as the client holds it: its id in the synced component registry and its encoded value.
pub type WireComponent = (i32, Vec<u8>);

type ComponentCache = BTreeMap<String, Arc<[WireComponent]>>;
static COMPONENT_CACHE: std::sync::Mutex<ComponentCache> = std::sync::Mutex::new(BTreeMap::new());

/// Encodes the NBT of mod components with Java's component codecs; unknown types are skipped by Java.
pub fn encode_components(components: &NbtCompound) -> Result<Arc<[WireComponent]>, Error> {
    let key = encode_nbt(components);
    let mut cache = COMPONENT_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(hit) = cache.get(&key) {
        return Ok(hit.clone());
    }
    let reply = play_unleased(&serde_json::json!({"op": "components_encode", "nbt": key}))?;
    let wire: Arc<[WireComponent]> = reply
        .as_array()
        .ok_or_else(|| Error::Runtime("component encoding returned no list".into()))?
        .iter()
        .map(|entry| {
            let id = entry["id"]
                .as_i64()
                .and_then(|id| i32::try_from(id).ok())
                .ok_or_else(|| Error::Runtime("component id missing".into()))?;
            let data = BASE64_STANDARD
                .decode(entry["data"].as_str().unwrap_or_default())
                .map_err(|e| Error::Runtime(e.to_string()))?;
            Ok((id, data))
        })
        .collect::<Result<_, Error>>()?;
    if cache.len() >= MAX_CACHED_COMPONENT_SETS {
        cache.clear();
    }
    cache.insert(key, wire.clone());
    Ok(wire)
}

/// Decodes client component values to NBT by component name; types Java does not know are skipped.
pub fn decode_components(wire: &[WireComponent]) -> Result<NbtCompound, Error> {
    let components: Vec<Value> = wire
        .iter()
        .map(|(id, data)| serde_json::json!({"id": id, "data": BASE64_STANDARD.encode(data)}))
        .collect();
    let reply =
        play_unleased(&serde_json::json!({"op": "components_decode", "components": components}))?;
    decode_nbt(reply["nbt"].as_str().unwrap_or_default())
}

use bytes::Bytes;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::particle::Particle as P;
use pumpkin_data::registry_ids::REGISTRY_ENTRIES;
use pumpkin_nbt::{serializer::NbtWriteHelperJava, tag::NbtTag};
use pumpkin_protocol::{
    ConnectionState, RawPacket, VarInt,
    ser::{NetworkReadExt, NetworkReadSliceExt, NetworkWriteExt},
};
use pumpkin_util::version::JavaMinecraftVersion;
use std::{collections::HashMap, sync::LazyLock};
use tracing::warn;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const fn phase(state: ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Config => Some("config"),
        ConnectionState::Play => Some("play"),
        _ => None,
    }
}
struct Tables {
    to_client: HashMap<&'static str, Vec<i32>>,
    to_engine: HashMap<&'static str, Vec<i32>>,
}
static TABLES: LazyLock<Tables> = LazyLock::new(Tables::build);
fn set(table: &mut Vec<i32>, index: i32, value: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    if table.len() <= index {
        table.resize(index + 1, -1);
    }
    table[index] = value;
}
impl Tables {
    fn build() -> Self {
        let mut to_client = HashMap::new();
        let mut to_engine = HashMap::new();
        for &(registry, names) in REGISTRY_ENTRIES {
            let client = names
                .iter()
                .enumerate()
                .map(|(id, name)| {
                    ferrum_neoforge::world::synced_registry_id(
                        registry,
                        &format!("minecraft:{name}"),
                    )
                    .unwrap_or(id as i32)
                })
                .collect::<Vec<_>>();
            to_client.insert(registry, client);
        }
        for registry in ["item", "block", "block_entity_type", "entity_type"] {
            let client = to_client.entry(registry).or_default();
            for (engine_id, wire) in ferrum_neoforge::world::runtime_ids(registry) {
                set(
                    client,
                    i32::from(engine_id),
                    i32::try_from(wire).unwrap_or(-1),
                );
            }
        }
        for (&registry, client) in &to_client {
            let mut engine = Vec::new();
            for (id, &wire) in client.iter().enumerate().rev() {
                set(&mut engine, wire, id as i32);
            }
            to_engine.insert(registry, engine);
        }
        Self {
            to_client,
            to_engine,
        }
    }
}

fn lookup(table: &[i32], id: i32) -> Option<i32> {
    table
        .get(usize::try_from(id).ok()?)
        .copied()
        .filter(|id| *id >= 0)
}

fn mapped(registry: &str, id: i32) -> Option<i32> {
    lookup(TABLES.to_client.get(registry)?, id)
}

fn registry_id(registry: &str, id: i32) -> Result<i32> {
    mapped(registry, id)
        .ok_or_else(|| format!("26.2 has no {registry} entry for engine ID {id}").into())
}

fn block_state(id: i32) -> Result<i32> {
    if id < 0 {
        return Err("Invalid engine block state".into());
    }
    if id < i32::from(pumpkin_data::BlockStateId::COUNT) {
        return Ok(id);
    }
    ferrum_neoforge::world::java_state_raw(u16::try_from(id)?)
        .ok_or_else(|| "Unknown mod block state".into())
}

pub(super) fn incoming(
    version: JavaMinecraftVersion,
    state: ConnectionState,
    packet: &RawPacket,
) -> Result<RawPacket> {
    let id = packet.id;
    let mut payload = packet.payload.clone();
    if version == JavaMinecraftVersion::V_26_2 && state == ConnectionState::Play {
        use pumpkin_data::packet::serverbound::play as p;
        let mut input = &payload[..];
        let mut output = Vec::new();
        if id == p::SET_CREATIVE_MODE_SLOT.0 {
            copy(&mut input, &mut output, 2)?;
            creative_item(&mut input, &mut output)?;
            if !input.is_empty() {
                return Err("Trailing creative item data".into());
            }
            payload = output.into();
        } else if id == p::CONTAINER_CLICK.0 {
            output.write_var_int(&input.get_var_int()?)?;
            output.write_var_int(&input.get_var_int()?)?;
            copy(&mut input, &mut output, 3)?;
            output.write_var_int(&input.get_var_int()?)?;
            let count = bounded_count(&mut input)?;
            output.write_var_int(&VarInt(count))?;
            for _ in 0..count {
                copy(&mut input, &mut output, 2)?;
                item_hash(&mut input, &mut output)?;
            }
            item_hash(&mut input, &mut output)?;
            if !input.is_empty() {
                return Err("Trailing container click data".into());
            }
            payload = output.into();
        }
    }
    Ok(RawPacket { id, payload })
}

#[expect(
    clippy::too_many_lines,
    reason = "One dispatch table for the 26.2 wire boundary"
)]
pub(super) fn outgoing(
    version: JavaMinecraftVersion,
    state: ConnectionState,
    packet: Bytes,
) -> Result<Option<Bytes>> {
    if version != JavaMinecraftVersion::V_26_2 {
        return Ok(Some(packet));
    }
    let Some(phase) = phase(state) else {
        return Ok(Some(packet));
    };
    let mut input = &packet[..];
    let id = input.get_var_int()?.0;
    let mut output = Vec::with_capacity(packet.len());
    output.write_var_int(&VarInt(id))?;
    if phase == "config" {
        if id == pumpkin_data::packet::clientbound::config::UPDATE_TAGS.0 {
            tags(&mut input, &mut output)?;
        } else {
            output.extend_from_slice(input);
        }
    } else {
        use pumpkin_data::packet::clientbound::play as p;
        match id {
            id if id == p::LEVEL_CHUNK_WITH_LIGHT.0 => chunk(&mut input, &mut output)?,
            id if id == p::BLOCK_UPDATE.0 => {
                copy(&mut input, &mut output, 8)?;
                output.write_var_int(&VarInt(block_state(input.get_var_int()?.0)?))?;
            }
            id if id == p::ADD_ENTITY.0 => {
                output.write_var_int(&input.get_var_int()?)?;
                copy(&mut input, &mut output, 16)?;
                output
                    .write_var_int(&VarInt(registry_id("entity_type", input.get_var_int()?.0)?))?;
                output.extend_from_slice(input);
            }
            id if id == p::UPDATE_ATTRIBUTES.0 => attributes(&mut input, &mut output)?,
            id if id == p::UPDATE_ADVANCEMENTS.0 => advancements(&mut input, &mut output)?,
            id if id == p::RECIPE_BOOK_ADD.0 => recipes(&mut input, &mut output)?,
            id if id == p::LOGIN.0 => login(&mut input, &mut output)?,
            id if id == p::RESPAWN.0 => spawn_data(&mut input, &mut output)?,
            id if id == p::CONTAINER_SET_CONTENT.0 => {
                output.write_var_int(&input.get_var_int()?)?;
                output.write_var_int(&input.get_var_int()?)?;
                let count = input.get_var_int()?;
                output.write_var_int(&count)?;
                for _ in 0..=count.0 {
                    item(&mut input, &mut output, false)?;
                }
            }
            id if id == p::CONTAINER_SET_SLOT.0 => {
                output.write_var_int(&input.get_var_int()?)?;
                output.write_var_int(&input.get_var_int()?)?;
                copy(&mut input, &mut output, 2)?;
                item(&mut input, &mut output, false)?;
            }
            id if id == p::SET_EQUIPMENT.0 => {
                output.write_var_int(&input.get_var_int()?)?;
                loop {
                    let slot = input.get_u8()?;
                    output.write_u8(slot)?;
                    item(&mut input, &mut output, false)?;
                    if slot & 128 == 0 {
                        break;
                    }
                }
            }
            id if id == p::SET_PLAYER_INVENTORY.0 => {
                output.write_var_int(&input.get_var_int()?)?;
                item(&mut input, &mut output, false)?;
            }
            id if id == p::SET_CURSOR_ITEM.0 => item(&mut input, &mut output, false)?,
            id if id == p::SOUND.0 || id == p::SOUND_ENTITY.0 => {
                sound(&mut input, &mut output)?;
                output.extend_from_slice(input);
            }
            id if id == p::SECTION_BLOCKS_UPDATE.0 => {
                copy(&mut input, &mut output, 8)?;
                let count = input.get_var_int()?;
                output.write_var_int(&count)?;
                for _ in 0..count.0 {
                    let packed = input.get_var_long()?.0 as u64;
                    let state = block_state((packed >> 12) as i32)? as u64;
                    output.write_var_long(&pumpkin_protocol::codec::var_long::VarLong(
                        ((state << 12) | (packed & 4095)) as i64,
                    ))?;
                }
            }
            id if id == p::BLOCK_EVENT.0 => {
                copy(&mut input, &mut output, 10)?;
                output.write_var_int(&VarInt(registry_id("block", input.get_var_int()?.0)?))?;
            }
            id if id == p::LEVEL_EVENT.0 => {
                use pumpkin_data::world::WorldEvent;
                let event = input.get_i32_be()?;
                output.write_i32_be(event)?;
                copy(&mut input, &mut output, 8)?;
                let data = input.get_i32_be()?;
                output.write_i32_be(if event == WorldEvent::ParticlesDestroyBlock as i32 {
                    block_state(data)?
                } else if event == WorldEvent::SoundPlayJukeboxSong as i32 {
                    registry_id("jukebox_song", data)?
                } else {
                    data
                })?;
                output.extend_from_slice(input);
            }
            id if id == p::SET_ENTITY_DATA.0 => entity_data(&mut input, &mut output)?,
            id if id == p::UPDATE_MOB_EFFECT.0 => {
                output.write_var_int(&input.get_var_int()?)?;
                output
                    .write_var_int(&VarInt(registry_id("mob_effect", input.get_var_int()?.0)?))?;
                output.extend_from_slice(input);
            }
            id if id == p::REMOVE_MOB_EFFECT.0 => {
                output.write_var_int(&input.get_var_int()?)?;
                output
                    .write_var_int(&VarInt(registry_id("mob_effect", input.get_var_int()?.0)?))?;
            }
            id if id == p::DAMAGE_EVENT.0 => {
                output.write_var_int(&input.get_var_int()?)?;
                output
                    .write_var_int(&VarInt(registry_id("damage_type", input.get_var_int()?.0)?))?;
                output.extend_from_slice(input);
            }
            id if id == p::BLOCK_ENTITY_DATA.0 => {
                copy(&mut input, &mut output, 8)?;
                output.write_var_int(&VarInt(registry_id(
                    "block_entity_type",
                    input.get_var_int()?.0,
                )?))?;
                output.extend_from_slice(input);
            }
            id if id == p::PLAYER_CHAT.0 => player_chat(&mut input, &mut output)?,
            id if id == p::DISGUISED_CHAT.0 => {
                let mut message = input;
                nbt(&mut input, &mut output)?;
                let chat_type = input.get_var_int()?.0;
                if is_raw_chat_type(chat_type) {
                    // The client has no raw chat type, so show the text as a system message.
                    let target = p::SYSTEM_CHAT.0;
                    output.clear();
                    output.write_var_int(&VarInt(target))?;
                    nbt(&mut message, &mut output)?;
                    output.write_bool(false)?;
                    return Ok(Some(output.into()));
                }
                output.write_var_int(&VarInt(chat_type_id(chat_type)?))?;
                nbt(&mut input, &mut output)?;
                let target = input.get_bool()?;
                output.write_bool(target)?;
                if target {
                    nbt(&mut input, &mut output)?;
                }
            }
            id if id == p::MERCHANT_OFFERS.0 => merchant_offers(&mut input, &mut output)?,
            id if id == p::LEVEL_PARTICLES.0 => particles(&mut input, &mut output)?,
            id if id == p::EXPLODE.0 => {
                copy(&mut input, &mut output, 32)?;
                let impulse = input.get_bool()?;
                output.write_bool(impulse)?;
                if impulse {
                    copy(&mut input, &mut output, 24)?;
                }
                particle_data(input.get_var_int()?.0, &mut input, &mut output)?;
                sound(&mut input, &mut output)?;
                output.write_var_int(&input.get_var_int()?)?;
            }
            _ => output.extend_from_slice(input),
        }
    }
    Ok(Some(output.into()))
}

fn copy(input: &mut &[u8], output: &mut Vec<u8>, length: usize) -> Result<()> {
    output.extend_from_slice(input.read_slice_borrowed(length)?);
    Ok(())
}

fn reverse_id(registry: &str, id: i32) -> Result<i32> {
    if registry == "item"
        && let Some(rust) = u32::try_from(id)
            .ok()
            .and_then(ferrum_neoforge::world::rust_item)
    {
        return Ok(i32::from(rust));
    }
    TABLES
        .to_engine
        .get(registry)
        .and_then(|ids| lookup(ids, id))
        .ok_or_else(|| format!("Unknown 26.2 {registry} ID {id}").into())
}

fn bounded_count(input: &mut &[u8]) -> Result<i32> {
    let count = input.get_var_int()?.0;
    if !(0..=256).contains(&count) {
        return Err("Too many item components or changed slots".into());
    }
    Ok(count)
}

fn creative_item(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    let count = input.get_var_int()?;
    output.write_var_int(&count)?;
    if count.0 == 0 {
        return Ok(());
    }
    u8::try_from(count.0)?;
    output.write_var_int(&VarInt(reverse_id("item", input.get_var_int()?.0)?))?;
    let added = bounded_count(input)?;
    let removed = bounded_count(input)?;
    let custom_data = i32::from(DataComponent::CustomData.to_id());
    let mut components = Vec::new();
    let mut custom = None;
    // Components the engine has no ID for belong to mods and travel as NBT in the stack's custom data.
    let mut mod_components = Vec::new();
    for _ in 0..added {
        let client_id = input.get_var_int()?.0;
        let length = usize::try_from(input.get_var_int()?.0)?;
        let value = input.read_slice_borrowed(length)?;
        match reverse_id("data_component_type", client_id) {
            Ok(id) if id == custom_data => {
                let mut value = value;
                custom = value.get_compound_nbt_borrowed(&JavaMinecraftVersion::V_26_2)?;
            }
            Ok(id) => match engine_component(id, value) {
                Ok(Some(value)) => components.push((id, value)),
                Ok(None) => {}
                Err(error) => warn!("Dropping client item component {client_id}: {error}"),
            },
            Err(_) => mod_components.push((client_id, value.to_vec())),
        }
    }
    let mut removals = Vec::new();
    for _ in 0..removed {
        if let Ok(id) = reverse_id("data_component_type", input.get_var_int()?.0) {
            removals.push(id);
        }
    }
    if !mod_components.is_empty() {
        match ferrum_neoforge::world::decode_components(&mod_components) {
            Ok(decoded) if !decoded.is_empty() => custom
                .get_or_insert_with(pumpkin_nbt::compound::NbtCompound::new)
                .put(ferrum_neoforge::world::MOD_COMPONENTS, decoded),
            Ok(_) => {}
            Err(error) => warn!("Dropping client mod components: {error}"),
        }
    }
    let mut custom_bytes = Vec::new();
    if let Some(custom) = custom {
        NbtTag::Compound(custom)
            .serialize(&mut NbtWriteHelperJava::new(&mut custom_bytes))
            .map_err(|error| error.to_string())?;
        components.push((custom_data, custom_bytes));
    }
    output.write_var_int(&VarInt(components.len() as i32))?;
    output.write_var_int(&VarInt(removals.len() as i32))?;
    for (id, value) in components {
        output.write_var_int(&VarInt(id))?;
        output.write_var_int(&VarInt(i32::try_from(value.len())?))?;
        output.extend_from_slice(&value);
    }
    for id in removals {
        output.write_var_int(&VarInt(id))?;
    }
    Ok(())
}

/// A client component value with the engine's registry IDs, the reverse of `component`; `None` when the engine
/// cannot hold it. The value must decode afterwards, since the engine rejects the whole packet otherwise.
fn engine_component(id: i32, mut value: &[u8]) -> Result<Option<Vec<u8>>> {
    use pumpkin_data::data_component::DataComponent as C;
    let kind = C::try_from_id(u8::try_from(id)?).ok_or("Unknown engine item component")?;
    let mut output = Vec::with_capacity(value.len());
    match kind {
        C::Enchantments | C::StoredEnchantments => {
            let count = bounded_count(&mut value)?;
            let mut kept = Vec::new();
            for _ in 0..count {
                let enchantment = value.get_var_int()?.0;
                let level = value.get_var_int()?;
                // Mod enchantments have no engine entry.
                if let Ok(enchantment) = reverse_id("enchantment", enchantment) {
                    kept.push((enchantment, level));
                }
            }
            output.write_var_int(&VarInt(i32::try_from(kept.len())?))?;
            for (enchantment, level) in kept {
                output.write_var_int(&VarInt(enchantment))?;
                output.write_var_int(&level)?;
            }
        }
        C::PotionContents => {
            let potion = value.get_bool()?;
            output.write_bool(potion)?;
            if potion {
                output.write_var_int(&VarInt(reverse_id("potion", value.get_var_int()?.0)?))?;
            }
        }
        _ => {}
    }
    output.extend_from_slice(value);
    let mut check = &output[..];
    pumpkin_protocol::codec::data_component::deserialize(kind, &mut check)?;
    if !check.is_empty() {
        return Err("Trailing component data".into());
    }
    Ok(Some(output))
}

/// Serverbound packets that carry a client item stack: a stack the engine cannot read is dropped, not kicked.
pub(super) const fn carries_item(id: i32) -> bool {
    use pumpkin_data::packet::serverbound::play as p;
    id == p::SET_CREATIVE_MODE_SLOT.0 || id == p::CONTAINER_CLICK.0
}

fn item_hash(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    let present = input.get_bool()?;
    output.write_bool(present)?;
    if !present {
        return Ok(());
    }
    output.write_var_int(&VarInt(reverse_id("item", input.get_var_int()?.0)?))?;
    output.write_var_int(&input.get_var_int()?)?;
    let added = bounded_count(input)?;
    let mut components = Vec::new();
    for _ in 0..added {
        let id = input.get_var_int()?.0;
        let hash = input.read_slice_borrowed(4)?;
        // Mod components have no engine ID; the engine never sees them as components.
        if let Ok(id) = reverse_id("data_component_type", id) {
            components.push((id, hash));
        }
    }
    output.write_var_int(&VarInt(components.len() as i32))?;
    for (id, hash) in components {
        output.write_var_int(&VarInt(id))?;
        output.extend_from_slice(hash);
    }
    let removed = bounded_count(input)?;
    let mut removals = Vec::new();
    for _ in 0..removed {
        if let Ok(id) = reverse_id("data_component_type", input.get_var_int()?.0) {
            removals.push(id);
        }
    }
    output.write_var_int(&VarInt(removals.len() as i32))?;
    for id in removals {
        output.write_var_int(&VarInt(id))?;
    }
    Ok(())
}

fn tags(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    let count = input.get_var_int()?.0;
    let mut registries = Vec::new();
    let mut retained = 0;
    for _ in 0..count {
        let name = input.get_str()?;
        let registry = name.strip_prefix("minecraft:").unwrap_or(&name);
        let count = input.get_var_int()?.0;
        let mut tag_data = Vec::new();
        for _ in 0..count {
            let name = input.get_str()?;
            let length = input.get_var_int()?.0;
            let mut ids = Vec::new();
            for _ in 0..length {
                let id = input.get_var_int()?.0;
                if let Some(id) = mapped(registry, id) {
                    ids.push(id);
                }
            }
            tag_data.write_string(&name)?;
            tag_data.write_var_int(&VarInt(ids.len() as i32))?;
            for id in ids {
                tag_data.write_var_int(&VarInt(id))?;
            }
        }
        if REGISTRY_ENTRIES.iter().any(|(key, _)| *key == registry) {
            registries.write_string(&name)?;
            registries.write_var_int(&VarInt(count))?;
            registries.extend_from_slice(&tag_data);
            retained += 1;
        }
    }
    output.write_var_int(&VarInt(retained))?;
    output.extend_from_slice(&registries);
    Ok(())
}

fn attributes(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    output.write_var_int(&input.get_var_int()?)?;
    let count = input.get_var_int()?.0;
    output.write_var_int(&VarInt(count))?;
    for _ in 0..count {
        output.write_var_int(&VarInt(registry_id("attribute", input.get_var_int()?.0)?))?;
        copy(input, output, 8)?;
        let modifiers = input.get_var_int()?;
        output.write_var_int(&modifiers)?;
        for _ in 0..modifiers.0 {
            output.write_string(&input.get_str()?)?;
            copy(input, output, 9)?;
        }
    }
    Ok(())
}

fn palette(input: &mut &[u8], output: &mut Vec<u8>, blocks: bool) -> Result<()> {
    let bits = input.get_u8()?;
    let size: usize = if blocks { 4096 } else { 64 };
    let direct = bits > if blocks { 8 } else { 3 };
    let target_bits = if blocks && direct {
        direct_state_bits()
    } else {
        bits
    };
    output.write_u8(target_bits)?;
    let map = |id| {
        if blocks {
            block_state(id)
        } else {
            registry_id("worldgen/biome", id)
        }
    };
    if bits == 0 {
        output.write_var_int(&VarInt(map(input.get_var_int()?.0)?))?;
        return Ok(());
    }
    if !direct {
        let count = input.get_var_int()?;
        output.write_var_int(&count)?;
        for _ in 0..count.0 {
            output.write_var_int(&VarInt(map(input.get_var_int()?.0)?))?;
        }
    }
    let per_long = 64 / usize::from(bits);
    let longs = size.div_ceil(per_long);
    if direct {
        let mut values = Vec::with_capacity(size);
        for _ in 0..longs {
            let word = input.get_i64_be()? as u64;
            for i in 0..per_long {
                if values.len() < size {
                    values.push(map(
                        ((word >> (i * usize::from(bits))) & ((1u64 << bits) - 1)) as i32
                    )? as u64);
                }
            }
        }
        let per_long = 64 / usize::from(target_bits);
        for values in values.chunks(per_long) {
            let word = values.iter().enumerate().fold(0, |word, (i, value)| {
                word | (value << (i * usize::from(target_bits)))
            });
            output.write_i64_be(word as i64)?;
        }
    } else {
        copy(input, output, longs * 8)?;
    }
    Ok(())
}

/// `Mth.ceillog2` of the client's block state registry size; mods enlarge it past 15 bits.
fn direct_state_bits() -> u8 {
    ferrum_neoforge::world::java_state_count().map_or(15, |count| {
        (usize::BITS - count.saturating_sub(1).leading_zeros()) as u8
    })
}

fn chunk(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    copy(input, output, 8)?;
    let heightmaps = input.get_var_int()?;
    output.write_var_int(&heightmaps)?;
    for _ in 0..heightmaps.0 {
        output.write_var_int(&input.get_var_int()?)?;
        let count = input.get_var_int()?;
        output.write_var_int(&count)?;
        copy(input, output, usize::try_from(count.0)? * 8)?;
    }
    let size = usize::try_from(input.get_var_int()?.0)?;
    let mut sections = input.read_slice_borrowed(size)?;
    let mut translated = Vec::with_capacity(size);
    while !sections.is_empty() {
        copy(&mut sections, &mut translated, 4)?;
        palette(&mut sections, &mut translated, true)?;
        palette(&mut sections, &mut translated, false)?;
    }
    output.write_var_int(&VarInt(translated.len() as i32))?;
    output.extend_from_slice(&translated);
    let count = input.get_var_int()?;
    let mut entities = Vec::new();
    let mut retained = 0;
    for _ in 0..count.0 {
        let position = input.read_slice_borrowed(3)?;
        let id = input.get_var_int()?.0;
        let mut data = Vec::new();
        nbt(input, &mut data)?;
        if let Some(id) = mapped("block_entity_type", id) {
            entities.extend_from_slice(position);
            entities.write_var_int(&VarInt(id))?;
            entities.extend_from_slice(&data);
            retained += 1;
        }
    }
    output.write_var_int(&VarInt(retained))?;
    output.extend_from_slice(&entities);
    output.extend_from_slice(input);
    Ok(())
}

fn nbt(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    let before = *input;
    input.get_nbt(&JavaMinecraftVersion::V_26_2)?;
    output.extend_from_slice(&before[..before.len() - input.len()]);
    Ok(())
}

fn item(input: &mut &[u8], output: &mut Vec<u8>, template: bool) -> Result<()> {
    let first = input.get_var_int()?;
    if !template && first.0 == 0 {
        output.write_var_int(&first)?;
        return Ok(());
    }
    let second = input.get_var_int()?;
    let (id, count) = if template {
        (first, second)
    } else {
        (second, first)
    };
    // Rust holds a mod item's vanilla prototype values only for its own gameplay; the client has the real ones.
    let modded = u16::try_from(id.0)
        .ok()
        .and_then(pumpkin_data::runtime::item)
        .is_some();
    let id = mapped("item", id.0).ok_or("Missing barrier item mapping")?;
    if template {
        output.write_var_int(&VarInt(id))?;
        output.write_var_int(&count)?;
    } else {
        output.write_var_int(&count)?;
        output.write_var_int(&VarInt(id))?;
    }
    let added = input.get_var_int()?.0;
    let removed = input.get_var_int()?.0;
    let custom_data = i32::from(DataComponent::CustomData.to_id());
    let mut engine_components = Vec::new();
    let mut custom = None;
    let mut mod_components = None;
    let damage = i32::from(DataComponent::Damage.to_id());
    for _ in 0..added {
        let mut peek = *input;
        let id = peek.get_var_int()?.0;
        if id != custom_data {
            let mut value = Vec::new();
            if component(input, &mut value)? {
                engine_components.push((id, value));
            }
            continue;
        }
        input.get_var_int()?;
        let mut data = input
            .get_compound_nbt_borrowed(&JavaMinecraftVersion::V_26_2)?
            .unwrap_or_default();
        // Mod components wait in custom data for the client's real typed components.
        if let Some(NbtTag::Compound(components)) = data
            .child_tags
            .remove(ferrum_neoforge::world::MOD_COMPONENTS)
        {
            match ferrum_neoforge::world::encode_components(&components) {
                Ok(wire) => mod_components = Some(wire),
                Err(error) => warn!("Dropping mod components from a stack: {error}"),
            }
        }
        data.child_tags
            .remove(ferrum_neoforge::world::PROTOTYPE_COMPONENTS);
        custom = Some(data);
    }
    let wire = mod_components.as_deref().unwrap_or_default();
    let mut add_data = Vec::new();
    let mut add_count = 0;
    for (id, value) in engine_components {
        // Java sends every other change to a mod item, and its encoding of a component is exact.
        if modded && id != damage
            || mapped("data_component_type", id)
                .is_some_and(|target| wire.iter().any(|(java, _)| *java == target))
        {
            continue;
        }
        add_data.extend_from_slice(&value);
        add_count += 1;
    }
    if let Some(data) = custom.filter(|data| !data.is_empty())
        && let Some(target) = mapped("data_component_type", custom_data)
    {
        add_data.write_var_int(&VarInt(target))?;
        NbtTag::Compound(data)
            .serialize(&mut NbtWriteHelperJava::new(&mut add_data))
            .map_err(|error| error.to_string())?;
        add_count += 1;
    }
    for (id, value) in wire {
        add_data.write_var_int(&VarInt(*id))?;
        add_data.extend_from_slice(value);
        add_count += 1;
    }
    let mut remove_ids = Vec::new();
    for _ in 0..removed {
        if let Some(id) = mapped("data_component_type", input.get_var_int()?.0) {
            remove_ids.push(id);
        }
    }
    output.write_var_int(&VarInt(add_count))?;
    output.write_var_int(&VarInt(remove_ids.len() as i32))?;
    output.extend_from_slice(&add_data);
    for id in remove_ids {
        output.write_var_int(&VarInt(id))?;
    }
    Ok(())
}

/// Writes one component (type and value) with 26.2 IDs; false when 26.2 has no such component.
fn component(input: &mut &[u8], output: &mut Vec<u8>) -> Result<bool> {
    use pumpkin_data::data_component::DataComponent as C;
    let id = input.get_var_int()?.0;
    let kind = C::try_from_id(u8::try_from(id)?).ok_or("Unknown engine item component")?;
    let start = *input;
    pumpkin_protocol::codec::data_component::deserialize(kind, input)?;
    let Some(target) = mapped("data_component_type", id) else {
        return Ok(false);
    };
    let mut value = &start[..start.len() - input.len()];
    output.write_var_int(&VarInt(target))?;
    match kind {
        C::Enchantments | C::StoredEnchantments => {
            let count = bounded_count(&mut value)?;
            output.write_var_int(&VarInt(count))?;
            for _ in 0..count {
                output
                    .write_var_int(&VarInt(registry_id("enchantment", value.get_var_int()?.0)?))?;
                output.write_var_int(&value.get_var_int()?)?;
            }
        }
        // 26.2 holds a damage type holder set: a leading 0 names a tag, without the `#`.
        C::PotionContents => {
            let potion = value.get_bool()?;
            output.write_bool(potion)?;
            if potion {
                output.write_var_int(&VarInt(registry_id("potion", value.get_var_int()?.0)?))?;
            }
        }
        _ => {}
    }
    output.extend_from_slice(value);
    Ok(true)
}

/// `TradeCost`: the item and count of a trade input plus the components it must match.
fn item_cost(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    output.write_var_int(&VarInt(registry_id("item", input.get_var_int()?.0)?))?;
    output.write_var_int(&input.get_var_int()?)?;
    let count = bounded_count(input)?;
    let mut components = Vec::new();
    let mut kept = 0;
    for _ in 0..count {
        if component(input, &mut components)? {
            kept += 1;
        }
    }
    output.write_var_int(&VarInt(kept))?;
    output.extend_from_slice(&components);
    Ok(())
}

fn merchant_offers(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    output.write_var_int(&input.get_var_int()?)?;
    let count = bounded_count(input)?;
    output.write_var_int(&VarInt(count))?;
    for _ in 0..count {
        item_cost(input, output)?;
        item(input, output, false)?;
        let second = input.get_bool()?;
        output.write_bool(second)?;
        if second {
            item_cost(input, output)?;
        }
        // Reward flag, uses, max uses, xp, special price, multiplier and demand.
        copy(input, output, 25)?;
    }
    output.extend_from_slice(input);
    Ok(())
}

/// Engine-only `raw` chat type, which no 26.2 client registry contains.
fn is_raw_chat_type(id: i32) -> bool {
    pumpkin_data::registry::REGISTRY_V_26_2
        .iter()
        .find(|reg| reg.registry_id == "chat_type")
        .and_then(|reg| reg.entries.get(usize::try_from(id).ok()?))
        .is_some_and(|entry| entry.name == "raw")
}

fn chat_type_id(id: i32) -> Result<i32> {
    if is_raw_chat_type(id) {
        return ferrum_neoforge::world::synced_registry_id("chat_type", "minecraft:chat")
            .ok_or_else(|| "Client has no chat chat type".into());
    }
    registry_id("chat_type", id)
}

fn player_chat(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    output.write_var_int(&input.get_var_int()?)?;
    copy(input, output, 16)?;
    output.write_var_int(&input.get_var_int()?)?;
    let signed = input.get_bool()?;
    output.write_bool(signed)?;
    if signed {
        copy(input, output, 256)?;
    }
    output.write_string(&input.get_str()?)?;
    copy(input, output, 16)?;
    let previous = bounded_count(input)?;
    output.write_var_int(&VarInt(previous))?;
    for _ in 0..previous {
        let id = input.get_var_int()?;
        output.write_var_int(&id)?;
        if id.0 == 0 {
            copy(input, output, 256)?;
        }
    }
    let unsigned = input.get_bool()?;
    output.write_bool(unsigned)?;
    if unsigned {
        nbt(input, output)?;
    }
    let filter = input.get_var_int()?;
    output.write_var_int(&filter)?;
    if filter.0 == 2 {
        let longs = bounded_count(input)?;
        output.write_var_int(&VarInt(longs))?;
        copy(input, output, usize::try_from(longs)? * 8)?;
    }
    output.write_var_int(&VarInt(chat_type_id(input.get_var_int()?.0)?))?;
    nbt(input, output)?;
    let target = input.get_bool()?;
    output.write_bool(target)?;
    if target {
        nbt(input, output)?;
    }
    Ok(())
}

/// Serializer IDs of 26.2's `EntityDataSerializers`, in registration order.
mod data_type {
    pub const BYTE: i32 = pumpkin_data::meta_data_type::MetaDataType::BYTE.id;
    pub const INT: i32 = pumpkin_data::meta_data_type::MetaDataType::INT.id;
    pub const LONG: i32 = pumpkin_data::meta_data_type::MetaDataType::LONG.id;
    pub const FLOAT: i32 = pumpkin_data::meta_data_type::MetaDataType::FLOAT.id;
    pub const STRING: i32 = pumpkin_data::meta_data_type::MetaDataType::STRING.id;
    pub const COMPONENT: i32 = pumpkin_data::meta_data_type::MetaDataType::COMPONENT.id;
    pub const OPTIONAL_COMPONENT: i32 =
        pumpkin_data::meta_data_type::MetaDataType::OPTIONAL_COMPONENT.id;
    pub const ITEM_STACK: i32 = pumpkin_data::meta_data_type::MetaDataType::ITEM_STACK.id;
    pub const BOOLEAN: i32 = pumpkin_data::meta_data_type::MetaDataType::BOOLEAN.id;
    pub const ROTATIONS: i32 = pumpkin_data::meta_data_type::MetaDataType::ROTATIONS.id;
    pub const BLOCK_POS: i32 = pumpkin_data::meta_data_type::MetaDataType::BLOCK_POS.id;
    pub const OPTIONAL_BLOCK_POS: i32 =
        pumpkin_data::meta_data_type::MetaDataType::OPTIONAL_BLOCK_POS.id;
    pub const DIRECTION: i32 = pumpkin_data::meta_data_type::MetaDataType::DIRECTION.id;
    pub const OPTIONAL_LIVING_ENTITY_REFERENCE: i32 =
        pumpkin_data::meta_data_type::MetaDataType::OPTIONAL_LIVING_ENTITY_REFERENCE.id;
    pub const BLOCK_STATE: i32 = pumpkin_data::meta_data_type::MetaDataType::BLOCK_STATE.id;
    pub const OPTIONAL_BLOCK_STATE: i32 =
        pumpkin_data::meta_data_type::MetaDataType::OPTIONAL_BLOCK_STATE.id;
    pub const PARTICLE: i32 = pumpkin_data::meta_data_type::MetaDataType::PARTICLE.id;
    pub const PARTICLES: i32 = pumpkin_data::meta_data_type::MetaDataType::PARTICLES.id;
    pub const VILLAGER_DATA: i32 = pumpkin_data::meta_data_type::MetaDataType::VILLAGER_DATA.id;
    pub const OPTIONAL_UNSIGNED_INT: i32 =
        pumpkin_data::meta_data_type::MetaDataType::OPTIONAL_UNSIGNED_INT.id;
    pub const POSE: i32 = pumpkin_data::meta_data_type::MetaDataType::POSE.id;
    pub const CAT_VARIANT: i32 = pumpkin_data::meta_data_type::MetaDataType::CAT_VARIANT.id;
    pub const ZOMBIE_NAUTILUS_VARIANT: i32 =
        pumpkin_data::meta_data_type::MetaDataType::ZOMBIE_NAUTILUS_VARIANT.id;
    pub const OPTIONAL_GLOBAL_POS: i32 =
        pumpkin_data::meta_data_type::MetaDataType::OPTIONAL_GLOBAL_POS.id;
    pub const PAINTING_VARIANT: i32 =
        pumpkin_data::meta_data_type::MetaDataType::PAINTING_VARIANT.id;
    pub const SNIFFER_STATE: i32 = pumpkin_data::meta_data_type::MetaDataType::SNIFFER_STATE.id;
    pub const ARMADILLO_STATE: i32 = pumpkin_data::meta_data_type::MetaDataType::ARMADILLO_STATE.id;
    pub const COPPER_GOLEM_STATE: i32 =
        pumpkin_data::meta_data_type::MetaDataType::COPPER_GOLEM_STATE.id;
    pub const WEATHERING_COPPER_STATE: i32 =
        pumpkin_data::meta_data_type::MetaDataType::WEATHERING_COPPER_STATE.id;
    pub const VECTOR3: i32 = pumpkin_data::meta_data_type::MetaDataType::VECTOR3.id;
    pub const QUATERNION: i32 = pumpkin_data::meta_data_type::MetaDataType::QUATERNION.id;
    pub const RESOLVABLE_PROFILE: i32 =
        pumpkin_data::meta_data_type::MetaDataType::RESOLVABLE_PROFILE.id;
    pub const HUMANOID_ARM: i32 = pumpkin_data::meta_data_type::MetaDataType::HUMANOID_ARM.id;
    /// Registries of `CAT_VARIANT..=ZOMBIE_NAUTILUS_VARIANT`.
    pub const VARIANT_REGISTRIES: [&str; 12] = [
        "cat_variant",
        "cat_sound_variant",
        "cow_variant",
        "cow_sound_variant",
        "wolf_variant",
        "wolf_sound_variant",
        "frog_variant",
        "pig_variant",
        "pig_sound_variant",
        "chicken_variant",
        "chicken_sound_variant",
        "zombie_nautilus_variant",
    ];
}

const END_OF_ENTITY_DATA: u8 = 0xFF;

fn entity_data(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    output.write_var_int(&input.get_var_int()?)?;
    loop {
        let index = input.get_u8()?;
        if index == END_OF_ENTITY_DATA {
            output.write_u8(index)?;
            return Ok(());
        }
        let kind = input.get_var_int()?.0;
        let mut value = Vec::new();
        if entity_data_value(kind, input, &mut value)? {
            output.write_u8(index)?;
            output.write_var_int(&VarInt(kind))?;
            output.extend_from_slice(&value);
        }
    }
}

/// Writes one entity data value with 26.2 IDs; false when the client has no such serializer.
fn entity_data_value(kind: i32, input: &mut &[u8], output: &mut Vec<u8>) -> Result<bool> {
    use data_type as t;
    match kind {
        t::BYTE | t::BOOLEAN => copy(input, output, 1)?,
        t::INT
        | t::DIRECTION
        | t::OPTIONAL_UNSIGNED_INT
        | t::POSE
        | t::SNIFFER_STATE
        | t::ARMADILLO_STATE
        | t::COPPER_GOLEM_STATE
        | t::WEATHERING_COPPER_STATE
        | t::HUMANOID_ARM => output.write_var_int(&input.get_var_int()?)?,
        t::LONG => output.write_var_long(&input.get_var_long()?)?,
        t::FLOAT => copy(input, output, 4)?,
        t::STRING => output.write_string(&input.get_str()?)?,
        t::COMPONENT => nbt(input, output)?,
        t::OPTIONAL_COMPONENT => {
            let present = input.get_bool()?;
            output.write_bool(present)?;
            if present {
                nbt(input, output)?;
            }
        }
        t::ITEM_STACK => item(input, output, false)?,
        t::ROTATIONS | t::VECTOR3 => copy(input, output, 12)?,
        t::QUATERNION => copy(input, output, 16)?,
        t::BLOCK_POS => copy(input, output, 8)?,
        t::OPTIONAL_BLOCK_POS => {
            let present = input.get_bool()?;
            output.write_bool(present)?;
            if present {
                copy(input, output, 8)?;
            }
        }
        t::OPTIONAL_LIVING_ENTITY_REFERENCE => {
            let present = input.get_bool()?;
            output.write_bool(present)?;
            if present {
                copy(input, output, 16)?;
            }
        }
        t::BLOCK_STATE => output.write_var_int(&VarInt(block_state(input.get_var_int()?.0)?))?,
        t::OPTIONAL_BLOCK_STATE => {
            // Zero is the absent state.
            let state = input.get_var_int()?.0;
            output.write_var_int(&VarInt(if state == 0 { 0 } else { block_state(state)? }))?;
        }
        t::PARTICLE => {
            let id = input.get_var_int()?.0;
            particle_data(id, input, output)?;
        }
        t::PARTICLES => {
            let count = bounded_count(input)?;
            output.write_var_int(&VarInt(count))?;
            for _ in 0..count {
                let id = input.get_var_int()?.0;
                particle_data(id, input, output)?;
            }
        }
        t::VILLAGER_DATA => {
            // Villager types and professions keep vanilla's order in both versions.
            for _ in 0..3 {
                output.write_var_int(&input.get_var_int()?)?;
            }
        }
        t::CAT_VARIANT..=t::ZOMBIE_NAUTILUS_VARIANT => {
            let registry = t::VARIANT_REGISTRIES[usize::try_from(kind - t::CAT_VARIANT)?];
            output.write_var_int(&VarInt(registry_id(registry, input.get_var_int()?.0)?))?;
        }
        t::OPTIONAL_GLOBAL_POS => {
            let present = input.get_bool()?;
            output.write_bool(present)?;
            if present {
                output.write_string(&input.get_str()?)?;
                copy(input, output, 8)?;
            }
        }
        t::PAINTING_VARIANT => {
            // A holder: zero marks an inline variant, otherwise the registry ID plus one.
            let holder = input.get_var_int()?.0;
            if holder == 0 {
                return Err("Unsupported inline painting variant".into());
            }
            output.write_var_int(&VarInt(registry_id("painting_variant", holder - 1)? + 1))?;
        }
        t::RESOLVABLE_PROFILE => return Err("Unsupported resolvable profile entity data".into()),
        _ => return Err(format!("Unknown entity data serializer {kind}").into()),
    }
    Ok(true)
}

fn advancements(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    copy(input, output, 1)?;
    let added = input.get_var_int()?;
    output.write_var_int(&added)?;
    for _ in 0..added.0 {
        output.write_string(&input.get_str()?)?;
        let parent = input.get_bool()?;
        output.write_bool(parent)?;
        if parent {
            output.write_string(&input.get_str()?)?;
        }
        let display = input.get_bool()?;
        output.write_bool(display)?;
        if display {
            nbt(input, output)?;
            nbt(input, output)?;
            item(input, output, true)?;
            output.write_var_int(&input.get_var_int()?)?;
            let flags = input.get_i32_be()?;
            output.write_i32_be(flags)?;
            if flags & 1 != 0 {
                output.write_string(&input.get_str()?)?;
            }
            copy(input, output, 8)?;
        }
        let start = *input;
        let requirements = input.get_var_int()?.0;
        for _ in 0..requirements {
            let count = input.get_var_int()?.0;
            for _ in 0..count {
                input.get_str()?;
            }
        }
        input.get_bool()?;
        let requirements = &start[..start.len() - input.len()];
        output.extend_from_slice(requirements);
    }
    output.extend_from_slice(input);
    Ok(())
}

fn slot(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    let kind = input.get_var_int()?;
    output.write_var_int(&kind)?;
    match kind.0 {
        0 | 1 => (),
        4 => output.write_var_int(&VarInt(
            mapped("item", input.get_var_int()?.0).ok_or("Missing item mapping")?,
        ))?,
        5 => item(input, output, true)?,
        6 => output.write_string(&input.get_str()?)?,
        9 => {
            slot(input, output)?;
            slot(input, output)?;
        }
        10 => {
            let count = input.get_var_int()?;
            output.write_var_int(&count)?;
            for _ in 0..count.0 {
                slot(input, output)?;
            }
        }
        _ => return Err(format!("Unsupported recipe slot display {}", kind.0).into()),
    }
    Ok(())
}

fn recipes(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    let count = input.get_var_int()?;
    output.write_var_int(&count)?;
    for _ in 0..count.0 {
        output.write_var_int(&input.get_var_int()?)?;
        let display = input.get_var_int()?;
        output.write_var_int(&display)?;
        match display.0 {
            0 | 1 => {
                if display.0 == 1 {
                    output.write_var_int(&input.get_var_int()?)?;
                    output.write_var_int(&input.get_var_int()?)?;
                }
                let count = input.get_var_int()?;
                output.write_var_int(&count)?;
                for _ in 0..count.0 {
                    slot(input, output)?;
                }
                slot(input, output)?;
                slot(input, output)?;
            }
            2 => {
                for _ in 0..4 {
                    slot(input, output)?;
                }
                output.write_var_int(&input.get_var_int()?)?;
                copy(input, output, 4)?;
            }
            3 => {
                for _ in 0..3 {
                    slot(input, output)?;
                }
            }
            4 => {
                for _ in 0..5 {
                    slot(input, output)?;
                }
            }
            _ => return Err("Unsupported recipe display".into()),
        }
        output.write_var_int(&input.get_var_int()?)?;
        output.write_var_int(&input.get_var_int()?)?;
        let requirements = input.get_bool()?;
        output.write_bool(requirements)?;
        if requirements {
            let count = input.get_var_int()?;
            output.write_var_int(&count)?;
            for _ in 0..count.0 {
                let size = input.get_var_int()?;
                output.write_var_int(&size)?;
                if size.0 == 0 {
                    output.write_string(&input.get_str()?)?;
                } else {
                    for _ in 1..size.0 {
                        let id = input.get_var_int()?.0;
                        output.write_var_int(&VarInt(
                            mapped("item", id)
                                .or_else(|| {
                                    mapped("item", i32::from(pumpkin_data::item::Item::BARRIER.id))
                                })
                                .ok_or("Missing item mapping")?,
                        ))?;
                    }
                }
            }
        }
        copy(input, output, 1)?;
    }
    copy(input, output, 1)?;
    Ok(())
}

fn sound(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    let id = input.get_var_int()?.0;
    if id == 0 {
        output.write_var_int(&VarInt(0))?;
        output.write_string(&input.get_str()?)?;
        let fixed = input.get_bool()?;
        output.write_bool(fixed)?;
        if fixed {
            copy(input, output, 4)?;
        }
    } else {
        output.write_var_int(&VarInt(registry_id("sound_event", id - 1)? + 1))?;
    }
    Ok(())
}

fn login(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    copy(input, output, 5)?;
    let dimensions = input.get_var_int()?;
    output.write_var_int(&dimensions)?;
    for _ in 0..dimensions.0 {
        output.write_string(&input.get_str()?)?;
    }
    for _ in 0..3 {
        output.write_var_int(&input.get_var_int()?)?;
    }
    copy(input, output, 3)?;
    spawn_data(input, output)
}

fn spawn_data(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    output.write_var_int(&VarInt(registry_id(
        "dimension_type",
        input.get_var_int()?.0,
    )?))?;
    output.write_string(&input.get_str()?)?;
    copy(input, output, 8)?;
    copy(input, output, 2)?;
    output.extend_from_slice(input);
    Ok(())
}

fn particles(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    copy(input, output, 46)?;
    particle_data(input.get_var_int()?.0, input, output)
}

fn particle_data(id: i32, input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    output.write_var_int(&VarInt(registry_id("particle_type", id)?))?;
    let particle = pumpkin_data::particle::Particle::from_id(u16::try_from(id)?)
        .ok_or("Invalid particle ID")?;
    match particle {
        P::Block | P::BlockMarker | P::FallingDust | P::DustPillar | P::BlockCrumble => {
            output.write_var_int(&VarInt(block_state(input.get_var_int()?.0)?))?;
            // NeoForge's `BlockParticleOption` appends an optional `BlockPos` (absent here).
            output.write_bool(false)?;
        }
        P::Dust => copy(input, output, 8)?,
        P::DustColorTransition => copy(input, output, 12)?,
        P::EntityEffect | P::SculkCharge => copy(input, output, 4)?,
        P::Item => item(input, output, false)?,
        P::Shriek => {
            output.write_var_int(&input.get_var_int()?)?;
        }
        P::Trail => {
            copy(input, output, 28)?;
            output.write_var_int(&input.get_var_int()?)?;
        }
        P::Vibration => {
            let source = input.get_var_int()?;
            output.write_var_int(&source)?;
            match source.0 {
                0 => copy(input, output, 8)?,
                1 => {
                    output.write_var_int(&input.get_var_int()?)?;
                    copy(input, output, 4)?;
                }
                _ => return Err("Invalid vibration source".into()),
            }
            output.write_var_int(&input.get_var_int()?)?;
        }
        _ => (),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_protocol::java::client::play::CUpdateEntityRot;
    use pumpkin_protocol::java::server::play::SSetCreativeSlot;
    use pumpkin_protocol::{ClientPacket, ServerPacket};

    #[test]
    fn native_movement_keeps_packet_id_and_layout() -> Result<()> {
        let mut packet = Vec::new();
        packet.write_var_int(&VarInt(
            pumpkin_data::packet::clientbound::play::MOVE_ENTITY_ROT.0,
        ))?;
        CUpdateEntityRot::new(VarInt(17), 90, 45, true)
            .write_packet_data(&mut packet, &JavaMinecraftVersion::V_26_2)?;
        let original = Bytes::from(packet);
        assert_eq!(
            outgoing(
                JavaMinecraftVersion::V_26_2,
                ConnectionState::Play,
                original.clone()
            )?,
            Some(original)
        );
        Ok(())
    }

    #[test]
    fn creative_stack_keeps_native_item_and_components_and_rejects_truncation() -> Result<()> {
        let mut payload = Vec::new();
        payload.write_i16_be(36)?;
        payload.write_var_int(&VarInt(1))?;
        payload.write_var_int(&VarInt(i32::from(pumpkin_data::item::Item::ACACIA_BOAT.id)))?;
        payload.write_var_int(&VarInt(1))?;
        payload.write_var_int(&VarInt(0))?;
        payload.write_var_int(&VarInt(i32::from(DataComponent::Damage.to_id())))?;
        payload.extend_from_slice(&[1, 5]);
        let packet = RawPacket {
            id: pumpkin_data::packet::serverbound::play::SET_CREATIVE_MODE_SLOT.0,
            payload: payload.clone().into(),
        };
        let mapped = incoming(JavaMinecraftVersion::V_26_2, ConnectionState::Play, &packet)?;
        assert_eq!(mapped.id, packet.id);
        assert_eq!(mapped.payload, packet.payload);
        let mut data = &mapped.payload[..];
        let stack = SSetCreativeSlot::read(&mut data, &JavaMinecraftVersion::V_26_2)?;
        assert_eq!(
            stack.clicked_item.0.item.id,
            pumpkin_data::item::Item::ACACIA_BOAT.id
        );
        assert_eq!(stack.clicked_item.0.get_damage(), 5);
        assert!(data.is_empty());
        payload.pop();
        let truncated = RawPacket {
            id: packet.id,
            payload: payload.into(),
        };
        assert!(
            incoming(
                JavaMinecraftVersion::V_26_2,
                ConnectionState::Play,
                &truncated
            )
            .is_err()
        );
        Ok(())
    }
}

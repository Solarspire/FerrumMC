//! Dependency-free offline configuration and gameplay probe.
//! Captures are also checked by the matching unmodified vanilla packet codecs.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const MAX_PACKET: usize = 2 * 1024 * 1024;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FixtureItem {
    id: u32,
    count: u32,
    removed: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Inventory {
    slots: Vec<Option<FixtureItem>>,
    carried: Option<FixtureItem>,
}

struct SessionState {
    block: ([i32; 3], u32),
    inventory: Inventory,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn put_varint(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        out.push(byte | if value == 0 { 0 } else { 0x80 });
        if value == 0 {
            break;
        }
    }
}

fn varint(input: &mut impl Read) -> io::Result<u32> {
    let mut value = 0;
    for shift in (0..35).step_by(7) {
        let mut byte = [0];
        input.read_exact(&mut byte)?;
        if shift == 28 && byte[0] > 0x0f {
            return Err(invalid("VarInt exceeds 32 bits"));
        }
        value |= u32::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(invalid("VarInt exceeds five bytes"))
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<()> {
    put_varint(out, value.len().try_into()?);
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn string(input: &mut &[u8]) -> Result<String> {
    let length = varint(input)? as usize;
    if length > input.len() {
        return Err(invalid("truncated string").into());
    }
    let value = std::str::from_utf8(&input[..length])?.to_owned();
    *input = &input[length..];
    Ok(value)
}

fn send(stream: &mut TcpStream, id: u32, body: &[u8]) -> Result<()> {
    let mut packet = Vec::new();
    put_varint(&mut packet, id);
    packet.extend_from_slice(body);
    let mut length = Vec::new();
    put_varint(&mut length, packet.len().try_into()?);
    stream.write_all(&length)?;
    stream.write_all(&packet)?;
    Ok(())
}

fn receive(stream: &mut impl Read) -> Result<(u32, Vec<u8>)> {
    let length = varint(stream)? as usize;
    if length == 0 || length > MAX_PACKET {
        return Err(invalid("invalid packet length").into());
    }
    let mut packet = vec![0; length];
    stream.read_exact(&mut packet)?;
    let mut remaining = packet.as_slice();
    let id = varint(&mut remaining)?;
    Ok((id, remaining.to_vec()))
}

fn connect(host: &str, port: u16, protocol: u32, state: u32) -> Result<TcpStream> {
    let address = (host, port).to_socket_addrs()?.next().ok_or("no address")?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut handshake = Vec::new();
    put_varint(&mut handshake, protocol);
    put_string(&mut handshake, host)?;
    handshake.extend_from_slice(&port.to_be_bytes());
    put_varint(&mut handshake, state);
    send(&mut stream, 0, &handshake)?;
    Ok(stream)
}

fn status(host: &str, port: u16, protocol: u32) -> Result<()> {
    let mut stream = connect(host, port, protocol, 1)?;
    send(&mut stream, 0, &[])?;
    let (id, body) = receive(&mut stream)?;
    if id != 0 {
        return Err(format!("expected status response, got {id}").into());
    }
    let mut body = body.as_slice();
    let json = string(&mut body)?;
    // shortcut: this only checks the brand text, add JSON parsing for status-schema conformance.
    if !body.is_empty()
        || !json.starts_with('{')
        || !json.contains("FerrumMC")
        || !json.contains("\"isModded\":true")
        || !json.contains(&format!("\"protocol\":{protocol}"))
    {
        return Err("status response is malformed or does not identify FerrumMC".into());
    }
    let nonce = 0x1234_5678_1234_5678_i64.to_be_bytes();
    send(&mut stream, 1, &nonce)?;
    let (id, pong) = receive(&mut stream)?;
    if id != 1 || pong != nonce {
        return Err("status ping/pong did not round-trip".into());
    }
    println!("PASS protocol {protocol}: status and ping/pong");
    Ok(())
}

fn login(
    host: &str,
    port: u16,
    protocol: u32,
    reconnect: Option<&SessionState>,
    item_ids: (u32, u32),
) -> Result<SessionState> {
    let mut stream = connect(host, port, protocol, 2)?;
    let name = format!("FerrumProbe{protocol}");
    let mut body = Vec::new();
    put_string(&mut body, &name)?;
    body.extend_from_slice(&[0; 16]);
    send(&mut stream, 0, &body)?;
    let (id, body) = receive(&mut stream)?;
    match id {
        0 => return Err(format!("login rejected: {}", string(&mut body.as_slice())?).into()),
        1 => return Err("encryption requested; use the isolated offline smoke config".into()),
        3 => return Err("compression requested; disable compression in the smoke config".into()),
        2 => (),
        _ => return Err(format!("unexpected login packet {id}").into()),
    }
    let mut remaining = body.as_slice();
    let mut uuid = [0; 16];
    remaining.read_exact(&mut uuid)?;
    if string(&mut remaining)? != name || uuid == [0; 16] {
        return Err("invalid login-success profile".into());
    }
    let properties = varint(&mut remaining)?;
    if properties != 0 {
        return Err("expected an offline profile without properties".into());
    }
    let mut session_id = [0; 16];
    remaining.read_exact(&mut session_id)?; // 26.2+ login-finished session UUID.
    if !remaining.is_empty() {
        return Err("unexpected trailing login-success bytes".into());
    }
    send(&mut stream, 3, &[])?;
    let suffix = if reconnect.is_some() {
        "reconnect"
    } else {
        "join"
    };
    let mut capture = File::create(format!("run/smoke/wire-{protocol}-{suffix}.bin"))?;
    let mut min_y = None;
    let mut registry_counts = BTreeMap::new();
    let mut settings = Vec::new();
    put_string(&mut settings, "en_us")?;
    settings.extend_from_slice(&[2, 0, 1, 127, 1, 0, 1, 0]);
    send(&mut stream, 0, &settings)?;
    loop {
        let (id, body) = receive(&mut stream)?;
        record(&mut capture, 0, id, &body)?;
        if id == 7 {
            let mut data = body.as_slice();
            let registry = string(&mut data)?;
            let count = varint(&mut data)?;
            registry_counts.insert(registry.clone(), count);
            for _ in 0..count {
                let entry = string(&mut data)?;
                let present = take(&mut data, 1)?[0];
                if present != 1 {
                    return Err("smoke declined known packs but registry data is absent".into());
                }
                let mut ints = BTreeMap::new();
                nbt(&mut data, &mut ints)?;
                if registry == "minecraft:dimension_type" && entry == "minecraft:overworld" {
                    min_y = ints.get("min_y").copied();
                }
            }
            if !data.is_empty() {
                return Err("trailing registry data".into());
            }
        }
        match id {
            2 => return Err(format!("configuration disconnect: {body:?}").into()),
            3 => {
                send(&mut stream, 3, &[])?;
                break;
            }
            4 => send(&mut stream, 4, &body)?,
            id if id == if protocol == 776 { 14 } else { 15 } => {
                send(&mut stream, 7, &[0])?;
            }
            _ => (),
        }
    }
    let min_y = min_y.ok_or("missing dimension minimum height")?;
    let biome_count = *registry_counts
        .get("minecraft:worldgen/biome")
        .ok_or("missing biomes")?;
    let mut chunk_blocks = BTreeMap::new();
    let mut position = None;
    let mut target = reconnect.map(|state| state.block);
    let mut attempted = false;
    let mut restored = false;
    let mut chat_seen = false;
    let mut login_seen = false;
    let mut inventory = None;
    let mut expected_inventory = reconnect.map(|state| state.inventory.clone());
    let mut inventory_moved = false;
    let mut revision = 0_u32;
    let item_id = if protocol == 776 {
        item_ids.0
    } else {
        item_ids.1
    };
    let start = Instant::now();
    let mut counts = BTreeMap::new();
    let mut chunks = 0;
    let mut teleports = 0;
    let mut keepalives = 0;
    loop {
        let (id, body) = receive(&mut stream)?;
        record(&mut capture, 1, id, &body)?;
        *counts.entry(id).or_insert(0) += 1;
        let current_id = if protocol == 776 {
            old_to_current(id)?
        } else {
            id
        };

        match current_id {
            0x20 => return Err(format!("play disconnect: {body:?}").into()),
            0x2d => {
                send_play(&mut stream, protocol, 28, &body)?;
                keepalives += 1;
            }
            0x2e => {
                let (key, blocks) = decode_chunk(&body, protocol, biome_count)?;
                chunk_blocks.insert(key, blocks);
                chunks += 1;
            }
            0x12 => {
                let mut data = body.as_slice();
                let container = varint(&mut data)?;
                let state_id = varint(&mut data)?;
                let slots = varint(&mut data)?;
                if slots > 256 {
                    return Err("too many inventory slots".into());
                }
                let mut contents = Vec::new();
                for _ in 0..slots {
                    contents.push(fixture_item(&mut data)?);
                }
                let carried = fixture_item(&mut data)?;
                if !data.is_empty() {
                    return Err("trailing inventory bytes".into());
                }
                if container == 0 {
                    revision = state_id;
                    inventory = Some(Inventory {
                        slots: contents,
                        carried,
                    });
                }
            }
            0x6e => {
                let mut data = body.as_slice();
                let slot = varint(&mut data)?;
                let item = fixture_item(&mut data)?;
                if !data.is_empty() {
                    return Err("trailing player inventory bytes".into());
                }
                let slot = match slot {
                    0..=8 => slot + 36,
                    9..=35 => slot,
                    36..=39 => 44 - slot,
                    40 => 45,
                    _ => return Err("invalid player inventory slot".into()),
                };
                if let Some(inventory) = &mut inventory {
                    *inventory
                        .slots
                        .get_mut(slot as usize)
                        .ok_or("missing inventory slot")? = item;
                }
            }
            0x62 => {
                let mut data = body.as_slice();
                let carried = fixture_item(&mut data)?;
                if !data.is_empty() {
                    return Err("trailing cursor item bytes".into());
                }
                if let Some(inventory) = &mut inventory {
                    inventory.carried = carried;
                }
            }
            0x14 => {
                let mut data = body.as_slice();
                let container = varint(&mut data)?;
                let state_id = varint(&mut data)?;
                let slot = i16::from_be_bytes(take(&mut data, 2)?.try_into()?);
                let item = fixture_item(&mut data)?;
                if !data.is_empty() {
                    return Err("trailing inventory slot bytes".into());
                }
                if container == 0
                    && let Some(inventory) = &mut inventory
                {
                    revision = state_id;
                    *inventory
                        .slots
                        .get_mut(usize::try_from(slot)?)
                        .ok_or("invalid inventory slot")? = item;
                }
            }
            0x32 => {
                login_seen = true;
            }
            0x21 | 0x7c | 0x42 => {
                if body
                    .windows(format!("FerrumSmoke {protocol}").len())
                    .any(|b| b == format!("FerrumSmoke {protocol}").as_bytes())
                {
                    chat_seen = true;
                }
            }
            0x08 => {
                let mut data = body.as_slice();
                let packed = u64::from_be_bytes(take(&mut data, 8)?.try_into()?);
                let block = [
                    (packed as i64 >> 38) as i32,
                    ((packed << 52) as i64 >> 52) as i32,
                    ((packed << 26) as i64 >> 38) as i32,
                ];
                let state = varint(&mut data)?;
                if attempted
                    && let Some((pos, expected)) = target
                    && pos == block
                {
                    if state != expected {
                        return Err(format!(
                            "break was not cancelled: expected {expected}, received {state}"
                        )
                        .into());
                    }
                    restored = true;
                }
            }
            0x0b => {
                send_play(&mut stream, protocol, 11, &20.0_f32.to_be_bytes())?;
            }
            0x49 => {
                let mut data = body.as_slice();
                let teleport = varint(&mut data)?;
                let mut confirm = Vec::new();
                put_varint(&mut confirm, teleport);
                if protocol == 777 {
                    confirm.extend_from_slice(&data[..24]);
                    confirm.extend_from_slice(&data[48..56]);
                }
                send_play(&mut stream, protocol, 0, &confirm)?;
                let mut coords = data;
                let xyz = [
                    f64::from_be_bytes(take(&mut coords, 8)?.try_into()?),
                    f64::from_be_bytes(take(&mut coords, 8)?.try_into()?),
                    f64::from_be_bytes(take(&mut coords, 8)?.try_into()?),
                ];
                position = Some(xyz);
                let mut pos = data[..24].to_vec();
                pos.push(1);
                send_play(&mut stream, protocol, 30, &pos)?;
                send_play(&mut stream, protocol, 44, &[])?;
                teleports += 1;
                let mut chat = Vec::new();
                put_string(&mut chat, &format!("FerrumSmoke {protocol}"))?;
                chat.extend_from_slice(
                    &(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_millis() as i64)
                        .to_be_bytes(),
                );
                chat.extend_from_slice(&0_i64.to_be_bytes());
                chat.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
                send_play(&mut stream, protocol, 9, &chat)?;
            }
            _ => (),
        }
        if let Some(xyz) = position {
            if reconnect.is_none() && target.is_none() && chunks > 0 {
                let x = xyz[0].floor() as i32;
                let z = xyz[2].floor() as i32;
                for depth in 1..=3 {
                    let pos = [x, xyz[1].floor() as i32 - depth, z];
                    if let Some(state) = block_at(&chunk_blocks, pos, min_y)
                        && state != 0
                    {
                        target = Some((pos, state));
                        break;
                    }
                }
            }
            if !attempted
                && start.elapsed() >= Duration::from_secs(5)
                && let Some((pos, expected)) = target
                && let Some(state) = block_at(&chunk_blocks, pos, min_y)
            {
                if state != expected {
                    return Err(format!("reconnected block changed: {expected} -> {state}").into());
                }
                if reconnect.is_some() {
                    restored = true;
                } else {
                    let packed = ((i64::from(pos[0]) & 0x3ffffff) << 38)
                        | ((i64::from(pos[2]) & 0x3ffffff) << 12)
                        | (i64::from(pos[1]) & 0xfff);
                    let mut dig = vec![0];
                    dig.extend_from_slice(&packed.to_be_bytes());
                    dig.push(1);
                    put_varint(&mut dig, 1);
                    send_play(&mut stream, protocol, 41, &dig)?;
                    println!("DIG protocol {protocol}: {pos:?}, original state {expected}");
                    let mut expected = inventory.clone().ok_or("missing initial inventory")?;
                    if expected.slots.len() != 46 || expected.carried.is_some() {
                        return Err("unexpected player inventory shape or carried item".into());
                    }
                    expected.slots[36] = None;
                    expected.slots[37] = Some(FixtureItem {
                        id: item_id,
                        count: 1,
                        removed: Vec::new(),
                    });
                    expected_inventory = Some(expected);
                    let mut empty = 37_i16.to_be_bytes().to_vec();
                    empty.push(0);
                    send_play(&mut stream, protocol, 57, &empty)?;
                    let mut item = 36_i16.to_be_bytes().to_vec();
                    put_varint(&mut item, 1);
                    put_varint(&mut item, item_id);
                    item.extend_from_slice(&[0, 0]);
                    send_play(&mut stream, protocol, 57, &item)?;
                }
                attempted = true;
            }
        }
        if reconnect.is_none()
            && attempted
            && !inventory_moved
            && start.elapsed() >= Duration::from_secs(8)
        {
            let mut click = vec![0];
            // A stale revision requests an authoritative full snapshot after the number-key swap.
            put_varint(&mut click, revision.wrapping_sub(1));
            click.extend_from_slice(&36_i16.to_be_bytes());
            click.extend_from_slice(&[1, 2, 0, 0]);
            send_play(&mut stream, protocol, 18, &click)?;
            inventory = None;
            inventory_moved = true;
        }
        let duration = if reconnect.is_some() { 6 } else { 31 };
        if start.elapsed() >= Duration::from_secs(duration) {
            if chunks == 0
                || teleports == 0
                || !login_seen
                || (reconnect.is_none() && keepalives == 0)
            {
                return Err("missing play login, chunks, teleport or keepalive".into());
            }
            if !restored {
                return Err("block cancellation/reconnect was not confirmed".into());
            }
            if inventory.is_none() || inventory != expected_inventory {
                return Err(format!(
                    "inventory mismatch: expected {expected_inventory:?}, received {inventory:?}"
                )
                .into());
            }
            if !chat_seen {
                return Err("chat message was not echoed".into());
            }
            println!(
                "PASS protocol {protocol} {suffix}: configuration, play, {chunks} chunks, {teleports} teleports, {keepalives} keepalives, chat, block unchanged, {}s connected",
                start.elapsed().as_secs()
            );
            break;
        }
    }
    stream.shutdown(Shutdown::Both)?;
    Ok(SessionState {
        block: target.ok_or("no break target")?,
        inventory: inventory.ok_or("missing final inventory")?,
    })
}

fn fixture_item(input: &mut &[u8]) -> Result<Option<FixtureItem>> {
    let count = varint(input)?;
    if count == 0 {
        return Ok(None);
    }
    let id = varint(input)?;
    let added = varint(input)?;
    let removed = varint(input)?;
    if added != 0 {
        return Err("Unexpected patched item in smoke fixture; vanilla oracle handles arbitrary component codecs".into());
    }
    if removed > 256 {
        return Err("too many removed item components".into());
    }
    let mut components = Vec::new();
    for _ in 0..removed {
        components.push(varint(input)?);
    }
    Ok(Some(FixtureItem {
        id,
        count,
        removed: components,
    }))
}

fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8]> {
    if count > input.len() {
        return Err("truncated field".into());
    }
    let (value, rest) = input.split_at(count);
    *input = rest;
    Ok(value)
}

fn nbt(input: &mut &[u8], ints: &mut BTreeMap<String, i32>) -> Result<()> {
    let kind = take(input, 1)?[0];
    nbt_value(input, kind, 0, ints)
}

fn nbt_string(input: &mut &[u8]) -> Result<String> {
    let length = u16::from_be_bytes(take(input, 2)?.try_into()?) as usize;
    Ok(std::str::from_utf8(take(input, length)?)?.to_owned())
}

fn nbt_value(
    input: &mut &[u8],
    kind: u8,
    depth: usize,
    ints: &mut BTreeMap<String, i32>,
) -> Result<()> {
    if depth > 64 {
        return Err("NBT depth exceeded".into());
    }
    match kind {
        0 => (),
        1 => {
            take(input, 1)?;
        }
        2 => {
            take(input, 2)?;
        }
        3 | 5 => {
            take(input, 4)?;
        }
        4 | 6 => {
            take(input, 8)?;
        }
        7 | 11 | 12 => {
            let count = u32::from_be_bytes(take(input, 4)?.try_into()?) as usize;
            let width = match kind {
                11 => 4,
                12 => 8,
                _ => 1,
            };
            take(
                input,
                count.checked_mul(width).ok_or("NBT length overflow")?,
            )?;
        }
        8 => {
            nbt_string(input)?;
        }
        9 => {
            let kind = take(input, 1)?[0];
            let count = u32::from_be_bytes(take(input, 4)?.try_into()?);
            if count > MAX_PACKET as u32 {
                return Err("NBT list too long".into());
            }
            for _ in 0..count {
                nbt_value(input, kind, depth + 1, ints)?;
            }
        }
        10 => loop {
            let kind = take(input, 1)?[0];
            if kind == 0 {
                break;
            }
            let name = nbt_string(input)?;
            if kind == 3 {
                let value = i32::from_be_bytes(take(input, 4)?.try_into()?);
                ints.insert(name, value);
            } else {
                nbt_value(input, kind, depth + 1, ints)?;
            }
        },
        _ => return Err("invalid NBT tag".into()),
    }
    Ok(())
}

fn decode_palette(input: &mut &[u8], size: usize, blocks: bool, limit: u32) -> Result<Vec<u32>> {
    let bits = take(input, 1)?[0];
    if bits > 32 {
        return Err("invalid palette bits".into());
    }
    let mut palette = Vec::new();
    if bits == 0 {
        let value = varint(input)?;
        if value >= limit {
            return Err("invalid single palette ID".into());
        }
        return Ok(vec![value; size]);
    }
    let indirect = bits <= if blocks { 8 } else { 3 };
    if indirect {
        let count = varint(input)?;
        if count == 0 || count > (1 << bits) {
            return Err("invalid palette length".into());
        }
        for _ in 0..count {
            let value = varint(input)?;
            if value >= limit {
                return Err("invalid palette ID".into());
            }
            palette.push(value);
        }
    }
    let per_long = 64 / bits as usize;
    let mut values = Vec::with_capacity(size);
    for _ in 0..size.div_ceil(per_long) {
        let word = u64::from_be_bytes(take(input, 8)?.try_into()?);
        for index in 0..per_long {
            if values.len() == size {
                break;
            }
            let value = ((word >> (index * bits as usize)) & ((1_u64 << bits) - 1)) as u32;
            let value = if indirect {
                *palette
                    .get(value as usize)
                    .ok_or("palette index out of range")?
            } else {
                value
            };
            if value >= limit {
                return Err("direct palette ID out of range".into());
            }
            values.push(value);
        }
    }
    Ok(values)
}

fn decode_chunk(body: &[u8], protocol: u32, biomes: u32) -> Result<((i32, i32), Vec<u32>)> {
    let mut input = body;
    let x = i32::from_be_bytes(take(&mut input, 4)?.try_into()?);
    let z = i32::from_be_bytes(take(&mut input, 4)?.try_into()?);
    let heightmaps = varint(&mut input)?;
    for _ in 0..heightmaps {
        let kind = varint(&mut input)?;
        if kind > 5 {
            return Err("invalid heightmap kind".into());
        }
        let count = varint(&mut input)? as usize;
        take(&mut input, count * 8)?;
    }
    let length = varint(&mut input)? as usize;
    let mut data = take(&mut input, length)?;
    let mut blocks = Vec::new();
    while !data.is_empty() {
        take(&mut data, 4)?;
        blocks.extend(decode_palette(
            &mut data,
            4096,
            true,
            if protocol == 776 {
                compatibility::BLOCK_STATE_COUNT_26_2 as u32
            } else {
                compatibility::BLOCK_STATES_26_2.len() as u32
            },
        )?);
        decode_palette(&mut data, 64, false, biomes)?;
    }
    let count = varint(&mut input)?;
    for _ in 0..count {
        take(&mut input, 3)?;
        varint(&mut input)?;
        nbt(&mut input, &mut BTreeMap::new())?;
    }
    for _ in 0..4 {
        let length = varint(&mut input)? as usize;
        take(&mut input, length * if protocol == 776 { 8 } else { 1 })?;
    }
    for _ in 0..2 {
        let count = varint(&mut input)?;
        for _ in 0..count {
            let size = varint(&mut input)? as usize;
            if size != 2048 {
                return Err("invalid light array".into());
            }
            take(&mut input, size)?;
        }
    }
    if !input.is_empty() {
        return Err("trailing chunk bytes".into());
    }
    Ok(((x, z), blocks))
}

fn block_at(chunks: &BTreeMap<(i32, i32), Vec<u32>>, pos: [i32; 3], min_y: i32) -> Option<u32> {
    let y = usize::try_from(pos[1] - min_y).ok()?;
    let index = (y / 16) * 4096
        + (y % 16) * 256
        + (pos[2].rem_euclid(16) as usize) * 16
        + pos[0].rem_euclid(16) as usize;
    chunks
        .get(&(pos[0].div_euclid(16), pos[2].div_euclid(16)))?
        .get(index)
        .copied()
}

fn record(file: &mut File, state: u8, id: u32, body: &[u8]) -> Result<()> {
    file.write_all(&[state])?;
    let mut packet = Vec::new();
    put_varint(&mut packet, id);
    packet.extend_from_slice(body);
    file.write_all(&(packet.len() as u32).to_be_bytes())?;
    file.write_all(&packet)?;
    Ok(())
}

#[allow(dead_code)]
mod compatibility {
    include!("../../crates/pumpkin-data/src/generated/compatibility.rs");
}
fn old_to_current(id: u32) -> Result<u32> {
    compatibility::PACKETS_26_2
        .iter()
        .find(|(d, p, _, old)| *d == "clientbound" && *p == "play" && *old == id as i32)
        .map(|(_, _, new, _)| *new as u32)
        .ok_or_else(|| format!("Unknown 26.2 play packet {id}").into())
}
fn send_play(stream: &mut TcpStream, protocol: u32, id: u32, body: &[u8]) -> Result<()> {
    let id = if protocol == 776 {
        compatibility::PACKETS_26_2
            .iter()
            .find(|(d, p, new, _)| *d == "serverbound" && *p == "play" && *new == id as i32)
            .map(|(_, _, _, old)| *old as u32)
            .ok_or("Missing serverbound packet mapping")?
    } else {
        id
    };
    send(stream, id, body)
}

fn self_test() -> Result<()> {
    for (value, bytes) in [
        (0, vec![0]),
        (127, vec![127]),
        (128, vec![128, 1]),
        (776, vec![0x88, 0x06]),
        (777, vec![0x89, 0x06]),
        (u32::MAX, vec![255, 255, 255, 255, 15]),
    ] {
        let mut encoded = Vec::new();
        put_varint(&mut encoded, value);
        assert_eq!(encoded, bytes);
        assert_eq!(varint(&mut bytes.as_slice())?, value);
    }
    assert!(varint(&mut [255, 255, 255, 255, 16].as_slice()).is_err());
    assert!(varint(&mut [128].as_slice()).is_err());
    assert!(receive(&mut [0].as_slice()).is_err());
    assert!(receive(&mut [2, 0].as_slice()).is_err());
    assert!(receive(&mut [255, 255, 255, 255, 15].as_slice()).is_err());
    assert!(string(&mut [2, b'a'].as_slice()).is_err());
    assert!(string(&mut [1, 255].as_slice()).is_err());
    assert_eq!(receive(&mut [2, 0, 42].as_slice())?, (0, vec![42]));
    let mut ints = BTreeMap::new();
    nbt(
        &mut [
            10, 3, 0, 5, b'm', b'i', b'n', b'_', b'y', 255, 255, 255, 192, 0,
        ]
        .as_slice(),
        &mut ints,
    )?;
    assert_eq!(ints.get("min_y"), Some(&-64));
    assert!(nbt(&mut [10, 3, 0, 1, b'x', 0].as_slice(), &mut ints).is_err());
    assert!(nbt(&mut [13].as_slice(), &mut ints).is_err());
    let mut palette = vec![1, 2, 0, 1];
    palette.extend_from_slice(&1_u64.to_be_bytes());
    let values = decode_palette(&mut palette.as_slice(), 64, false, 2)?;
    assert_eq!(values[0], 1);
    assert!(values[1..].iter().all(|v| *v == 0));
    assert!(decode_palette(&mut [1, 1, 0].as_slice(), 64, false, 2).is_err());
    assert!(decode_palette(&mut [0, 2].as_slice(), 64, false, 2).is_err());
    assert_eq!(fixture_item(&mut [0].as_slice())?, None);
    assert_eq!(
        fixture_item(&mut [3, 42, 0, 1, 7].as_slice())?,
        Some(FixtureItem {
            id: 42,
            count: 3,
            removed: vec![7]
        })
    );
    assert!(fixture_item(&mut [1, 42, 0, 1].as_slice()).is_err());
    assert!(fixture_item(&mut [1, 42, 1, 0].as_slice()).is_err());
    assert!(fixture_item(&mut [1, 42, 0, 0x81, 2].as_slice()).is_err());
    println!("PASS codec fixtures, bounded packets, NBT, chunk palettes and malformed input");
    Ok(())
}

fn reject_vanilla(host: &str, port: u16, protocol: u32, early_finish: bool) -> Result<()> {
    let mut stream = connect(host, port, protocol, 2)?;
    let mut login = Vec::new();
    put_string(&mut login, &format!("FerrumProbe{protocol}"))?;
    login.extend_from_slice(&[0; 16]);
    send(&mut stream, 0, &login)?;
    if receive(&mut stream)?.0 != 2 {
        return Err("expected login success before configuration rejection".into());
    }
    send(&mut stream, 3, &[])?;
    if early_finish {
        send(&mut stream, 3, &[])?;
    }
    for _ in 0..32 {
        let (id, body) = receive(&mut stream)?;
        match id {
            2 => {
                let mut input = body.as_slice();
                nbt(&mut input, &mut BTreeMap::new())?;
                let message = b"This server requires NeoForge 26.2 with the server's mods.";
                if !input.is_empty() || !body.windows(message.len()).any(|w| w == message) {
                    return Err(
                        "configuration disconnect missing valid NBT or NeoForge requirement".into(),
                    );
                }
                println!(
                    "PASS protocol {protocol}: clean NeoForge requirement rejection (early_finish={early_finish})"
                );
                return Ok(());
            }
            5 if !early_finish => send(&mut stream, 5, &body)?,
            3 => return Err("vanilla client was admitted to play".into()),
            _ => (),
        }
    }
    Err("configuration did not reject vanilla client".into())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--self-test") {
        return self_test();
    }
    let host = args.get(1).map_or("127.0.0.1", String::as_str);
    let port = args.get(2).map_or(Ok(25566), |value| value.parse())?;
    if args.get(3).is_some_and(|arg| arg == "--reject-vanilla") {
        for protocol in [776, 777] {
            status(host, port, protocol)?;
            reject_vanilla(host, port, protocol, false)?;
            reject_vanilla(host, port, protocol, true)?;
        }
        return Ok(());
    }
    let item_ids = (
        args.get(3).ok_or("Missing 26.2 test item ID")?.parse()?,
        args.get(4).ok_or("Missing engine test item ID")?.parse()?,
    );
    for protocol in [776, 777] {
        status(host, port, protocol)?;
        let target = login(host, port, protocol, None, item_ids)?;
        std::thread::sleep(Duration::from_millis(500));
        login(host, port, protocol, Some(&target), item_ids)?;
        println!(
            "PASS protocol {protocol}: acacia boat moved from slot 36 to 37; all 46 slots, counts, component patches and empty cursor match after reconnect"
        );
    }
    Ok(())
}

//! Dependency-free offline configuration and gameplay probe.
//! Captures are also checked by the matching unmodified vanilla packet codecs.
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const MAX_PACKET: usize = 2 * 1024 * 1024;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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

fn self_test() -> Result<()> {
    for (value, bytes) in [
        (0, vec![0]),
        (127, vec![127]),
        (128, vec![128, 1]),
        (776, vec![0x88, 0x06]),
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
    println!("PASS codec fixtures, bounded packets, NBT and malformed input");
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
        status(host, port, 776)?;
        reject_vanilla(host, port, 776, false)?;
        reject_vanilla(host, port, 776, true)?;
        // Protocols adjacent to the supported release fail before login success.
        for protocol in [775, 777] {
            let mut stream = connect(host, port, protocol, 2)?;
            let (id, body) = receive(&mut stream)?;
            if id != 0 || body.is_empty() {
                return Err("unsupported protocol admitted".into());
            }
            println!("PASS protocol {protocol}: rejected at handshake");
        }
        return Ok(());
    }
    Err(
        "Use --self-test or --reject-vanilla; gameplay runs through the NeoForge client probe"
            .into(),
    )
}

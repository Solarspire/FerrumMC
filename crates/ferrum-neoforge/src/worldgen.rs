//! Batched generation calls on the current worker; no live-world locks or callbacks.
use std::sync::OnceLock;

use jni::{
    JavaVM,
    objects::{JByteArray, JShortArray},
};
use pumpkin_data::biome::Biome;

use crate::Error;

static STEPS: OnceLock<u32> = OnceLock::new();

pub(crate) fn install(vm: &JavaVM) -> Result<(), Error> {
    let states: Vec<i32> = (0..=u16::MAX)
        .map(|id| crate::world::java_state_raw(id).unwrap_or(-1))
        .collect();
    let biomes: Vec<_> = (0..=u8::MAX)
        .map(|id| Biome::from_id(id).map(|b| b.registry_id))
        .collect();
    let steps = vm.attach_current_thread(|env| -> Result<u32, Error> {
        let names = env.new_string(serde_json::to_string(&biomes)?)?;
        let array = env.new_int_array(states.len())?;
        array.set_region(env, 0, &states)?;
        let result = env.call_static_method(
            jni::jni_str!("net/solarspire/ferrum/runtime/WorldgenRuntime"),
            jni::jni_str!("initialize"),
            jni::jni_sig!("(Ljava/lang/String;[I)I"),
            &[(&names).into(), (&array).into()],
        );
        Ok(crate::checked(env, result)?.i()? as u32)
    })?;
    STEPS
        .set(steps)
        .map_err(|_| Error::Runtime("generation bridge already installed".into()))
}

pub fn has_step(step: usize) -> bool {
    step < u32::BITS as usize && STEPS.get().is_some_and(|steps| steps & (1 << step) != 0)
}

/// Block writes and saved block entities of one generation call; the caller validates before committing.
#[derive(Default)]
pub struct Generated {
    /// Snapshot index and Java-side Rust state id pairs.
    pub writes: Vec<(i32, i32)>,
    /// Block position and its binary NBT, as written by `NbtIo.write`.
    pub block_entities: Vec<([i32; 3], Vec<u8>)>,
}

pub fn decorate(
    seed: u64,
    dimension: &str,
    metadata: &[i32],
    blocks: &[i16],
    biomes: &[u8],
) -> Result<Generated, Error> {
    JavaVM::singleton()?.attach_current_thread(|env| -> Result<Generated, Error> {
        let dimension = env.new_string(dimension)?;
        let meta = env.new_int_array(metadata.len())?;
        meta.set_region(env, 0, metadata)?;
        let states: JShortArray = env.new_short_array(blocks.len())?;
        states.set_region(env, 0, blocks)?;
        let biome_ids = env.byte_array_from_slice(biomes)?;
        let result = env.call_static_method(
            jni::jni_str!("net/solarspire/ferrum/runtime/WorldgenRuntime"),
            jni::jni_str!("decorate"),
            jni::jni_sig!("(JLjava/lang/String;[I[S[B)[B"),
            &[
                (seed as i64).into(),
                (&dimension).into(),
                (&meta).into(),
                (&states).into(),
                (&biome_ids).into(),
            ],
        );
        let object = crate::checked(env, result)?.l()?;
        let bytes = env.convert_byte_array(env.cast_local::<JByteArray>(object)?)?;
        parse(&bytes, blocks.len())
    })
}

fn parse(bytes: &[u8], lease_len: usize) -> Result<Generated, Error> {
    let mut reader = Reader(bytes);
    let writes = reader.count(lease_len)?;
    let mut generated = Generated {
        writes: (0..writes)
            .map(|_| Ok((reader.int()?, reader.int()?)))
            .collect::<Result<_, Error>>()?,
        ..Generated::default()
    };
    let entities = reader.count(lease_len)?;
    for _ in 0..entities {
        let position = [reader.int()?, reader.int()?, reader.int()?];
        let length = reader.count(MAX_BLOCK_ENTITY_BYTES)?;
        generated
            .block_entities
            .push((position, reader.take(length)?.to_vec()));
    }
    Ok(generated)
}

/// Upper bound for one block entity's saved data from untrusted Java output.
const MAX_BLOCK_ENTITY_BYTES: usize = 1 << 20;

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let (head, tail) = self
            .0
            .split_at_checked(length)
            .ok_or_else(|| Error::Runtime("truncated generation result".into()))?;
        self.0 = tail;
        Ok(head)
    }

    fn int(&mut self) -> Result<i32, Error> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn count(&mut self, limit: usize) -> Result<usize, Error> {
        usize::try_from(self.int()?)
            .ok()
            .filter(|count| *count <= limit)
            .ok_or_else(|| Error::Runtime("invalid generation result length".into()))
    }
}

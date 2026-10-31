//! Transfers exclusively leased generation terrain, never the live world, to Java.
use std::io::Cursor;

use ferrum_neoforge::worldgen;
use pumpkin_nbt::{Nbt, deserializer::NbtReadHelperJava};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::generation::modded::{Decorator, Output, Snapshot};

pub fn install() -> Result<(), String> {
    if !ferrum_neoforge::world::enabled() {
        return Ok(());
    }
    pumpkin_world::generation::modded::install(Box::new(JavaDecorator))
}

struct JavaDecorator;

impl Decorator for JavaDecorator {
    fn has_step(&self, step: usize) -> bool {
        worldgen::has_step(step)
    }

    fn decorate(&self, seed: u64, snapshot: &Snapshot) -> Result<Output, String> {
        let generated = worldgen::decorate(
            seed,
            snapshot.dimension,
            &snapshot.metadata,
            &snapshot.blocks,
            &snapshot.biomes,
        )
        .map_err(|e| e.to_string())?;
        // Validate the whole batch before any canonical mutation.
        let blocks = generated
            .writes
            .iter()
            .map(|&(index, state)| {
                let index =
                    usize::try_from(index).map_err(|_| "negative generation write index")?;
                if index >= snapshot.blocks.len() {
                    return Err("generation write outside lease");
                }
                let state = u16::try_from(state).map_err(|_| "invalid generation state")?;
                let state = ferrum_neoforge::world::java_state_raw(state)
                    .and_then(|java| ferrum_neoforge::world::rust_state(java as u32))
                    .ok_or("unknown generation state")?;
                Ok((index, state))
            })
            .collect::<Result<_, &str>>()?;
        let block_entities = generated
            .block_entities
            .iter()
            .map(|(position, bytes)| {
                let nbt = Nbt::read(&mut NbtReadHelperJava::new(Cursor::new(bytes.as_slice())))
                    .map_err(|e| format!("invalid generated block entity: {e}"))?;
                Ok((
                    Vector3::new(position[0], position[1], position[2]),
                    nbt.root_tag,
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(Output {
            blocks,
            block_entities,
        })
    }
}

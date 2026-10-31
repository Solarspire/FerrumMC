//! Synchronous decoration extensions over the exclusively leased generation cache.
//!
//! The 3x3 chunk snapshot is built here so it compiles at the world crate's optimisation level.
use std::cell::RefCell;
use std::sync::OnceLock;

use pumpkin_data::BlockStateId;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{HeightMap, math::vector3::Vector3};

use super::proto_chunk::GenerationCache;

/// Side length of the leased chunk neighbourhood.
const LEASE_WIDTH: i32 = 3;
const CHUNK_AREA: usize = 256;
const BIOME_COLUMNS: i32 = 4;
const HEIGHTMAPS: [HeightMap; 2] = [HeightMap::WorldSurfaceWg, HeightMap::OceanFloorWg];

/// Terrain copied out of the generation cache for one decoration step. Slots are chunk columns in
/// x-major order, each holding `height` blocks in engine section order.
pub struct Snapshot {
    pub dimension: &'static str,
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub step: usize,
    /// Chunk coordinates, vertical extents, step and both generation heightmaps of every slot.
    pub metadata: Vec<i32>,
    /// Engine block state ids reinterpreted as raw 16-bit values, ready for a Java `short[]`.
    pub blocks: Vec<i16>,
    pub biomes: Vec<u8>,
    volume: usize,
    bottom: i32,
}

/// Blocks and block entities produced by one generation call, to be committed by Rust.
#[derive(Default)]
pub struct Output {
    /// Snapshot indices with the block state to place there.
    pub blocks: Vec<(usize, BlockStateId)>,
    pub block_entities: Vec<(Vector3<i32>, NbtCompound)>,
}

pub trait Decorator: Send + Sync {
    fn has_step(&self, step: usize) -> bool;
    fn decorate(&self, seed: u64, snapshot: &Snapshot) -> Result<Output, String>;
}

static DECORATOR: OnceLock<Box<dyn Decorator>> = OnceLock::new();

thread_local! {
    static BLOCKS: RefCell<Vec<i16>> = const { RefCell::new(Vec::new()) };
}

pub fn install(decorator: Box<dyn Decorator>) -> Result<(), String> {
    DECORATOR
        .set(decorator)
        .map_err(|_| "mod decoration already installed".into())
}

pub(crate) fn decorate(cache: &mut dyn GenerationCache, seed: u64, step: usize) {
    let Some(decorator) = DECORATOR.get() else {
        return;
    };
    if !decorator.has_step(step) {
        return;
    }
    let snapshot = Snapshot::capture(cache, step);
    let result = decorator.decorate(seed, &snapshot);
    let (bottom, volume, chunk_x, chunk_z) = (
        snapshot.bottom,
        snapshot.volume,
        snapshot.chunk_x,
        snapshot.chunk_z,
    );
    BLOCKS.with(|buffer| *buffer.borrow_mut() = snapshot.blocks);
    // Java's output is all or nothing, so a failed step leaves the chunk without mod features rather than half done.
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            tracing::error!("Mod decoration of chunk {chunk_x} {chunk_z} failed: {error}");
            return;
        }
    };
    for (index, state) in output.blocks {
        let slot = index / volume;
        let position = snapshot_position(
            chunk_x + (slot / LEASE_WIDTH as usize) as i32 - 1,
            chunk_z + (slot % LEASE_WIDTH as usize) as i32 - 1,
            bottom,
            index % volume,
        );
        cache.set_block_state(&position, state.to_state());
    }
    for (position, nbt) in output.block_entities {
        cache.add_block_entity(&position, nbt);
    }
}

impl Snapshot {
    fn capture(cache: &dyn GenerationCache, step: usize) -> Self {
        let center = cache.get_center_chunk();
        let (chunk_x, chunk_z) = (center.x, center.z);
        let bottom = i32::from(cache.bottom_y());
        let height = usize::from(cache.height());
        let volume = height * CHUNK_AREA;
        let slots = (LEASE_WIDTH * LEASE_WIDTH) as usize;
        let mut blocks = BLOCKS.with(|buffer| std::mem::take(&mut *buffer.borrow_mut()));
        blocks.clear();
        blocks.resize(slots * volume, BlockStateId::AIR.as_u16() as i16);
        let mut biomes = Vec::with_capacity(slots * height * BIOME_COLUMNS as usize);
        let mut metadata = Vec::with_capacity(8 + slots * HEIGHTMAPS.len() * CHUNK_AREA);
        metadata.extend([
            chunk_x,
            chunk_z,
            bottom,
            height as i32,
            cache.get_sea_level(),
            i32::from(center.generation_bottom_y()),
            i32::from(center.generation_height()),
            step as i32,
        ]);
        for dx in -1..=1 {
            for dz in -1..=1 {
                let (x, z) = (chunk_x + dx, chunk_z + dz);
                let slot = ((dx + 1) * LEASE_WIDTH + dz + 1) as usize;
                snapshot_blocks(cache, x, z, bottom, &mut blocks[slot * volume..][..volume]);
                for lx in 0..BIOME_COLUMNS {
                    for ly in 0..height as i32 / 4 {
                        for lz in 0..BIOME_COLUMNS {
                            biomes.push(cache.get_noise_biome_id(
                                x * 4 + lx,
                                (bottom >> 2) + ly,
                                z * 4 + lz,
                            ));
                        }
                    }
                }
                for map in &HEIGHTMAPS {
                    for lx in 0..16 {
                        for lz in 0..16 {
                            metadata.push(cache.get_top_y(map, x * 16 + lx, z * 16 + lz));
                        }
                    }
                }
            }
        }
        Self {
            dimension: center.generation_dimension,
            chunk_x,
            chunk_z,
            step,
            metadata,
            blocks,
            biomes,
            volume,
            bottom,
        }
    }
}

fn snapshot_blocks(cache: &dyn GenerationCache, x: i32, z: i32, bottom: i32, blocks: &mut [i16]) {
    if let Some(chunk) = cache.get_chunk(x, z) {
        for (target, state) in blocks.iter_mut().zip(chunk.generation_block_states()) {
            *target = state.as_u16() as i16;
        }
    } else {
        for (index, target) in blocks.iter_mut().enumerate() {
            let position = snapshot_position(x, z, bottom, index);
            *target = GenerationCache::get_block_state(cache, &position).as_u16() as i16;
        }
    }
}

const fn snapshot_position(x: i32, z: i32, bottom: i32, index: usize) -> Vector3<i32> {
    Vector3::new(
        x * 16 + ((index % 4096) / 256) as i32,
        bottom + (index / 4096 * 16 + (index % 256) / 16) as i32,
        z * 16 + (index % 16) as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::snapshot_position;

    #[test]
    fn generation_snapshot_uses_engine_section_order_at_negative_coordinates() {
        assert_eq!(
            snapshot_position(-2, -1, -64, 0),
            super::Vector3::new(-32, -64, -16)
        );
        assert_eq!(
            snapshot_position(-2, -1, -64, 4095),
            super::Vector3::new(-17, -49, -1)
        );
        assert_eq!(
            snapshot_position(-2, -1, -64, 4096),
            super::Vector3::new(-32, -48, -16)
        );
    }
}

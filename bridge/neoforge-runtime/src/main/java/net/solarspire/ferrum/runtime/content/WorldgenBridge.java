package net.solarspire.ferrum.runtime.content;

import com.google.gson.Gson;
import it.unimi.dsi.fastutil.ints.Int2IntOpenHashMap;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.EntityBlock;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.neoforged.neoforge.registries.NeoForgeRegistries;

/** Per-call terrain views; Rust alone commits blocks, heightmaps, lighting and saves. */
public final class WorldgenBridge {
    private record Plan(ChunkGenerator generator, List<FeatureSorter.StepFeatureData> steps, Set<PlacedFeature> ores, List<List<Structure>> structures) {}
    /** Logs every placed structure and generated block entity, for the worldgen acceptance script. */
    private static final boolean TRACE = System.getenv("FERRUM_WORLDGEN_TRACE") != null;
    private static final org.slf4j.Logger LOGGER = org.slf4j.LoggerFactory.getLogger(WorldgenBridge.class);
    private static final Set<PlacedFeature> FAILED = Collections.newSetFromMap(new java.util.concurrent.ConcurrentHashMap<>());
    private static final Map<String,ServerLevelStandIn> STAND_INS = new java.util.concurrent.ConcurrentHashMap<>();
    private static final Map<String,Plan> PLANS = new HashMap<>();
    private static final Map<String,StructureWorld> STRUCTURE_WORLDS = new java.util.concurrent.ConcurrentHashMap<>();
    private static final Set<Structure> FAILED_STRUCTURES = Collections.newSetFromMap(new java.util.concurrent.ConcurrentHashMap<>());
    /** Structure starts per world are cheap to recompute from the seed; the cache only bounds repeated work. */
    private static final int MAX_CACHED_START_CHUNKS = 1 << 16;
    /** Vanilla's createReferences radius: farthest chunk whose start can reach a chunk. */
    private static final int STRUCTURE_REFERENCE_RADIUS = 8;
    private static BlockState[] states;
    private static int[] rustStates;
    private static List<Holder<Biome>> biomes;
    private static PalettedContainerFactory containers;

    public static void applyBiomeModifiers() {
        var access = BridgeContent.worldRegistries;
        var modifiers = access.lookupOrThrow(NeoForgeRegistries.Keys.BIOME_MODIFIERS).stream().toList();
        access.lookupOrThrow(Registries.BIOME).listElements().forEach(b ->
            b.value().modifiableBiomeInfo().applyBiomeModifiers(b, modifiers, access));
        access.lookupOrThrow(Registries.LEVEL_STEM).forEach(stem -> stem.generator().refreshFeaturesPerStep());
    }

    public static int initialize(String biomeNames, int[] javaStates) {
        states = new BlockState[javaStates.length];
        rustStates = new int[Block.BLOCK_STATE_REGISTRY.size()];
        Arrays.fill(rustStates, -1);
        for (int i = 0; i < javaStates.length; i++) if (javaStates[i] >= 0) {
            states[i] = Block.stateById(javaStates[i]);
            rustStates[javaStates[i]] = i;
        }
        var registry = BridgeContent.worldRegistries.lookupOrThrow(Registries.BIOME);
        var plains = registry.get(Identifier.withDefaultNamespace("plains")).orElseThrow();
        biomes = Arrays.stream(new Gson().fromJson(biomeNames, String[].class))
            .map(name -> name == null ? plains : registry.get(Identifier.parse(name)).orElse(plains))
            .map(holder -> (Holder<Biome>)holder).toList();
        containers = PalettedContainerFactory.create(BridgeContent.worldRegistries);
        int steps = 0;
        // WorldLoader supplies datapack dimension overrides, not the default preset's stems.
        var dimensions = new LinkedHashMap<>(net.minecraft.world.level.levelgen.presets.WorldPresets
            .createNormalWorldDimensions(BridgeContent.worldRegistries).dimensions());
        for (var entry : BridgeContent.worldRegistries.lookupOrThrow(Registries.LEVEL_STEM).entrySet())
            dimensions.put(entry.getKey(),entry.getValue());
        for (var entry : dimensions.entrySet()) {
            var generator = entry.getValue().generator();
            var sorted = FeatureSorter.buildFeaturesPerStep(List.copyOf(generator.getBiomeSource().possibleBiomes()),
                b -> generator.getBiomeGenerationSettings(b).features(), true);
            Set<PlacedFeature> ores = Collections.newSetFromMap(new IdentityHashMap<>());
            var placed = BridgeContent.worldRegistries.lookupOrThrow(Registries.PLACED_FEATURE);
            for (int step = 0; step < sorted.size(); step++) for (var feature : sorted.get(step).features()) {
                var key = placed.getKey(feature);
                // Real generators and placement modifiers run for every mod feature; structures are separate.
                if (key != null && !key.getNamespace().equals("minecraft")) {
                    ores.add(feature);
                    steps |= 1 << step;
                }
            }
            var structures = new ArrayList<List<Structure>>();
            for (int i = 0; i < GenerationStep.Decoration.values().length; i++) structures.add(new ArrayList<>());
            // Start generation needs a noise generator's RandomState; other generator types get no mod structures.
            if (generator instanceof NoiseBasedChunkGenerator)
            BridgeContent.worldRegistries.lookupOrThrow(Registries.STRUCTURE).listElements()
                .filter(h -> !h.key().identifier().getNamespace().equals("minecraft"))
                .sorted(Comparator.comparing(h -> h.key().identifier().toString()))
                .forEach(h -> structures.get(h.value().step().ordinal()).add(h.value()));
            for (int step = 0; step < structures.size(); step++) if (!structures.get(step).isEmpty()) steps |= 1 << step;
            PLANS.put(entry.getKey().identifier().toString(), new Plan(generator, sorted, ores, structures));
        }
        org.slf4j.LoggerFactory.getLogger(WorldgenBridge.class).info("Rust generation bridge ready: {} ore features, {} structures, step mask {}",
            PLANS.values().stream().mapToInt(p -> p.ores.size()).sum(),
            PLANS.values().stream().mapToInt(p -> p.structures.stream().mapToInt(List::size).sum()).sum(), steps);
        return steps;
    }

    private static ServerLevelStandIn standIn(String dimension, long seed) {
        return STAND_INS.computeIfAbsent(dimension + '@' + seed, key -> {
            try { return ServerLevelStandIn.create(Identifier.parse(dimension), seed); }
            catch (Exception error) { throw new IllegalStateException(error); }
        });
    }

    public static byte[] decorate(long seed, String dimension, int[] metadata, short[] blocks, byte[] biomeIds) {
        var plan = PLANS.get(dimension);
        if (plan == null) throw new IllegalArgumentException("No generation plan for " + dimension);
        var region = new Region(seed, metadata, blocks, biomeIds, standIn(dimension, seed));
        WorldGenLevel level = (WorldGenLevel) Proxy.newProxyInstance(WorldgenBridge.class.getClassLoader(),
            new Class<?>[] {WorldGenLevel.class}, region);
        region.level = level;
        var random = new WorldgenRandom(new XoroshiroRandomSource(0));
        long decorationSeed = random.setDecorationSeed(seed, metadata[0] << 4, metadata[1] << 4);
        int step = metadata[7];
        if (step < plan.structures.size() && !plan.structures.get(step).isEmpty())
            placeStructures(plan, region, dimension, seed, random, decorationSeed, step);
        if (step >= plan.steps.size()) return region.pack();
        var features = plan.steps.get(step);
        var selected = new TreeSet<Integer>();
        Set<Holder<Biome>> possible = new HashSet<>();
        for (byte id : biomeIds) possible.add(biomes.get(Byte.toUnsignedInt(id)));
        possible.retainAll(plan.generator.getBiomeSource().possibleBiomes());
        for (var biome : possible) {
            var list = plan.generator.getBiomeGenerationSettings(biome).features();
            if (step < list.size()) for (var feature : list.get(step)) if (plan.ores.contains(feature.value()))
                selected.add(features.indexMapping().applyAsInt(feature.value()));
        }
        for (int index : selected) {
            random.setFeatureSeed(decorationSeed, index, step);
            var feature = features.features().get(index);
            region.undo.clear();
            region.blockEntityUndo.clear();
            try {
                feature.placeWithBiomeCheck(level, plan.generator, random,
                    new BlockPos(metadata[0] << 4, metadata[2], metadata[1] << 4));
            } catch (RuntimeException | LinkageError error) {
                region.rollback();
                if (FAILED.add(feature)) LOGGER.error("Generation feature {} failed; skipped", BridgeContent.worldRegistries
                    .lookupOrThrow(Registries.PLACED_FEATURE).getKey(feature), error);
            }
        }
        return region.pack();
    }

    /** Mirrors the structure half of ChunkGenerator.applyBiomeDecoration for the mod structures of one step. */
    private static void placeStructures(Plan plan, Region region, String dimension, long seed, WorldgenRandom random, long decorationSeed, int step) {
        var world = STRUCTURE_WORLDS.computeIfAbsent(dimension + '@' + seed, key -> new StructureWorld(dimension, seed, plan, standIn(dimension, seed)));
        var heights = LevelHeightAccessor.create(region.metadata[2], region.metadata[3]);
        int chunkX = region.metadata[0], chunkZ = region.metadata[1];
        var chunkPos = new ChunkPos(chunkX, chunkZ);
        var writable = new BoundingBox(chunkPos.getMinBlockX(), region.metadata[2] + 1, chunkPos.getMinBlockZ(),
            chunkPos.getMinBlockX() + 15, region.metadata[2] + region.metadata[3] - 1, chunkPos.getMinBlockZ() + 15);
        var structures = plan.structures.get(step);
        for (int index = 0; index < structures.size(); index++) {
            var structure = structures.get(index);
            random.setFeatureSeed(decorationSeed, index, step);
            region.undo.clear();
            region.blockEntityUndo.clear();
            try {
                for (var start : world.startsReaching(structure, chunkX, chunkZ, heights)) {
                    int before = region.writes.size();
                    start.placeInChunk(region.level, world.manager, plan.generator, random, writable, chunkPos);
                    if (TRACE) LOGGER.info("Placed structure {} in chunk {},{}: {} block writes", BridgeContent.worldRegistries
                        .lookupOrThrow(Registries.STRUCTURE).getKey(structure), chunkX, chunkZ, region.writes.size() - before);
                }
            } catch (RuntimeException | LinkageError error) {
                region.rollback();
                if (FAILED_STRUCTURES.add(structure)) LOGGER.error("Generation structure {} failed; skipped", BridgeContent.worldRegistries
                    .lookupOrThrow(Registries.STRUCTURE).getKey(structure), error);
            }
        }
    }

    /** Deterministic mod structure starts of one dimension and seed, generated by the real Java generator on demand. */
    private static final class StructureWorld {
        final ChunkGenerator generator;
        final net.minecraft.world.level.chunk.ChunkGeneratorStructureState state;
        final StructureManager manager = new StructureManager(null, null, null);
        final ServerLevelStandIn level;
        final net.minecraft.resources.ResourceKey<Level> dimension;
        final Map<Long,Map<Structure,StructureStart>> starts = new java.util.concurrent.ConcurrentHashMap<>();
        StructureWorld(String name, long seed, Plan plan, ServerLevelStandIn level) {
            this.generator = plan.generator; this.level = level;
            this.dimension = net.minecraft.resources.ResourceKey.create(Registries.DIMENSION, Identifier.parse(name));
            var noise = (NoiseBasedChunkGenerator) generator;
            var randomState = RandomState.create(noise.generatorSettings().value(), BridgeContent.worldRegistries.lookupOrThrow(Registries.NOISE), seed);
            // Vanilla structure sets are generated and placed by Rust.
            var sets = BridgeContent.worldRegistries.lookupOrThrow(Registries.STRUCTURE_SET).filterElements(set -> set.structures().stream()
                .allMatch(entry -> entry.structure().unwrapKey().map(key -> !key.identifier().getNamespace().equals("minecraft")).orElse(false)));
            this.state = generator.createState(sets, randomState, seed);
        }
        Map<Structure,StructureStart> startsAt(int x, int z, LevelHeightAccessor heights) {
            long key = ChunkPos.pack(x, z);
            var cached = starts.get(key);
            if (cached != null) return cached;
            if (starts.size() > MAX_CACHED_START_CHUNKS) starts.clear();
            var chunk = new ProtoChunk(new ChunkPos(x, z), UpgradeData.EMPTY, heights, containers, null);
            generator.createStructures(BridgeContent.worldRegistries, state, manager, chunk, level.getStructureManager(), dimension);
            var valid = new LinkedHashMap<Structure,StructureStart>();
            chunk.getAllStarts().forEach((structure, start) -> {
                if (!start.isValid()) return;
                valid.put(structure, start);
                if (TRACE) LOGGER.info("Structure start {} in {} at chunk {},{} ({} pieces, {})", BridgeContent.worldRegistries.lookupOrThrow(Registries.STRUCTURE).getKey(structure),
                    dimension.identifier(), x, z, start.getPieces().size(), start.getBoundingBox());
            });
            var previous = starts.putIfAbsent(key, valid);
            return previous != null ? previous : valid;
        }
        /** Starts whose bounding box reaches the chunk, in vanilla's reference scan order. */
        List<StructureStart> startsReaching(Structure structure, int chunkX, int chunkZ, LevelHeightAccessor heights) {
            var result = new ArrayList<StructureStart>();
            int minX = chunkX << 4, minZ = chunkZ << 4;
            for (int dx = -STRUCTURE_REFERENCE_RADIUS; dx <= STRUCTURE_REFERENCE_RADIUS; dx++)
                for (int dz = -STRUCTURE_REFERENCE_RADIUS; dz <= STRUCTURE_REFERENCE_RADIUS; dz++) {
                    var start = startsAt(chunkX + dx, chunkZ + dz, heights).get(structure);
                    if (start != null && start.getBoundingBox().intersects(minX, minZ, minX + 15, minZ + 15)) result.add(start);
                }
            return result;
        }
    }

    private static final class Region implements InvocationHandler {
        private static final net.minecraft.world.level.border.WorldBorder WORLD_BORDER = new net.minecraft.world.level.border.WorldBorder();
        final long seed;
        final int[] metadata;
        final short[] blocks;
        final byte[] biomeIds;
        final ProtoChunk[] chunks = new ProtoChunk[9];
        final Int2IntOpenHashMap writes = new Int2IntOpenHashMap();
        // (index, previous block value, previous write or -1) triples of the feature in flight.
        final it.unimi.dsi.fastutil.ints.IntArrayList undo = new it.unimi.dsi.fastutil.ints.IntArrayList();
        // Block entities of the call; Rust receives their saved data and owns the live instances.
        final Map<Long,BlockEntity> blockEntities = new LinkedHashMap<>();
        // (position key, previous block entity or null) pairs of the feature in flight.
        final List<Object[]> blockEntityUndo = new ArrayList<>();
        final ServerLevelStandIn server;
        final RandomSource random;
        WorldGenLevel level;
        Region(long seed, int[] metadata, short[] blocks, byte[] biomeIds, ServerLevelStandIn server) {
            this.server = server; this.seed = seed;
            this.random = new XoroshiroRandomSource(seed ^ (metadata[0] * 341873128712L + metadata[1] * 132897987541L)); this.metadata = metadata; this.blocks = blocks; this.biomeIds = biomeIds;
            if (metadata.length != 8 + 9 * 256 * 2 || metadata[3] <= 0 || metadata[3] > 4096
                || blocks.length != 9 * metadata[3] * 256 || biomeIds.length != 9 * metadata[3] * 4)
                throw new IllegalArgumentException("Invalid generation snapshot");
        }
        int chunkIndex(int x, int z) {
            int dx = x - metadata[0] + 1, dz = z - metadata[1] + 1;
            return dx < 0 || dx > 2 || dz < 0 || dz > 2 ? -1 : dx * 3 + dz;
        }
        int index(int x, int y, int z) {
            int chunk = chunkIndex(x >> 4, z >> 4), localY = y - metadata[2];
            if (chunk < 0 || localY < 0 || localY >= metadata[3]) return -1;
            return chunk * metadata[3] * 256 + (localY >> 4) * 4096 + (x & 15) * 256 + (localY & 15) * 16 + (z & 15);
        }
        BlockState state(int x, int y, int z) {
            int index = index(x,y,z);
            if (index < 0) return Blocks.VOID_AIR.defaultBlockState();
            return stateAt(index);
        }
        BlockState stateAt(int index) {
            var state = states[blocks[index] & 0xFFFF];
            if (state == null) return Blocks.AIR.defaultBlockState();
            return state;
        }
        boolean set(int x, int y, int z, BlockState state) {
            int index = index(x,y,z);
            if (index < 0) return false;
            int rust = rustStates[Block.getId(state)];
            if (rust < 0) throw new IllegalStateException("Unmapped Java block state " + state);
            if ((blocks[index] & 0xFFFF) == rust) return false;
            undo.add(index); undo.add(blocks[index] & 0xFFFF); undo.add(writes.getOrDefault(index,-1));
            blocks[index] = (short) rust; writes.put(index,rust);
            syncBlockEntity(x,y,z,state);
            int chunk = index / (metadata[3] * 256);
            if (chunks[chunk] != null) ((Section)chunks[chunk].getSection((y - metadata[2]) >> 4)).palette = null;
            return true;
        }
        /** Mirrors WorldGenRegion.setBlock: a block with an entity gets a fresh one, any other state drops it. */
        void syncBlockEntity(int x, int y, int z, BlockState state) {
            long key = BlockPos.asLong(x,y,z);
            var previous = blockEntities.get(key);
            BlockEntity next = state.hasBlockEntity() && state.getBlock() instanceof EntityBlock block
                ? block.newBlockEntity(new BlockPos(x,y,z),state) : null;
            if (previous == null && next == null) return;
            blockEntityUndo.add(new Object[] {key, previous});
            if (next == null) blockEntities.remove(key); else blockEntities.put(key,next);
        }
        BlockEntity blockEntity(BlockPos pos) {
            if (index(pos.getX(),pos.getY(),pos.getZ()) < 0) return null;
            var entity = blockEntities.get(pos.asLong());
            if (entity == null) {
                syncBlockEntity(pos.getX(),pos.getY(),pos.getZ(),state(pos.getX(),pos.getY(),pos.getZ()));
                entity = blockEntities.get(pos.asLong());
            }
            return entity;
        }
        void putBlockEntity(BlockEntity entity) {
            var pos = entity.getBlockPos();
            if (index(pos.getX(),pos.getY(),pos.getZ()) < 0) return;
            blockEntityUndo.add(new Object[] {pos.asLong(), blockEntities.put(pos.asLong(),entity)});
        }
        void removeBlockEntity(BlockPos pos) {
            if (blockEntities.containsKey(pos.asLong())) blockEntityUndo.add(new Object[] {pos.asLong(), blockEntities.remove(pos.asLong())});
        }
        /** Layout: write count, (index, Rust state) pairs, entity count, (x, y, z, length, NBT) entries. */
        byte[] pack() {
            var buffer = new java.io.ByteArrayOutputStream();
            var out = new java.io.DataOutputStream(buffer);
            try {
                out.writeInt(writes.size());
                for (var entry : writes.int2IntEntrySet()) { out.writeInt(entry.getIntKey()); out.writeInt(entry.getIntValue()); }
                var saved = new ArrayList<byte[]>();
                var positions = new ArrayList<BlockPos>();
                for (var entry : blockEntities.entrySet()) {
                    var pos = BlockPos.of(entry.getKey());
                    try {
                        var bytes = new java.io.ByteArrayOutputStream();
                        net.minecraft.nbt.NbtIo.write(entry.getValue().saveWithFullMetadata(BridgeContent.worldRegistries), new java.io.DataOutputStream(bytes));
                        saved.add(bytes.toByteArray());
                        if (TRACE) LOGGER.info("Generated block entity {} at {} {} {}", BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(entry.getValue().getType()), pos.getX(), pos.getY(), pos.getZ());
                        positions.add(pos);
                    } catch (RuntimeException | LinkageError error) {
                        LOGGER.error("Generated block entity at {} failed to save; skipped", pos, error);
                    }
                }
                out.writeInt(saved.size());
                for (int i = 0; i < saved.size(); i++) {
                    out.writeInt(positions.get(i).getX()); out.writeInt(positions.get(i).getY()); out.writeInt(positions.get(i).getZ());
                    out.writeInt(saved.get(i).length); out.write(saved.get(i));
                }
            } catch (java.io.IOException error) { throw new java.io.UncheckedIOException(error); }
            return buffer.toByteArray();
        }
        void rollback() {
            for (int i = blockEntityUndo.size() - 1; i >= 0; i--) {
                var entry = blockEntityUndo.get(i);
                if (entry[1] == null) blockEntities.remove((long) entry[0]); else blockEntities.put((long) entry[0], (BlockEntity) entry[1]);
            }
            blockEntityUndo.clear();
            for (int i = undo.size() - 3; i >= 0; i -= 3) {
                int index = undo.getInt(i);
                blocks[index] = (short) undo.getInt(i + 1);
                if (undo.getInt(i + 2) < 0) writes.remove(index); else writes.put(index, undo.getInt(i + 2));
                int chunk = index / (metadata[3] * 256);
                if (chunks[chunk] != null) ((Section)chunks[chunk].getSection((index % (metadata[3] * 256)) / 4096)).palette = null;
            }
            undo.clear();
        }
        Holder<Biome> biome(int x, int y, int z) {
            int chunk = chunkIndex(x >> 2,z >> 2);
            if (chunk < 0) throw new IllegalArgumentException("Biome read outside generation lease");
            int height = metadata[3] >> 2;
            int localY = Math.clamp(y - (metadata[2] >> 2),0,height - 1);
            return biomes.get(Byte.toUnsignedInt(biomeIds[chunk * height * 16 + (x & 3) * height * 4 + localY * 4 + (z & 3)]));
        }
        ProtoChunk chunk(int x, int z) {
            int index = chunkIndex(x,z);
            if (index < 0) return null;
            if (chunks[index] == null) {
                var region = this;
                var chunk = new ProtoChunk(new ChunkPos(x,z),UpgradeData.EMPTY,level,containers,null) {
                    @Override public BlockEntity getBlockEntity(BlockPos pos) { return region.blockEntity(pos); }
                    @Override public void setBlockEntity(BlockEntity entity) { region.putBlockEntity(entity); }
                    @Override public void removeBlockEntity(BlockPos pos) { region.removeBlockEntity(pos); }
                };
                var sections = chunk.getSections();
                for (int i = 0; i < sections.length; i++) sections[i] = new Section(this,x,z,i);
                chunks[index] = chunk;
            }
            return chunks[index];
        }
        @Override public Object invoke(Object proxy, java.lang.reflect.Method method, Object[] args) throws Throwable {
            switch (method.getName()) {
                case "getSeed": return seed;
                case "getRandom": return random;
                case "getMinY": return metadata[2];
                case "getHeight": {
                    if (args == null || args.length == 0) return metadata[3];
                    int x = (int)args[1], z = (int)args[2], index = chunkIndex(x >> 4,z >> 4);
                    if (index < 0) throw new IllegalArgumentException("Height read outside generation lease");
                    var type = (Heightmap.Types)args[0];
                    int map = type == Heightmap.Types.OCEAN_FLOOR_WG || type == Heightmap.Types.OCEAN_FLOOR ? 1 : 0;
                    return metadata[8 + (index * 2 + map) * 256 + (x & 15) * 16 + (z & 15)];
                }
                case "getSeaLevel": return metadata[4];
                case "getLevel": return server;
                case "registryAccess": return BridgeContent.worldRegistries;
                case "getBlockState": { var p = (BlockPos)args[0]; return state(p.getX(),p.getY(),p.getZ()); }
                case "getFluidState": { var p = (BlockPos)args[0]; return state(p.getX(),p.getY(),p.getZ()).getFluidState(); }
                case "setBlock": { var p = (BlockPos)args[0]; return set(p.getX(),p.getY(),p.getZ(),(BlockState)args[1]); }
                case "getBlockEntity": return blockEntity((BlockPos)args[0]);
                case "setBlockEntity": putBlockEntity((BlockEntity)args[0]); return null;
                case "removeBlockEntity": removeBlockEntity((BlockPos)args[0]); return null;
                case "ensureCanWrite": { var p = (BlockPos)args[0]; return index(p.getX(),p.getY(),p.getZ()) >= 0; }
                case "getChunk": if (args[0] instanceof Integer) return chunk((int)args[0],(int)args[1]); break;
                case "hasChunk": return chunkIndex((int)args[0],(int)args[1]) >= 0;
                case "getNoiseBiome": case "getUncachedNoiseBiome": return biome((int)args[0],(int)args[1],(int)args[2]);
                case "getBiomeManager": return new BiomeManager((x,y,z) -> biome(x,y,z),BiomeManager.obfuscateSeed(seed));
                case "isClientSide": return false;
                case "enabledFeatures": return net.minecraft.world.level.WorldDataConfiguration.DEFAULT.enabledFeatures();
                case "dimensionType": return server.dimensionType();
                case "getServer": return server.getServer();
                case "getCurrentDifficultyAt": return new net.minecraft.world.DifficultyInstance(net.minecraft.world.Difficulty.NORMAL,0L,0L,0.0f);
                case "getDifficulty": return net.minecraft.world.Difficulty.NORMAL;
                case "getWorldBorder": return WORLD_BORDER;
                case "getBlockTicks": case "getFluidTicks": return net.minecraft.world.ticks.BlackholeTickAccess.emptyLevelList();
                case "nextSubTickCount": return 0L;
                case "getSkyDarken": return 0;
                case "getShade": return 1.0f;
                case "getBrightness": case "getRawBrightness": case "getMaxLocalRawBrightness": return 15;
                case "hashCode": return System.identityHashCode(proxy);
                case "equals": return proxy == args[0];
                case "setCurrentlyGenerating": return null;
                case "blockUpdated": case "scheduleTick": case "updateNeighborsAt": case "neighborShapeChanged":
                case "gameEvent": case "levelEvent": case "playSound": case "addParticle": return method.getReturnType() == boolean.class ? Boolean.FALSE : null;
                case "removeBlock": case "destroyBlock": {
                    var p = (BlockPos)args[0];
                    return set(p.getX(),p.getY(),p.getZ(),state(p.getX(),p.getY(),p.getZ()).getFluidState().createLegacyBlock());
                }
                case "addFreshEntity": case "addFreshEntityWithPassengers": return Boolean.FALSE;
                case "toString": return "Rust generation lease";
            }
            if (method.isDefault()) return InvocationHandler.invokeDefault(proxy,method,args);
            throw new UnsupportedOperationException("Generation view: " + method);
        }
    }

    /** Identity-only ServerLevel for mod generators; blocks and entities are reachable only through the generation view. */
    private static final class ServerLevelStandIn extends ServerLevel {
        private static final sun.misc.Unsafe UNSAFE;
        static {
            try {
                var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
                field.setAccessible(true);
                UNSAFE = (sun.misc.Unsafe) field.get(null);
            } catch (ReflectiveOperationException error) { throw new ExceptionInInitializerError(error); }
        }
        private long seed;
        private net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager templates;
        private ServerLevelStandIn() { super(null,null,null,null,null,null,false,0,List.of(),false); }

        static ServerLevelStandIn create(Identifier dimension, long seed) throws Exception {
            var level = (ServerLevelStandIn) UNSAFE.allocateInstance(ServerLevelStandIn.class);
            level.seed = seed;
            put(level, Level.class, "dimension", net.minecraft.resources.ResourceKey.create(Registries.DIMENSION, dimension));
            put(level, Level.class, "registryAccess", BridgeContent.worldRegistries);
            put(level, Level.class, "damageSources", new net.minecraft.world.damagesource.DamageSources(BridgeContent.worldRegistries));
            var typeKey = dimension.toString().equals("minecraft:the_nether") ? net.minecraft.world.level.dimension.BuiltinDimensionTypes.NETHER
                : dimension.toString().equals("minecraft:the_end") ? net.minecraft.world.level.dimension.BuiltinDimensionTypes.END : net.minecraft.world.level.dimension.BuiltinDimensionTypes.OVERWORLD;
            put(level, Level.class, "dimensionTypeRegistration", BridgeContent.worldRegistries.lookupOrThrow(Registries.DIMENSION_TYPE).getOrThrow(typeKey));
            if (net.neoforged.fml.ModList.get().isLoaded("hbm")) {
                // Match MixinServerLevel's constructor injection, which allocateInstance skips.
                var fields = Class.forName("com.hbm.world.NtmWorldgenFields", true, level.getClass().getClassLoader())
                    .getConstructor(long.class, Identifier.class).newInstance(seed, dimension);
                put(level, ServerLevel.class, "hbm$worldgenFields", fields);
            }
            level.templates = templateManager();
            // Template placement asks level.getServer().getStructureManager().
            var server = RustLevel.standInServer();
            var field = net.minecraft.server.MinecraftServer.class.getDeclaredField("structureTemplateManager");
            if (UNSAFE.getObject(server, UNSAFE.objectFieldOffset(field)) == null) UNSAFE.putObject(server, UNSAFE.objectFieldOffset(field), level.templates);
            put(level, ServerLevel.class, "server", server);
            return level;
        }
        /** Reads structure NBT straight from the loaded mod resources; the vanilla constructor needs a world save folder. */
        private static net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager templateManager() throws ReflectiveOperationException {
            var type = net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager.class;
            var manager = UNSAFE.allocateInstance(type);
            var lister = type.getDeclaredField("RESOURCE_STRUCTURE_LISTER");
            lister.setAccessible(true);
            var source = new net.minecraft.world.level.levelgen.structure.templatesystem.loader.ResourceManagerTemplateSource(
                net.minecraft.util.datafix.DataFixers.getDataFixer(), BridgeContent.worldRegistries.lookupOrThrow(Registries.BLOCK),
                BridgeContent.resources, (net.minecraft.resources.FileToIdConverter) lister.get(null));
            put(manager, type, "structureRepository", new java.util.concurrent.ConcurrentHashMap<>());
            put(manager, type, "resourceManagerSource", source);
            put(manager, type, "sources", List.of(source));
            return (net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager) manager;
        }
        private static void put(Object target, Class<?> owner, String name, Object value) throws ReflectiveOperationException {
            UNSAFE.putObject(target, UNSAFE.objectFieldOffset(owner.getDeclaredField(name)), value);
        }
        @Override public long getSeed() { return seed; }
        @Override public net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager getStructureManager() { return templates; }
    }

    private static final class Section extends LevelChunkSection {
        final Region region;
        final int x, z, y, base;
        Set<BlockState> palette;
        Section(Region region, int x, int z, int section) {
            super(containers); this.region = region; this.x = x << 4; this.z = z << 4; this.y = region.metadata[2] + section * 16;
            this.base = region.index(this.x,this.y,this.z);
        }
        @Override public BlockState getBlockState(int x,int y,int z) { return region.stateAt(base+x*256+y*16+z); }
        @Override public net.minecraft.world.level.material.FluidState getFluidState(int x,int y,int z) { return getBlockState(x,y,z).getFluidState(); }
        @Override public BlockState setBlockState(int x,int y,int z,BlockState state,boolean lock) {
            var old = getBlockState(x,y,z); region.set(this.x+x,this.y+y,this.z+z,state); return old;
        }
        @Override public BlockState setBlockState(int x,int y,int z,BlockState state) { return setBlockState(x,y,z,state,false); }
        @Override public boolean hasOnlyAir() { return !maybeHas(s -> !s.isAir()); }
        @Override public boolean maybeHas(Predicate<BlockState> predicate) {
            if (palette == null) {
                palette = Collections.newSetFromMap(new IdentityHashMap<>());
                int previous = -1;
                for (int index=base;index<base+4096;index++) {
                    int state = region.blocks[index] & 0xFFFF;
                    if (state != previous) { palette.add(region.stateAt(index)); previous = state; }
                }
            }
            for (var state : palette) if (predicate.test(state)) return true;
            return false;
        }
        @Override public Holder<Biome> getNoiseBiome(int x,int y,int z) { return region.biome((this.x>>2)+x,(this.y>>2)+y,(this.z>>2)+z); }
        @Override public void acquire() {}
        @Override public void release() {}
    }
}

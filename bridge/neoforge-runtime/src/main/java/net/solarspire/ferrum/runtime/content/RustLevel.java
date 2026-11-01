package net.solarspire.ferrum.runtime.content;

import com.google.gson.*;
import java.io.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerChunkCache;
import net.minecraft.util.ProblemReporter;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.EntityBlock;
import net.minecraft.world.level.block.entity.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.storage.TagValueInput;
import net.neoforged.neoforge.capabilities.BlockCapability;
import net.solarspire.ferrum.runtime.BridgeRuntime;

/** A leased view. No ServerLevel constructor, chunk storage, scheduler or save loop runs. */
public final class RustLevel extends ServerLevel {
    private static final org.slf4j.Logger LOGGER = org.slf4j.LoggerFactory.getLogger(RustLevel.class);
    private static final Gson JSON = new Gson();
    private static final sun.misc.Unsafe UNSAFE;
    private static final Map<String,RustLevel> LEVELS = new java.util.concurrent.ConcurrentHashMap<>();
    private static final RegistryAccess REGISTRIES = BridgeContent.worldRegistries;
    private Map<BlockPos,Entry> entities;
    /** Block entities whose setChanged fired since the last batch reply. */
    private Set<BlockPos> changed;
    private int sweepCountdown;
    private int minimumY;
    private int levelHeight;
    private RandomSource rustRandom;
    private RustChunkSource chunks;
    private RustLightEngine light;
    private String worldId;
    /** Rust's handle for this world on typed calls, and values Rust pushes with each request instead of per call. */
    private int handle;
    private long seed, gameTime, dayTime;
    private int seaLevel;
    /** Rust's world folder for mod saved data, {@code <world>/data/neoforge}. */
    private java.nio.file.Path dataFolder;
    private net.minecraft.core.Holder<?>[] biomes;
    private String dimensionName;
    private EntityBridge.Getter entityGetter;
    private Map<ChunkPos,RustChunk> loadedViews;

    static {
        try {
            var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
            field.setAccessible(true);
            UNSAFE = (sun.misc.Unsafe) field.get(null);
        } catch (ReflectiveOperationException error) { throw new ExceptionInInitializerError(error); }
    }

    private RustLevel() { super(null,null,null,null,null,null,false,0,List.of(),false); }

    private static RustLevel create() throws Exception {
        var level = (RustLevel) UNSAFE.allocateInstance(RustLevel.class);
        level.entities = new LinkedHashMap<>();
        level.changed = new LinkedHashSet<>();
        level.biomes = new net.minecraft.core.Holder<?>[256];
        level.sweepCountdown = SAVE_INTERVAL_BATCHES;
        level.loadedViews = new LinkedHashMap<>();
        level.chunks = (RustChunkSource) UNSAFE.allocateInstance(RustChunkSource.class);
        level.chunks.owner = level;
        level.light = (RustLightEngine) UNSAFE.allocateInstance(RustLightEngine.class);
        level.light.owner = level;
        level.entityGetter = new EntityBridge.Getter(level);
        UNSAFE.putObject(level.chunks,UNSAFE.objectFieldOffset(ServerChunkCache.class.getDeclaredField("chunkMap")),UNSAFE.allocateInstance(RustChunkMap.class));
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(ServerLevel.class.getDeclaredField("waypointManager")),new net.minecraft.server.waypoints.ServerWaypointManager());
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(ServerLevel.class.getDeclaredField("dragonParts")),new it.unimi.dsi.fastutil.ints.Int2ObjectOpenHashMap<>());
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(ServerLevel.class.getDeclaredField("navigatingMobs")),new java.util.HashSet<>());
        put(level,Level.class,"neighborUpdater",level.new RustNeighborUpdater());
        // ServerLevel.explode and others read the field directly; it must show the players Rust has in this world.
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(ServerLevel.class.getDeclaredField("players")),new java.util.AbstractList<net.minecraft.server.level.ServerPlayer>() {
            @Override public net.minecraft.server.level.ServerPlayer get(int index) { return PlayerBridge.playersIn(level).get(index); }
            @Override public int size() { return PlayerBridge.playersIn(level).size(); }
        });
        level.rustRandom = new LegacyRandomSource(0) {
            @Override public int next(int bits) { return BridgeRuntime.random() >>> (32-bits); }
        };
        var random = Level.class.getDeclaredField("random");
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(random),level.rustRandom);
        for (var field : Level.class.getDeclaredFields()) {
            // threadSafeRandom and the sound seed generator are constructor-made randoms; Rust is the only randomness source.
            if (field.getType() == RandomSource.class && !java.lang.reflect.Modifier.isStatic(field.getModifiers())) {
                UNSAFE.putObject(level,UNSAFE.objectFieldOffset(field),level.rustRandom);
            }
        }
        initConstructorState(level);
        // Biomes come from Rust chunk sections through the "biome" world op.
        put(level,Level.class,"biomeManager",new net.minecraft.world.level.biome.BiomeManager(level::getUncachedNoiseBiome,0L));
        var capabilities = ServerLevel.class.getDeclaredField("capListenerHolder");
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(capabilities),new net.neoforged.neoforge.capabilities.CapabilityListenerHolder());
        var serverField = ServerLevel.class.getDeclaredField("server");
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(serverField),standInServer());
        if (net.neoforged.fml.ModList.get().isLoaded("hbm")) {
            // Match MixinServerLevel's field initializers without running a Java world constructor.
            var graphs = Class.forName("com.hbm.uninos.graph.LevelNodeGraph").getMethod("emptySlots").invoke(null);
            var field = ServerLevel.class.getDeclaredField("hbm$graphs");
            UNSAFE.putObject(level,UNSAFE.objectFieldOffset(field),graphs);
            for (String name : List.of("hbm$radarTargets","hbm$livingTargets","hbm$storageEntities")) {
                field = ServerLevel.class.getDeclaredField(name);
                UNSAFE.putObject(level,UNSAFE.objectFieldOffset(field),new it.unimi.dsi.fastutil.objects.ObjectLinkedOpenHashSet<>());
            }
        }
        return level;
    }


    private static void put(Object target,Class<?> owner,String name,Object value) throws ReflectiveOperationException {
        UNSAFE.putObject(target,UNSAFE.objectFieldOffset(owner.getDeclaredField(name)),value);
    }

    /** Level and ServerLevel constructors build these; allocateInstance skips them, so mod and vanilla code would hit null. */
    private static void initConstructorState(RustLevel level) throws Exception {
        for (String name : List.of("capturedBlockSnapshots","freshBlockEntities","pendingFreshBlockEntities")) put(level,Level.class,name,new ArrayList<>());
        for (String name : List.of("blockEntityTickers","pendingBlockEntityTickers")) put(level,Level.class,name,new ArrayList<>());
        put(level,Level.class,"damageSources",new net.minecraft.world.damagesource.DamageSources(REGISTRIES));
        var data = levelData(level);
        put(level,Level.class,"levelData",data);
        put(level,ServerLevel.class,"serverLevelData",data);
        put(level,ServerLevel.class,"blockEvents",new it.unimi.dsi.fastutil.objects.ObjectLinkedOpenHashSet<>());
        put(level,ServerLevel.class,"blockEventsToReschedule",new ArrayList<>());
        put(level,ServerLevel.class,"customSpawners",List.of());
        put(level,ServerLevel.class,"seedOverride",java.util.OptionalLong.empty());
    }

    /** Level data Rust owns: game time comes from Rust, everything else reads as a fresh default world. */
    private static net.minecraft.world.level.storage.ServerLevelData levelData(RustLevel level) {
        return (net.minecraft.world.level.storage.ServerLevelData) java.lang.reflect.Proxy.newProxyInstance(
            RustLevel.class.getClassLoader(),new Class<?>[] { net.minecraft.world.level.storage.ServerLevelData.class },
            (proxy,method,args) -> {
                if (method.isDefault()) return java.lang.reflect.InvocationHandler.invokeDefault(proxy,method,args);
                Class<?> type = method.getReturnType();
                return switch (method.getName()) {
                    case "getGameTime" -> level.getGameTime();
                    case "getDifficulty" -> net.minecraft.world.Difficulty.NORMAL;
                    case "getGameType" -> net.minecraft.world.level.GameType.SURVIVAL;
                    case "getLevelName" -> level.dimensionName;
                    case "getRespawnData" -> net.minecraft.world.level.storage.LevelData.RespawnData.DEFAULT;
                    case "isInitialized" -> true;
                    case "getDayTimePerTick" -> 1.0f;
                    default -> type == boolean.class ? (Object) false : type == int.class ? (Object) 0 : type == long.class ? (Object) 0L
                        : type == float.class ? (Object) 0.0f : type == double.class ? (Object) 0.0 : null;
                };
            });
    }

    private static final java.util.Set<String> FAILED_TICKS = java.util.concurrent.ConcurrentHashMap.newKeySet();
    private static final int SAVE_INTERVAL_BATCHES = 1200;
    private static net.minecraft.world.level.levelgen.WorldDimensions defaultDimensions;
    private static synchronized net.minecraft.world.level.levelgen.WorldDimensions defaultDimensions() {
        if (defaultDimensions == null) defaultDimensions = net.minecraft.world.level.levelgen.presets.WorldPresets.createNormalWorldDimensions(REGISTRIES);
        return defaultDimensions;
    }
    private static net.minecraft.server.MinecraftServer standIn;

    /** Constructor-less server so mod code that asks its level for a server finds mixin-added state instead of null. */
    static synchronized net.minecraft.server.MinecraftServer standInServer() throws Exception {
        if (standIn == null) {
            var server = (net.minecraft.server.MinecraftServer) UNSAFE.allocateInstance(RustServer.class);
            for (Class<?> type = server.getClass(); type != Object.class; type = type.getSuperclass()) {
                for (var field : type.getDeclaredFields()) {
                    // Mixin-injected fields are named mod$field; their initializers did not run without a constructor.
                    if (!field.getName().contains("$") || java.lang.reflect.Modifier.isStatic(field.getModifiers()) || field.getType().isPrimitive()) continue;
                    try {
                        var constructor = field.getType().getDeclaredConstructor();
                        constructor.setAccessible(true);
                        UNSAFE.putObject(server,UNSAFE.objectFieldOffset(field),constructor.newInstance());
                    } catch (ReflectiveOperationException | RuntimeException ignored) { }
                }
            }
            put(server,MinecraftServer.class,"playerList",PlayerBridge.playerList());
            put(server,MinecraftServer.class,"registries",BridgeContent.layeredRegistries);
            put(server,MinecraftServer.class,"resources",BridgeContent.serverResources);
            put(server,MinecraftServer.class,"levels",new java.util.concurrent.ConcurrentHashMap<>());
            // Entity.checkInsideBlocks asks it for block debug listeners; nobody subscribes here.
            put(server,MinecraftServer.class,"debugSubscribers",new net.minecraft.util.debug.ServerDebugSubscribers(server));
            // Entity and mob ticks read these through their level (teams, tick rate, weather, brewing, fuel).
            try {
            put(server,MinecraftServer.class,"scoreboard",new net.minecraft.server.ServerScoreboard(server));
            put(server,MinecraftServer.class,"tickRateManager",new net.minecraft.server.ServerTickRateManager(server));
            put(server,MinecraftServer.class,"weatherData",new net.minecraft.world.level.saveddata.WeatherData());
            put(server,MinecraftServer.class,"random",RandomSource.create());
            put(server,MinecraftServer.class,"customBossEvents",new net.minecraft.server.bossevents.CustomBossEvents());
            put(server,MinecraftServer.class,"suppressedExceptions",new net.minecraft.server.SuppressedExceptionCollector());
            put(server,MinecraftServer.class,"potionBrewing",net.minecraft.world.item.alchemy.PotionBrewing.bootstrap(net.minecraft.world.level.WorldDataConfiguration.DEFAULT.enabledFeatures()));
            put(server,MinecraftServer.class,"fuelValues",BridgeContent.fuels);
            } catch (RuntimeException error) { LOGGER.error("Cannot fill the bridge server's gameplay state", error); }
            standIn = server;
            // NeoForge's server lifecycle: the hook records the current server, mods see it on the start events.
            try {
                var current = net.neoforged.neoforge.server.ServerLifecycleHooks.class.getDeclaredField("currentServer");
                current.setAccessible(true);
                current.set(null,server);
            } catch (ReflectiveOperationException error) { LOGGER.error("Cannot register the bridge server with NeoForge",error); }
            postLifecycle(new net.neoforged.neoforge.event.server.ServerAboutToStartEvent(server));
            postLifecycle(new net.neoforged.neoforge.event.server.ServerStartingEvent(server));
        }
        return standIn;
    }

    private static void postLifecycle(net.neoforged.bus.api.Event event) {
        SafeEvents.post(event);
    }

    private static boolean serverStarted;

    /** Rust has no blocking world load: the server counts as started once the overworld exists, as in vanilla's loadLevel. */
    private static void levelLoaded(RustLevel level) {
        postLifecycle(new net.neoforged.neoforge.event.level.LevelEvent.Load(level));
        if (!serverStarted && level.dimension().equals(Level.OVERWORLD)) {
            serverStarted = true;
            postLifecycle(new net.neoforged.neoforge.event.server.ServerStartedEvent(standIn));
        }
    }

    /** Stopping events, then saved data and the registered server go away. */
    static synchronized void stopServer() {
        if (standIn == null) return;
        postLifecycle(new net.neoforged.neoforge.event.server.ServerStoppingEvent(standIn));
        for (var level : LEVELS.values()) {
            postLifecycle(new net.neoforged.neoforge.event.level.LevelEvent.Unload(level));
            level.chunks.saveData();
        }
        postLifecycle(new net.neoforged.neoforge.event.server.ServerStoppedEvent(standIn));
        try {
            var current = net.neoforged.neoforge.server.ServerLifecycleHooks.class.getDeclaredField("currentServer");
            current.setAccessible(true);
            current.set(null,null);
        } catch (ReflectiveOperationException error) { LOGGER.warn("Cannot clear the bridge server",error); }
        standIn = null;
        serverStarted = false;
    }

    /** The server mods see. Rust runs the tick loop, so Java-side scheduling happens inline on the tick thread. */
    private static final class RustServer extends net.minecraft.server.dedicated.DedicatedServer {
        private RustServer() { super(null,null,null,null,Optional.empty(),null,null,null,null,null); }
        @Override public boolean isSameThread() { return true; }
        @Override public void execute(Runnable task) { task.run(); }
        @Override public void executeIfPossible(Runnable task) { task.run(); }
        @Override public void schedule(net.minecraft.server.TickTask task) { task.run(); }
        @Override public boolean isRunning() { return true; }
        @Override public boolean isStopped() { return false; }
        @Override public boolean isDedicatedServer() { return true; }
        @Override public int getTickCount() { var level = LEVELS.values().stream().findFirst(); return level.isPresent() ? (int) level.get().getGameTime() : 0; }
        @Override public Iterable<ServerLevel> getAllLevels() { return new ArrayList<ServerLevel>(LEVELS.values()); }
        // Loot contexts ask for a named random sequence; Rust is the only randomness source.
        @Override public RandomSource getRandomSequence(net.minecraft.resources.Identifier id) { return LEVELS.values().iterator().next().rustRandom; }
        @Override public int getPlayerCount() { return PlayerBridge.all().size(); }
        @Override public net.minecraft.world.level.storage.SavedDataStorage getDataStorage() { return overworld().getDataStorage(); }
        @Override public boolean isHardcore() { return false; }
        // Rust applies spawn protection before any Java break or placement runs.
        @Override public boolean isUnderSpawnProtection(ServerLevel level, BlockPos pos, net.minecraft.world.entity.player.Player player) { return false; }
        @Override public net.minecraft.world.level.GameType getDefaultGameType() { return net.minecraft.world.level.GameType.SURVIVAL; }
        @Override public net.minecraft.world.level.gamerules.GameRules getGameRules() { return GAME_RULES; }
        // Weather and time belong to Rust's world.
        @Override public void setWeatherParameters(int clear,int rain,boolean raining,boolean thundering) { }
    }

    private static final net.minecraft.world.level.gamerules.GameRules GAME_RULES =
        new net.minecraft.world.level.gamerules.GameRules(net.minecraft.world.level.WorldDataConfiguration.DEFAULT.enabledFeatures());

    static JsonElement call(JsonObject request) { return JsonParser.parseString(BridgeRuntime.world(request.toString())); }

    JsonElement bridgeCall(JsonObject request) {
        request.addProperty("handle",handle);
        return call(request);
    }

    private JsonElement request(String operation, BlockPos pos) {
        var request = new JsonObject();
        request.addProperty("op",operation);
        if (pos != null) { request.addProperty("x",pos.getX()); request.addProperty("y",pos.getY()); request.addProperty("z",pos.getZ()); }
        return bridgeCall(request);
    }

    private static final int ATTACHMENT_SWEEP_TICKS = 20;
    private int attachmentCountdown = ATTACHMENT_SWEEP_TICKS;

    void syncChunks(com.google.gson.JsonArray chunks) {
        var alive = new java.util.HashSet<ChunkPos>();
        for (var element : chunks) {
            var coordinates = element.getAsJsonArray();
            var pos = new ChunkPos(coordinates.get(0).getAsInt(),coordinates.get(1).getAsInt());
            alive.add(pos);
            if (!loadedViews.containsKey(pos)) {
                var chunk = RustChunk.create(this,pos.x(),pos.z());
                loadedViews.put(pos,chunk);
                postLifecycle(new net.neoforged.neoforge.event.level.ChunkEvent.Load(chunk,false));
            }
        }
        for (var pos : new ArrayList<>(loadedViews.keySet())) {
            if (!alive.contains(pos)) {
                var chunk = loadedViews.remove(pos);
                chunk.saveAttachments();
                postLifecycle(new net.neoforged.neoforge.event.level.ChunkEvent.Unload(chunk));
            }
        }
        // Mods change attachment objects in place without telling the chunk, so Rust gets a copy every second.
        if (--attachmentCountdown <= 0) {
            attachmentCountdown = ATTACHMENT_SWEEP_TICKS;
            for (var chunk : loadedViews.values()) chunk.saveAttachments();
        }
    }

    private static CompoundTag decode(String data) throws IOException {
        byte[] bytes = Base64.getDecoder().decode(data);
        if (bytes.length>12*1024*1024) throw new IOException("Block entity NBT exceeds 12 MiB");
        return NbtIo.read(new DataInputStream(new ByteArrayInputStream(bytes)),NbtAccounter.create(12*1024*1024));
    }
    private static byte[] bytes(CompoundTag tag) throws IOException {
        var bytes = new ByteArrayOutputStream();
        NbtIo.write(tag,new DataOutputStream(bytes));
        return bytes.toByteArray();
    }
    private static String encode(CompoundTag tag) throws IOException {
        return Base64.getEncoder().encodeToString(bytes(tag));
    }

    /**
     * The Java block entity for a Rust one. {@code sent} is data Rust replaced since Java last saw it, or null when
     * Java's copy is current; a block entity Java does not hold yet is loaded from Rust.
     */
    private Entry entity(BlockPos pos, BlockState state, String sent) throws Exception {
        var old = entities.get(pos);
        if (old != null && old.state.getBlock()==state.getBlock()) {
            old.be.setBlockState(state);
            old.state=state;
            if (sent != null) {
                old.be.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING,REGISTRIES,decode(sent)));
                old.saved = bytes(old.be.saveWithFullMetadata(REGISTRIES));
            }
            return old;
        }
        if (old != null) old.be.setRemoved();
        if (!(state.getBlock() instanceof EntityBlock block)) { entities.remove(pos); return null; }
        var be = block.newBlockEntity(pos,state);
        if (be == null) { entities.remove(pos); return null; }
        if (sent == null) {
            var stored = request("be",pos);
            if (stored.isJsonNull()) { entities.remove(pos); return null; }
            sent = stored.getAsString();
        }
        be.setLevel(this);
        if (sent != null) be.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING,REGISTRIES,decode(sent)));
        var entry = new Entry(be,state,bytes(be.saveWithFullMetadata(REGISTRIES)));
        entities.put(pos.immutable(),entry);
        be.clearRemoved();
        be.onLoad();
        return entry;
    }

    /** Takes the world values Rust sends with every request that leases this level. */
    void configure(JsonObject request) throws Exception {
        handle=request.get("handle").getAsInt();
        seed=request.get("seed").getAsLong();
        seaLevel=request.get("seaLevel").getAsInt();
        gameTime=request.get("time").getAsLong();
        dayTime=request.get("dayTime").getAsLong();
        if (dataFolder == null && request.has("dataFolder")) dataFolder=java.nio.file.Path.of(request.get("dataFolder").getAsString());
        configure(request.get("minY").getAsInt(),request.get("height").getAsInt(),request.get("dimension").getAsString());
    }

    void configure(int minY,int height,String dimension) throws Exception {
        minimumY=minY; levelHeight=height;
        if (!dimension.equals(dimensionName)) {
            dimensionName=dimension;
            var key = net.minecraft.resources.ResourceKey.create(net.minecraft.core.registries.Registries.DIMENSION,net.minecraft.resources.Identifier.parse(dimension));
            UNSAFE.putObject(this,UNSAFE.objectFieldOffset(Level.class.getDeclaredField("dimension")),key);
            // Mods find worlds through the server (overworld(), getLevel) as well as through their own level.
            var levels = MinecraftServer.class.getDeclaredField("levels");
            levels.setAccessible(true);
            ((Map<net.minecraft.resources.ResourceKey<Level>,ServerLevel>) levels.get(standInServer())).put(key,this);
            var typeKey = dimension.equals("minecraft:the_nether") ? net.minecraft.world.level.dimension.BuiltinDimensionTypes.NETHER
                : dimension.equals("minecraft:the_end") ? net.minecraft.world.level.dimension.BuiltinDimensionTypes.END : net.minecraft.world.level.dimension.BuiltinDimensionTypes.OVERWORLD;
            var type = REGISTRIES.lookupOrThrow(net.minecraft.core.registries.Registries.DIMENSION_TYPE).getOrThrow(typeKey);
            UNSAFE.putObject(this,UNSAFE.objectFieldOffset(Level.class.getDeclaredField("dimensionTypeRegistration")),type);
            setEnvironmentAttributes(environmentAttributes(this));
            levelLoaded(this);
        }
    }
    /**
     * ServerLevel's constructor builds this with addDefaultLayers. Its timeline layers need the server's clock manager,
     * which Rust owns, so only the dimension and biome layers apply.
     */
    private static net.minecraft.world.attribute.EnvironmentAttributeSystem environmentAttributes(RustLevel level) throws ReflectiveOperationException {
        var type = net.minecraft.world.attribute.EnvironmentAttributeSystem.class;
        var builder = net.minecraft.world.attribute.EnvironmentAttributeSystem.builder();
        var dimension = type.getDeclaredMethod("addDimensionLayer",builder.getClass(),net.minecraft.world.level.dimension.DimensionType.class);
        dimension.setAccessible(true);
        dimension.invoke(null,builder,level.dimensionType());
        var biome = type.getDeclaredMethod("addBiomeLayer",builder.getClass(),net.minecraft.core.HolderLookup.class,net.minecraft.world.level.biome.BiomeManager.class);
        biome.setAccessible(true);
        biome.invoke(null,builder,REGISTRIES.lookupOrThrow(net.minecraft.core.registries.Registries.BIOME),level.getBiomeManager());
        return builder.build();
    }

    /**
     * Mixin handlers mods inject into ServerLevel.tick (Draconic's world tasks), which Rust never runs. HEAD handlers
     * run with the level pre-tick event, TAIL and RETURN ones with the post-tick event.
     */
    private static List<java.lang.reflect.Method>[] tickHooks;

    @SuppressWarnings("unchecked")
    private static synchronized List<java.lang.reflect.Method>[] tickHooks() {
        if (tickHooks != null) return tickHooks;
        List<java.lang.reflect.Method> head = new ArrayList<>(), tail = new ArrayList<>();
        for (var method : ServerLevel.class.getDeclaredMethods()) {
            var types = method.getParameterTypes();
            // Merged injector handlers keep only @MixinMerged; tick(BooleanSupplier) is ServerLevel's only method with
            // that parameter, so a handler taking it with a CallbackInfo targets the tick. Their injection point is
            // not kept, and mods hook the end of the tick, so they run with the post-tick event.
            if (!method.getName().startsWith("handler$") || types.length != 2 || types[0] != java.util.function.BooleanSupplier.class
                || !types[1].getName().equals("org.spongepowered.asm.mixin.injection.callback.CallbackInfo")) continue;
            method.setAccessible(true);
            tail.add(method);
        }
        LOGGER.info("Running {} mod ServerLevel.tick hooks: {} {}", head.size() + tail.size(), head.stream().map(java.lang.reflect.Method::getName).toList(), tail.stream().map(java.lang.reflect.Method::getName).toList());
        tickHooks = new List[] { head, tail };
        return tickHooks;
    }

    void runTickHooks(boolean post) {
        for (var method : tickHooks()[post ? 1 : 0]) {
            try { method.invoke(this, (java.util.function.BooleanSupplier) () -> false, method.getParameterTypes()[1].getConstructor(String.class, boolean.class).newInstance("tick", false)); }
            catch (ReflectiveOperationException | RuntimeException error) {
                var cause = error instanceof java.lang.reflect.InvocationTargetException wrapped ? wrapped.getCause() : error;
                if (cause instanceof VirtualMachineError fatal) throw fatal;
                var id = method.getName() + cause;
                if (FAILED_TICKS.size() < 4096 && FAILED_TICKS.add(id)) LOGGER.error("Mod level tick hook {} failed (logged once)", method.getName(), cause);
            }
        }
    }

    String worldId() { return worldId; }
    String dimensionName() { return dimensionName; }

    static String worldFor(net.minecraft.resources.ResourceKey<Level> dimension) {
        for (var level : LEVELS.values()) if (level.dimensionName != null && level.dimensionName.equals(dimension.identifier().toString())) return level.worldId;
        return null;
    }

    static RustLevel forWorld(String world) throws Exception {
        RustLevel level = LEVELS.get(world);
        if (level == null) {
            synchronized (LEVELS) {
                level = LEVELS.get(world);
                if (level == null) { level=create(); level.worldId=world; LEVELS.put(world,level); }
            }
        }
        return level;
    }

    @SuppressWarnings({"rawtypes","unchecked"})
    public static String tick(String input) throws Exception {
        var batch = JSON.fromJson(input,JsonObject.class);
        RustLevel level = forWorld(batch.get("world").getAsString());
        // Worlds tick concurrently; a level's block entities only ever see one batch at a time, in Rust's order.
        synchronized (level) { return level.tickBatch(batch); }
    }

    @SuppressWarnings({"rawtypes","unchecked"})
    private String tickBatch(JsonObject batch) throws Exception {
        var level = this;
        level.configure(batch);
        var active = new LinkedHashSet<BlockPos>();
        try {
            for (var element : batch.getAsJsonArray("entities")) {
                var data=element.getAsJsonObject();
                var pos = new BlockPos(data.get("x").getAsInt(),data.get("y").getAsInt(),data.get("z").getAsInt());
                active.add(pos);
                level.entity(pos,Block.stateById(data.get("state").getAsInt()),data.has("nbt") ? data.get("nbt").getAsString() : null);
            }
            // Loaded block entities that Rust did not tick this round (unloaded neighbours) stay alive.
            var retained = new HashSet<BlockPos>();
            for (var element : batch.getAsJsonArray("retained")) {
                var data=element.getAsJsonObject();
                retained.add(new BlockPos(data.get("x").getAsInt(),data.get("y").getAsInt(),data.get("z").getAsInt()));
            }
            var iterator=level.entities.entrySet().iterator();
            while(iterator.hasNext()) {
                var entry=iterator.next();
                if (active.contains(entry.getKey()) || retained.contains(entry.getKey())) continue;
                iterator.remove();
                try { entry.getValue().be.onChunkUnloaded(); entry.getValue().be.setRemoved(); }
                catch (RuntimeException error) { LOGGER.warn("Block entity removal failed at {}",entry.getKey(),error); }
            }
            for (var pos : active) {
                var entry=level.entities.get(pos);
                if (entry == null) continue;
                var current=level.getBlockState(pos);
                if (current.getBlock()!=entry.state.getBlock()) continue;
                BlockEntityTicker ticker=current.getTicker(level,entry.be.getType());
                if (ticker == null) continue;
                try { ticker.tick(level,pos,current,entry.be); }
                catch (RuntimeException error) {
                    var id = level.worldId + pos + entry.be.getClass().getName();
                    if (FAILED_TICKS.size() < 4096 && FAILED_TICKS.add(id)) LOGGER.error("Block entity tick failed at {} (logged once)",pos,error);
                }
            }
            // Only data that changed goes back: block entities that called setChanged, and every so often all of
            // them for mods that change state without it, as a vanilla chunk save would still catch.
            var result = new JsonArray();
            boolean sweep = --level.sweepCountdown <= 0;
            if (sweep) level.sweepCountdown = SAVE_INTERVAL_BATCHES;
            for (var entry : level.entities.entrySet()) {
                var pos=entry.getKey();
                var be=entry.getValue();
                if (!sweep && !level.changed.contains(pos)) continue;
                if (vanilla(be.be)) continue;
                if (level.getBlockState(pos).getBlock()!=be.state.getBlock()) continue;
                var saved=bytes(be.be.saveWithFullMetadata(REGISTRIES));
                if (Arrays.equals(saved,be.saved)) continue;
                be.saved=saved;
                var data=new JsonObject();
                data.addProperty("x",pos.getX()); data.addProperty("y",pos.getY()); data.addProperty("z",pos.getZ());
                data.addProperty("nbt",Base64.getEncoder().encodeToString(saved));
                result.add(data);
            }
            level.changed.clear();
            level.flushVanilla();
            var reply = new JsonObject();
            reply.add("entities",result);
            reply.add("packets",Outbox.drain());
            level.chunks.saveDataSometimes();
            return reply.toString();
        } catch (Exception | Error error) { level.entities.clear(); level.changed.clear(); throw error; }
    }

    private static boolean vanilla(BlockEntity be) {
        var key = BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(be.getType());
        return key != null && key.getNamespace().equals("minecraft");
    }

    /**
     * Vanilla block entities belong to Rust; Java only borrows them for a mod's lookup (getBlockEntity, capabilities).
     * What changed goes back to Rust's block entity now, and the borrowed copy is dropped so the next lookup reads
     * Rust's current data. One failed entry never stops the others.
     */
    void flushVanilla() {
        var iterator = entities.entrySet().iterator();
        while (iterator.hasNext()) {
            var entry = iterator.next();
            var pos = entry.getKey();
            var held = entry.getValue();
            if (!vanilla(held.be)) continue;
            iterator.remove();
            changed.remove(pos);
            try {
                if (getBlockState(pos).getBlock() == held.state.getBlock()) {
                    var saved = bytes(held.be.saveWithFullMetadata(REGISTRIES));
                    if (!Arrays.equals(saved, held.saved)) commitVanilla(pos, held.be, saved);
                }
            } catch (Exception error) {
                LOGGER.warn("Vanilla block entity write-back failed at {}", pos, error);
            }
            try { held.be.setRemoved(); } catch (RuntimeException error) { LOGGER.debug("Releasing borrowed block entity at {} failed", pos, error); }
        }
    }

    private void commitVanilla(BlockPos pos, BlockEntity be, byte[] saved) throws IOException {
        var request = new JsonObject();
        request.addProperty("op", "commit_be");
        request.addProperty("x", pos.getX()); request.addProperty("y", pos.getY()); request.addProperty("z", pos.getZ());
        request.addProperty("id", BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(be.getType()).toString());
        // Container slots travel as Rust-shaped stacks so mod items keep their components.
        if (be instanceof net.minecraft.world.Container container) {
            var slots = new JsonArray();
            for (int slot = 0; slot < container.getContainerSize(); slot++) {
                var stack = container.getItem(slot);
                slots.add(stack.isEmpty() ? "" : PlayerBridge.encode(PlayerBridge.stackToRust(stack)));
            }
            request.add("slots", slots);
        }
        request.addProperty("nbt", Base64.getEncoder().encodeToString(saved));
        bridgeCall(request);
    }

    /** Level.neighborUpdater: Rust runs neighbour and shape updates in its own order and calls Java for mod blocks. */
    private final class RustNeighborUpdater extends net.minecraft.world.level.redstone.CollectingNeighborUpdater {
        RustNeighborUpdater() { super(RustLevel.this,0); }
        private JsonObject update(String op,BlockPos pos) {
            var request = new JsonObject();
            request.addProperty("op",op);
            request.addProperty("x",pos.getX()); request.addProperty("y",pos.getY()); request.addProperty("z",pos.getZ());
            return request;
        }
        @Override public void shapeUpdate(net.minecraft.core.Direction direction,BlockState neighborState,BlockPos pos,BlockPos neighborPos,int flags,int recursion) {
            var request = update("shape_update",pos);
            request.addProperty("direction",direction.get3DDataValue());
            request.addProperty("flags",flags);
            bridgeCall(request);
        }
        @Override public void neighborChanged(BlockPos pos,Block block,net.minecraft.world.level.redstone.Orientation orientation) {
            var request = update("neighbor_update",pos);
            request.addProperty("source",Block.getId(block.defaultBlockState()));
            bridgeCall(request);
        }
        @Override public void neighborChanged(BlockState state,BlockPos pos,Block block,net.minecraft.world.level.redstone.Orientation orientation,boolean movedByPiston) { neighborChanged(pos,block,orientation); }
        @Override public void updateNeighborsAtExceptFromFacing(BlockPos pos,Block block,net.minecraft.core.Direction skip,net.minecraft.world.level.redstone.Orientation orientation) {
            for (var direction : UPDATE_ORDER) if (direction != skip) neighborChanged(pos.relative(direction),block,null);
        }
    }

    /** Block ticks Java schedules go to Rust's tick queue; Rust calls back with scheduled_tick when one is due. */
    private final class RustBlockTicks extends net.minecraft.world.ticks.LevelTicks<Block> {
        RustBlockTicks() { super(chunk -> true); }
        private JsonObject tick(BlockPos pos, Block block) {
            var request = new JsonObject();
            request.addProperty("op", "schedule_tick");
            request.addProperty("x", pos.getX()); request.addProperty("y", pos.getY()); request.addProperty("z", pos.getZ());
            request.addProperty("state", Block.getId(block.defaultBlockState()));
            return request;
        }
        @Override public void schedule(net.minecraft.world.ticks.ScheduledTick<Block> tick) {
            var request = tick(tick.pos(), tick.type());
            request.addProperty("delay", tick.triggerTick() - getGameTime());
            request.addProperty("priority", tick.priority().getValue());
            bridgeCall(request);
        }
        @Override public boolean hasScheduledTick(BlockPos pos, Block block) {
            var request = tick(pos, block);
            request.addProperty("query", true);
            return bridgeCall(request).getAsBoolean();
        }
        @Override public boolean willTickThisTick(BlockPos pos, Block block) { return false; }
        @Override public int count() { return 0; }
    }
    private RustBlockTicks blockTicks;
    @Override public net.minecraft.world.ticks.LevelTicks<Block> getBlockTicks() {
        if (blockTicks == null) blockTicks = new RustBlockTicks();
        return blockTicks;
    }
    private static final net.minecraft.world.ticks.LevelTicks<net.minecraft.world.level.material.Fluid> NO_FLUID_TICKS =
        new net.minecraft.world.ticks.LevelTicks<>(chunk -> false) {
            @Override public void schedule(net.minecraft.world.ticks.ScheduledTick<net.minecraft.world.level.material.Fluid> tick) { }
        };
    /** Rust simulates fluids. */
    @Override public net.minecraft.world.ticks.LevelTicks<net.minecraft.world.level.material.Fluid> getFluidTicks() { return NO_FLUID_TICKS; }

    /**
     * LevelChunk.setBlockState's preRemoveSideEffects for a mod block Rust is replacing. Rust still holds the block
     * entity's data, so a block entity Java has not loaded yet comes from there.
     */
    JsonObject blockRemoving(JsonObject request) throws Exception {
        var pos = position(request);
        var old = Block.stateById(request.get("old").getAsInt());
        var held = entities.remove(pos);
        changed.remove(pos);
        BlockEntity be = held == null ? null : held.be;
        if (be == null && old.getBlock() instanceof EntityBlock block) {
            var stored = request("be", pos);
            be = stored.isJsonNull() ? null : block.newBlockEntity(pos, old);
            if (be != null) {
                be.setLevel(this);
                be.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING, REGISTRIES, decode(stored.getAsString())));
            }
        }
        if (be != null) {
            be.preRemoveSideEffects(pos, old);
            be.setRemoved();
        }
        return new JsonObject();
    }

    /** Level.setBlock's callbacks for a mod block after Rust changed the state: onPlace, then affectNeighborsAfterRemoval. */
    JsonObject blockChanged(JsonObject request) {
        var pos = position(request);
        var old = Block.stateById(request.get("old").getAsInt());
        var current = Block.stateById(request.get("new").getAsInt());
        boolean moved = request.get("moved").getAsBoolean();
        if (request.get("place").getAsBoolean()) current.onPlace(this, pos, old, moved);
        if (request.get("neighbors").getAsBoolean()) old.affectNeighborsAfterRemoval(this, pos, moved);
        return new JsonObject();
    }

    JsonObject neighborChanged(JsonObject request) {
        var pos = position(request);
        getBlockState(pos).handleNeighborChanged(this, pos, Block.stateById(request.get("source").getAsInt()).getBlock(), null, false);
        return new JsonObject();
    }

    JsonObject scheduledTick(JsonObject request) {
        var pos = position(request);
        getBlockState(pos).tick(this, pos, rustRandom);
        return new JsonObject();
    }

    /** Rust's random tick samples for mod blocks, as ServerLevel.tickChunk runs them: only randomly ticking states. */
    JsonObject randomTicks(JsonObject request) {
        for (var element : request.getAsJsonArray("positions")) {
            var xyz = element.getAsJsonArray();
            var pos = new BlockPos(xyz.get(0).getAsInt(), xyz.get(1).getAsInt(), xyz.get(2).getAsInt());
            try {
                var state = getBlockState(pos);
                if (state.isRandomlyTicking()) state.randomTick(this, pos, rustRandom);
            } catch (Throwable error) {
                String id = "random_tick:" + getBlockState(pos).getBlock();
                if (FAILED_TICKS.size() < 4096 && FAILED_TICKS.add(id)) LOGGER.error("Random tick failed at {} (logged once)", pos, error);
            }
        }
        return new JsonObject();
    }

    private static BlockPos position(JsonObject request) {
        return new BlockPos(request.get("x").getAsInt(),request.get("y").getAsInt(),request.get("z").getAsInt());
    }

    /** What BlockItem.place does after the block is set: block entity data and components from the stack, then setPlacedBy. */
    JsonObject blockPlaced(JsonObject request,net.minecraft.world.entity.LivingEntity placer) throws Exception {
        var pos = position(request);
        var stack = PlayerBridge.stackFromRust(decode(request.get("stack").getAsString()));
        var state = getBlockState(pos);
        BlockItem.updateCustomBlockEntityTag(this,placer instanceof net.minecraft.world.entity.player.Player player ? player : null,pos,stack);
        var reply = new JsonObject();
        var be = getBlockEntity(pos);
        if (be != null) be.applyComponentsFromItemStack(stack);
        state.getBlock().setPlacedBy(this,pos,state,placer,stack);
        var entry = entities.get(pos);
        if (entry != null) {
            entry.saved = bytes(entry.be.saveWithFullMetadata(REGISTRIES));
            reply.addProperty("nbt",Base64.getEncoder().encodeToString(entry.saved));
        }
        return reply;
    }

    /**
     * BlockItem.getPlacementState for the player's click: the block's getStateForPlacement, kept when it can survive.
     * "state" is -1 when the block refuses the spot.
     */
    JsonObject placementState(JsonObject request,net.minecraft.server.level.ServerPlayer placer) throws Exception {
        var reply = new JsonObject();
        if (placer == null) return reply;
        var stack = PlayerBridge.stackFromRust(decode(request.get("stack").getAsString()));
        var clicked = position(request);
        var face = net.minecraft.core.Direction.from3DDataValue(request.get("face").getAsInt());
        var cursor = new net.minecraft.world.phys.Vec3(clicked.getX() + request.get("cx").getAsDouble(), clicked.getY() + request.get("cy").getAsDouble(), clicked.getZ() + request.get("cz").getAsDouble());
        var hand = request.get("hand").getAsInt() == 1 ? net.minecraft.world.InteractionHand.OFF_HAND : net.minecraft.world.InteractionHand.MAIN_HAND;
        var context = new net.minecraft.world.item.context.BlockPlaceContext(this,placer,hand,stack,new net.minecraft.world.phys.BlockHitResult(cursor,face,clicked,request.get("inside").getAsBoolean()));
        var target = new BlockPos(request.get("px").getAsInt(),request.get("py").getAsInt(),request.get("pz").getAsInt());
        if (!context.getClickedPos().equals(target)) context = net.minecraft.world.item.context.BlockPlaceContext.at(context,target,face);
        var block = Block.stateById(request.get("block").getAsInt()).getBlock();
        var state = block.getStateForPlacement(context);
        reply.addProperty("state", state != null && state.canSurvive(this,target) ? Block.getId(state) : -1);
        return reply;
    }

    /** BlockBehaviour.updateShape for a mod block whose neighbour changed. */
    JsonObject updateShape(JsonObject request) {
        var pos = position(request);
        var direction = net.minecraft.core.Direction.from3DDataValue(request.get("direction").getAsInt());
        var state = Block.stateById(request.get("state").getAsInt());
        var neighbor = Block.stateById(request.get("neighbor").getAsInt());
        var reply = new JsonObject();
        reply.addProperty("state", Block.getId(state.updateShape(this,this,pos,direction,pos.relative(direction),neighbor,rustRandom)));
        return reply;
    }

    /** Block.getDrops against the Rust block entity: loot tables, component copying and mod overrides all apply. */
    JsonObject blockDrops(JsonObject request) throws Exception {
        var pos = position(request);
        var tool = request.has("tool") ? PlayerBridge.stackFromRust(decode(request.get("tool").getAsString())) : net.minecraft.world.item.ItemStack.EMPTY;
        var drops = new JsonArray();
        for (var stack : Block.getDrops(getBlockState(pos),this,pos,getBlockEntity(pos),null,tool)) {
            if (!stack.isEmpty()) drops.add(PlayerBridge.encode(PlayerBridge.stackToRust(stack)));
        }
        var reply = new JsonObject();
        
        reply.add("drops",drops);
        return reply;
    }

    // Rust owns entity identities, so Java entities take their ids from it.
    @Override public int getNextEntityId() {
        var request = new JsonObject();
        request.addProperty("op","allocate_entity");
        return call(request).getAsJsonObject().get("id").getAsInt();
    }
    @Override public boolean addFreshEntity(net.minecraft.world.entity.Entity entity) { return EntityBridge.add(this,entity); }
    @Override public java.util.List<net.minecraft.server.level.ServerPlayer> players() { return PlayerBridge.playersIn(this); }
    @Override public net.minecraft.world.level.entity.LevelEntityGetter<net.minecraft.world.entity.Entity> getEntities() { return entityGetter; }

    private static net.minecraft.world.entity.Entity audible(net.minecraft.world.entity.Entity source) { return source instanceof net.minecraft.world.entity.player.Player ? source : null; }

    // Java encodes the packet for the 26.2 client; Rust resolves the area to players of this world.
    @Override public void playSeededSound(net.minecraft.world.entity.Entity source,double x,double y,double z,net.minecraft.core.Holder<net.minecraft.sounds.SoundEvent> sound,net.minecraft.sounds.SoundSource category,float volume,float pitch,long seed) {
        var origin = audible(source);
        Outbox.toArea(worldId,new net.minecraft.network.protocol.game.ClientboundSoundPacket(sound,category,x,y,z,volume,pitch,seed),x,y,z,sound.value().getRange(volume),origin==null ? -1 : origin.getId());
    }
    @Override public void playSeededSound(net.minecraft.world.entity.Entity source,net.minecraft.world.entity.Entity entity,net.minecraft.core.Holder<net.minecraft.sounds.SoundEvent> sound,net.minecraft.sounds.SoundSource category,float volume,float pitch,long seed) {
        var origin = audible(source);
        Outbox.toArea(worldId,new net.minecraft.network.protocol.game.ClientboundSoundEntityPacket(sound,category,entity,volume,pitch,seed),entity.getX(),entity.getY(),entity.getZ(),sound.value().getRange(volume),origin==null ? -1 : origin.getId());
    }
    @Override public void levelEvent(net.minecraft.world.entity.Entity source,int type,BlockPos pos,int data) {
        var origin = audible(source);
        Outbox.toArea(worldId,new net.minecraft.network.protocol.game.ClientboundLevelEventPacket(type,pos,data,false),pos.getX(),pos.getY(),pos.getZ(),64.0,origin==null ? -1 : origin.getId());
    }
    @Override public <T extends net.minecraft.core.particles.ParticleOptions> int sendParticles(T particle,double x,double y,double z,int count,double dx,double dy,double dz,double speed) {
        return sendParticles(particle,false,false,x,y,z,count,dx,dy,dz,speed);
    }
    @Override public <T extends net.minecraft.core.particles.ParticleOptions> int sendParticles(T particle,boolean overrideLimiter,boolean alwaysShow,double x,double y,double z,int count,double dx,double dy,double dz,double speed) {
        Outbox.toArea(worldId,new net.minecraft.network.protocol.game.ClientboundLevelParticlesPacket(particle,overrideLimiter,alwaysShow,x,y,z,(float)dx,(float)dy,(float)dz,(float)speed,count),x,y,z,overrideLimiter ? 512.0 : 32.0,-1);
        return count;
    }
    @Override public void gameEvent(net.minecraft.core.Holder<net.minecraft.world.level.gameevent.GameEvent> event,net.minecraft.world.phys.Vec3 pos,net.minecraft.world.level.gameevent.GameEvent.Context context) { }
    @Override public boolean isClientSide() { return false; }
    @Override public long getGameTime() { return gameTime; }
    @Override public RandomSource getRandom() { return rustRandom; }
    @Override public int getMinY() { return minimumY; }
    @Override public int getHeight() { return levelHeight; }
    /** Rust keeps the heightmaps; chunk views have none. Ocean floor maps use the motion blocking one. */
    @Override public int getHeight(net.minecraft.world.level.levelgen.Heightmap.Types type,int x,int z) {
        return BridgeRuntime.getHeight(handle,switch (type) {
            case WORLD_SURFACE_WG, WORLD_SURFACE -> 0;
            case MOTION_BLOCKING_NO_LEAVES -> 2;
            default -> 1;
        },x,z);
    }
    @Override public long getSeed() { return seed; }
    @Override public long getDefaultClockTime() { return dayTime; }
    @Override public long getOverworldClockTime() { return dayTime; }
    @Override public net.minecraft.server.level.ServerPlayer getRandomPlayer() {
        var players = players();
        return players.isEmpty() ? null : players.get(rustRandom.nextInt(players.size()));
    }
    @Override public net.minecraft.world.level.gamerules.GameRules getGameRules() { return GAME_RULES; }
    @Override public int getSeaLevel() { return seaLevel; }
    @Override public RegistryAccess registryAccess() { return REGISTRIES; }
    @Override public net.minecraft.world.item.crafting.RecipeManager recipeAccess() { return BridgeContent.recipes; }
    @Override public FuelValues fuelValues() { return BridgeContent.fuels; }
    @Override public net.minecraft.world.flag.FeatureFlagSet enabledFeatures() { return net.minecraft.world.level.WorldDataConfiguration.DEFAULT.enabledFeatures(); }
    @Override public BlockState getBlockState(BlockPos pos) { return Block.stateById(BridgeRuntime.getState(handle,pos.getX(),pos.getY(),pos.getZ())); }
    @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    @Override public boolean setBlock(BlockPos pos, BlockState state,int flags,int recursion) {
        return BridgeRuntime.setState(handle,pos.getX(),pos.getY(),pos.getZ(),Block.getId(state),flags);
    }
    @Override public boolean setBlock(BlockPos pos,BlockState state,int flags) { return setBlock(pos,state,flags,512); }
    @Override public BlockEntity getBlockEntity(BlockPos pos) {
        var state=getBlockState(pos);
        var existing=entities.get(pos);
        if (existing != null && existing.state.getBlock()==state.getBlock()) return existing.be;
        try { var entry=entity(pos,state,null); return entry==null ? null : entry.be; }
        catch(Exception error) { throw new IllegalStateException("Cannot load Rust block entity at "+pos,error); }
    }
    /** Rust commits the block entity's data with the next batch reply, which marks its chunk for saving. */
    @Override public void blockEntityChanged(BlockPos pos) { changed.add(pos.immutable()); }
    @Override public void updateNeighbourForOutputSignal(BlockPos pos,Block block) { request("comparator",pos); }
    @Override public void sendBlockUpdated(BlockPos pos,BlockState old,BlockState state,int flags) { request("update",pos); }
    /** Rust runs the explosion (blocks, damage, knockback, packets); Java's would only half apply it. */
    @Override public void explode(net.minecraft.world.entity.Entity source,net.minecraft.world.damagesource.DamageSource damage,net.minecraft.world.level.ExplosionDamageCalculator calculator,double x,double y,double z,float power,boolean fire,Level.ExplosionInteraction interaction,net.minecraft.core.particles.ParticleOptions small,net.minecraft.core.particles.ParticleOptions large,net.minecraft.util.random.WeightedList<net.minecraft.core.particles.ExplosionParticleInfo> blockParticles,net.minecraft.core.Holder<net.minecraft.sounds.SoundEvent> sound) {
        var request = new JsonObject();
        request.addProperty("op","explode");
        request.addProperty("ex",x); request.addProperty("ey",y); request.addProperty("ez",z);
        request.addProperty("power",power);
        request.addProperty("interaction",interaction.name());
        bridgeCall(request);
    }
    @Override public boolean hasChunk(int x,int z) { return BridgeRuntime.isLoaded(handle,x,z); }
    @Override public LevelChunk getChunk(int x,int z) { return loadedViews.computeIfAbsent(new ChunkPos(x,z),pos -> RustChunk.create(this,x,z)); }
    @Override public ServerChunkCache getChunkSource() { return chunks; }
    /** Light is stored in Rust's chunk sections; this view reads it through the hot natives. */
    @Override public net.minecraft.world.level.lighting.LevelLightEngine getLightEngine() { return light; }
    @Override public int getSkyDarken() { return BridgeRuntime.skyDarken(handle); }
    /** Biomes live in Rust's chunk sections; coordinates are quart positions like vanilla's NoiseBiomeSource. */
    @SuppressWarnings("unchecked")
    @Override public net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome> getUncachedNoiseBiome(int x,int y,int z) {
        int id = BridgeRuntime.biome(handle,x << 2,y << 2,z << 2);
        if (id < 0 || id >= biomes.length) throw new IllegalStateException("Rust biome id out of range: " + id);
        var cached = (net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome>) biomes[id];
        if (cached != null) return cached;
        var name = request("biome",new BlockPos(x << 2,y << 2,z << 2)).getAsString();
        var registry = REGISTRIES.lookupOrThrow(net.minecraft.core.registries.Registries.BIOME);
        var holder = registry.get(net.minecraft.resources.Identifier.parse(name.contains(":") ? name : "minecraft:" + name))
            .<net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome>>map(h -> h)
            .orElseGet(() -> registry.getOrThrow(net.minecraft.world.level.biome.Biomes.PLAINS));
        biomes[id] = holder;
        return holder;
    }
    @Override public net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome> getNoiseBiome(int x,int y,int z) { return getUncachedNoiseBiome(x,y,z); }
    @Override public ChunkAccess getChunk(int x,int z,ChunkStatus status,boolean create) { return hasChunk(x,z) ? getChunk(x,z) : null; }
    @Override public <T,C> T getCapability(BlockCapability<T,C> capability,BlockPos pos,C context) {
        return capability.getCapability(this,pos,getBlockState(pos),getBlockEntity(pos),context);
    }
    @Override public <T,C> T getCapability(BlockCapability<T,C> capability,BlockPos pos,BlockState state,BlockEntity be,C context) {
        return capability.getCapability(this,pos,state==null ? getBlockState(pos) : state,be==null ? getBlockEntity(pos) : be,context);
    }

    private static final class Entry {
        final BlockEntity be; BlockState state; byte[] saved;
        Entry(BlockEntity be,BlockState state,byte[] saved) { this.be=be; this.state=state; this.saved=saved; }
    }
    private LevelChunkSection template;
    /** Sections of one chunk column; each reads and writes the Rust world, so mods that walk sections see real blocks. */
    private LevelChunkSection[] sections(int chunkX,int chunkZ) {
        int count = levelHeight >> 4;
        try {
            if (template == null) template = new LevelChunkSection(net.minecraft.world.level.chunk.PalettedContainerFactory.create(REGISTRIES));
            var sections = new LevelChunkSection[count];
            for (int i = 0; i < count; i++) {
                var section = (RustSection) UNSAFE.allocateInstance(RustSection.class);
                // The containers stay empty: block data lives in Rust and every access below goes there.
                RustPlayer.set(section,LevelChunkSection.class,"states",template.getStates());
                RustPlayer.set(section,LevelChunkSection.class,"biomes",template.getBiomes());
                section.owner = this;
                section.x = chunkX << 4; section.y = minimumY + (i << 4); section.z = chunkZ << 4;
                sections[i] = section;
            }
            return sections;
        } catch (ReflectiveOperationException error) { throw new IllegalStateException(error); }
    }
    private static final class RustSection extends LevelChunkSection {
        private RustLevel owner;
        private int x, y, z;
        private RustSection() { super(null,null); }
        @Override public BlockState getBlockState(int sx,int sy,int sz) { return Block.stateById(BridgeRuntime.getState(owner.handle,x+sx,y+sy,z+sz)); }
        @Override public net.minecraft.world.level.material.FluidState getFluidState(int sx,int sy,int sz) { return getBlockState(sx,sy,sz).getFluidState(); }
        /** LevelChunkSection writes skip neighbour updates; Rust still sends the change to clients. */
        @Override public BlockState setBlockState(int sx,int sy,int sz,BlockState state,boolean checkThreading) {
            var old = getBlockState(sx,sy,sz);
            BridgeRuntime.setState(owner.handle,x+sx,y+sy,z+sz,Block.getId(state),2|16);
            return old;
        }
        @Override public boolean hasFluid() { return true; }
        @Override public boolean hasOnlyAir() { return false; }
        @Override public boolean maybeHas(java.util.function.Predicate<BlockState> predicate) { return true; }
    }
    private static final class RustChunk extends LevelChunk {
        private RustLevel owner;
        private ChunkPos position;
        private RustChunk(RustLevel level,int x,int z) { super(level,new ChunkPos(x,z)); }
        static RustChunk create(RustLevel level,int x,int z) {
            try {
                var chunk=(RustChunk)UNSAFE.allocateInstance(RustChunk.class); chunk.owner=level; chunk.position=new ChunkPos(x,z);
                // Mods keep per-chunk data (HBM conductor networks) as attachments; the constructor that makes the holder did not run.
                chunk.holder=new net.neoforged.neoforge.attachment.AttachmentHolder.AsField(chunk);
                RustPlayer.set(chunk,ChunkAccess.class,"attachmentHolder",chunk.holder);
                chunk.loadAttachments();
                return chunk;
            }
            catch(ReflectiveOperationException error) { throw new IllegalStateException(error); }
        }
        private net.neoforged.neoforge.attachment.AttachmentHolder.AsField holder;
        private byte[] savedAttachments;
        private BlockPos origin() { return new BlockPos(position.x()<<4,owner.minimumY,position.z()<<4); }
        /** Rust keeps the chunk's attachments in its chunk NBT; Java restores them when it first views the chunk. */
        private void loadAttachments() {
            var data = owner.request("chunk_attachments",origin());
            if (data == null || data.isJsonNull()) return;
            try {
                var tag = decode(data.getAsString());
                holder.deserializeInternal(owner.registryAccess(),net.minecraft.world.level.storage.TagValueInput.create(net.minecraft.util.ProblemReporter.DISCARDING,owner.registryAccess(),tag));
                savedAttachments = bytes(tag);
            } catch (IOException | RuntimeException error) { LOGGER.warn("Chunk attachments of {} were not restored",position,error); }
        }
        /** Hands changed attachments to Rust's chunk, which saves them with the chunk. */
        void saveAttachments() {
            if (!holder.hasAttachments()) return;
            try {
                var output = net.minecraft.world.level.storage.TagValueOutput.createWithContext(net.minecraft.util.ProblemReporter.DISCARDING,owner.registryAccess());
                holder.serializeAttachments(output);
                var tag = output.buildResult();
                var saved = bytes(tag);
                if (Arrays.equals(saved,savedAttachments)) return;
                savedAttachments = saved;
                var request = new JsonObject();
                request.addProperty("op","set_chunk_attachments");
                var at = origin();
                request.addProperty("x",at.getX()); request.addProperty("y",at.getY()); request.addProperty("z",at.getZ());
                request.addProperty("nbt",Base64.getEncoder().encodeToString(saved));
                owner.bridgeCall(request);
            } catch (IOException | RuntimeException error) { LOGGER.warn("Chunk attachments of {} were not saved",position,error); }
        }
        @Override public Level getLevel() { return owner; }
        @Override public ChunkPos getPos() { return position; }
        @Override public int getMinY() { return owner.minimumY; }
        @Override public int getHeight() { return owner.levelHeight; }
        @Override public int getHeight(net.minecraft.world.level.levelgen.Heightmap.Types type,int x,int z) {
            return owner.getHeight(type,(position.x()<<4)+x,(position.z()<<4)+z) - 1;
        }
        private LevelChunkSection[] sections;
        @Override public LevelChunkSection[] getSections() {
            if (sections == null || sections.length != owner.levelHeight >> 4) sections = owner.sections(position.x(),position.z());
            return sections;
        }
        @Override public Map<BlockPos,BlockEntity> getBlockEntities() {
            var result = new LinkedHashMap<BlockPos,BlockEntity>();
            owner.entities.forEach((pos,entry) -> {
                if ((pos.getX()>>4)==position.x() && (pos.getZ()>>4)==position.z()) result.put(pos,entry.be);
            });
            return Collections.unmodifiableMap(result);
        }
        @Override public net.minecraft.core.Holder<net.minecraft.world.level.biome.Biome> getNoiseBiome(int x,int y,int z) { return owner.getUncachedNoiseBiome(x,y,z); }
        @Override public BlockState getBlockState(BlockPos pos) { return owner.getBlockState(pos); }
        @Override public BlockState setBlockState(BlockPos pos,BlockState state,int flags) { var old=getBlockState(pos); owner.setBlock(pos,state,flags); return old; }
        @Override public BlockEntity getBlockEntity(BlockPos pos) { return owner.getBlockEntity(pos); }
        @Override public BlockEntity getBlockEntity(BlockPos pos,EntityCreationType type) { return getBlockEntity(pos); }
        @Override public void markUnsaved() { saveAttachments(); owner.request("dirty",origin()); }
        @Override public net.minecraft.server.level.FullChunkStatus getFullStatus() { return net.minecraft.server.level.FullChunkStatus.ENTITY_TICKING; }
    }
    /** Entity broadcasts go to Rust's tracker, which knows who sees the entity. */
    private static final class RustChunkMap extends net.minecraft.server.level.ChunkMap {
        private RustChunkMap() { super(null,null,null,null,null,null,null,null,null,null,null,0,false); }
        @Override public void sendToTrackingPlayers(net.minecraft.world.entity.Entity entity,net.minecraft.network.protocol.Packet<? super net.minecraft.network.protocol.game.ClientGamePacketListener> packet) { Outbox.toTracking(entity.getId(),false,packet); }
        @Override public void sendToTrackingPlayersAndSelf(net.minecraft.world.entity.Entity entity,net.minecraft.network.protocol.Packet<? super net.minecraft.network.protocol.game.ClientGamePacketListener> packet) { Outbox.toTracking(entity.getId(),true,packet); }
    }
    /** Read-only light view; Rust runs all light propagation, so every update hook is a no-op. */
    private static final class RustLightEngine extends net.minecraft.server.level.ThreadedLevelLightEngine {
        private RustLevel owner;
        private final net.minecraft.world.level.lighting.LayerLightEventListener[] layers = {
            new RustLightLayer(this, net.minecraft.world.level.LightLayer.SKY),
            new RustLightLayer(this, net.minecraft.world.level.LightLayer.BLOCK)
        };
        private RustLightEngine() { super(null,null,false,null,null); }
        @Override public net.minecraft.world.level.lighting.LayerLightEventListener getLayerListener(net.minecraft.world.level.LightLayer layer) { return layers[layer.ordinal()]; /* enum order: SKY, BLOCK */ }
        /** Vanilla: max(block, sky - amount). */
        @Override public int getRawBrightness(BlockPos pos,int amount) {
            int sky = BridgeRuntime.light(owner.handle,1,pos.getX(),pos.getY(),pos.getZ()) - amount;
            int block = BridgeRuntime.light(owner.handle,0,pos.getX(),pos.getY(),pos.getZ());
            return Math.max(block,sky);
        }
        @Override public void close() { }
        @Override public int runLightUpdates() { return 0; }
        @Override public boolean hasLightWork() { return false; }
        @Override public void checkBlock(BlockPos pos) { }
        @Override public void updateSectionStatus(net.minecraft.core.SectionPos pos,boolean empty) { }
        @Override public void propagateLightSources(ChunkPos pos) { }
        @Override public void setLightEnabled(ChunkPos pos,boolean enabled) { }
        @Override public void queueSectionData(net.minecraft.world.level.LightLayer layer,net.minecraft.core.SectionPos pos,net.minecraft.world.level.chunk.DataLayer data) { }
        @Override public void retainData(ChunkPos pos,boolean retain) { }
        @Override public void tryScheduleUpdate() { }
        @Override public boolean lightOnInColumn(long column) { return true; }
        @Override public int getLightSectionCount() { return owner.getSectionsCount() + 2; }
        @Override public int getMinLightSection() { return owner.getMinSectionY() - 1; }
        @Override public int getMaxLightSection() { return owner.getMaxSectionY() + 1; }
    }
    private static final class RustLightLayer implements net.minecraft.world.level.lighting.LayerLightEventListener {
        private final RustLightEngine engine;
        private final int layer;
        private RustLightLayer(RustLightEngine engine,net.minecraft.world.level.LightLayer layer) { this.engine = engine; this.layer = layer == net.minecraft.world.level.LightLayer.SKY ? 1 : 0; }
        @Override public net.minecraft.world.level.chunk.DataLayer getDataLayerData(net.minecraft.core.SectionPos pos) { return null; }
        @Override public int getLightValue(BlockPos pos) { return BridgeRuntime.light(engine.owner.handle,layer,pos.getX(),pos.getY(),pos.getZ()); }
        @Override public void checkBlock(BlockPos pos) { }
        @Override public boolean hasLightWork() { return false; }
        @Override public int runLightUpdates() { return 0; }
        @Override public void updateSectionStatus(net.minecraft.core.SectionPos pos,boolean empty) { }
        @Override public void setLightEnabled(ChunkPos pos,boolean enabled) { }
        @Override public void propagateLightSources(ChunkPos pos) { }
    }
    private static final class RustChunkSource extends ServerChunkCache {
        private RustLevel owner;
        private RustChunkSource(RustLevel level) { super(level,null,null,null,Runnable::run,null,0,0,false,null,()->null); }
        @Override public LevelChunk getChunkNow(int x,int z) { return owner.hasChunk(x,z) ? owner.getChunk(x,z) : null; }
        @Override public ChunkAccess getChunk(int x,int z,ChunkStatus status,boolean create) { return owner.getChunk(x,z,status,create); }
        @Override public boolean hasChunk(int x,int z) { return owner.hasChunk(x,z); }
        @Override public Level getLevel() { return owner; }
        @Override public net.minecraft.server.level.ThreadedLevelLightEngine getLightEngine() { return owner.light; }
        @Override public void blockChanged(BlockPos pos) { owner.request("dirty",pos); }
        @Override public void sendToTrackingPlayers(net.minecraft.world.entity.Entity entity,net.minecraft.network.protocol.Packet<? super net.minecraft.network.protocol.game.ClientGamePacketListener> packet) { Outbox.toTracking(entity.getId(),false,packet); }
        @Override public void sendToTrackingPlayersAndSelf(net.minecraft.world.entity.Entity entity,net.minecraft.network.protocol.Packet<? super net.minecraft.network.protocol.game.ClientGamePacketListener> packet) { Outbox.toTracking(entity.getId(),true,packet); }
        private net.minecraft.world.level.storage.SavedDataStorage dataStorage;
        private int saveCountdown = SAVE_INTERVAL_BATCHES;
        /** Mod saved data (not blocks or entities) is the one thing Java persists; Rust owns the world itself. */
        @Override public synchronized net.minecraft.world.level.storage.SavedDataStorage getDataStorage() {
            if (dataStorage == null) {
                var dimension = owner.dimensionName.replace(':','_');
                var legacy = net.neoforged.fml.loading.FMLPaths.GAMEDIR.get().resolve("ferrum-saveddata").resolve(dimension).resolve("data");
                var folder = owner.dataFolder == null ? legacy : owner.dataFolder.resolve(dimension);
                migrate(legacy, folder);
                dataStorage = new net.minecraft.world.level.storage.SavedDataStorage(owner,folder,net.minecraft.util.datafix.DataFixers.getDataFixer(),REGISTRIES);
            }
            return dataStorage;
        }
        /** Mods read worldgen settings (biomes, features) from the generator; Rust generates, so this is vanilla's default stem for introspection only. */
        @Override public net.minecraft.world.level.chunk.ChunkGenerator getGenerator() {
            var key = owner.dimensionName.equals("minecraft:the_nether") ? net.minecraft.world.level.dimension.LevelStem.NETHER
                : owner.dimensionName.equals("minecraft:the_end") ? net.minecraft.world.level.dimension.LevelStem.END : net.minecraft.world.level.dimension.LevelStem.OVERWORLD;
            return defaultDimensions().get(key).orElseThrow().generator();
        }
        /** Saved data written before it moved into Rust's world folder. */
        private static void migrate(java.nio.file.Path legacy, java.nio.file.Path folder) {
            if (legacy.equals(folder) || !java.nio.file.Files.isDirectory(legacy) || java.nio.file.Files.exists(folder)) return;
            try (var files = java.nio.file.Files.list(legacy)) {
                java.nio.file.Files.createDirectories(folder);
                for (var file : files.toList()) java.nio.file.Files.copy(file, folder.resolve(file.getFileName()));
                LOGGER.info("Moved mod saved data from {} to {}", legacy, folder);
            } catch (java.io.IOException error) { LOGGER.error("Cannot move mod saved data from {}", legacy, error); }
        }
        synchronized void saveData() { if (dataStorage != null) dataStorage.saveAndJoin(); }
        synchronized void saveDataSometimes() {
            if (dataStorage != null && --saveCountdown <= 0) { saveCountdown = SAVE_INTERVAL_BATCHES; dataStorage.scheduleSave(); }
        }
        @Override public void tick(java.util.function.BooleanSupplier time,boolean chunks) { throw new UnsupportedOperationException("Rust owns chunk ticking"); }
        @Override public void save(boolean flush) { throw new UnsupportedOperationException("Rust owns chunk persistence"); }
    }
}

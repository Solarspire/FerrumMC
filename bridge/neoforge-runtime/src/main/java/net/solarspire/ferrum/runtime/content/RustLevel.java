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
    private int minimumY;
    private int levelHeight;
    private RandomSource rustRandom;
    private RustChunkSource chunks;
    private String worldId;
    private String dimensionName;
    private EntityBridge.Getter entityGetter;

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
        level.chunks = (RustChunkSource) UNSAFE.allocateInstance(RustChunkSource.class);
        level.chunks.owner = level;
        level.entityGetter = new EntityBridge.Getter(level);
        UNSAFE.putObject(level.chunks,UNSAFE.objectFieldOffset(ServerChunkCache.class.getDeclaredField("chunkMap")),UNSAFE.allocateInstance(RustChunkMap.class));
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(ServerLevel.class.getDeclaredField("waypointManager")),new net.minecraft.server.waypoints.ServerWaypointManager());
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(ServerLevel.class.getDeclaredField("dragonParts")),new it.unimi.dsi.fastutil.ints.Int2ObjectOpenHashMap<>());
        UNSAFE.putObject(level,UNSAFE.objectFieldOffset(ServerLevel.class.getDeclaredField("navigatingMobs")),new java.util.HashSet<>());
        level.rustRandom = new LegacyRandomSource(0) {
            @Override public int next(int bits) { return request("random",null).getAsInt() >>> (32-bits); }
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

    private static final int SAVE_INTERVAL_BATCHES = 1200;
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
            standIn = server;
            // NeoForge's server lifecycle: the hook records the current server, mods see it on the start events.
            try {
                var current = net.neoforged.neoforge.server.ServerLifecycleHooks.class.getDeclaredField("currentServer");
                current.setAccessible(true);
                current.set(null,server);
            } catch (ReflectiveOperationException error) { LOGGER.error("Cannot register the bridge server with NeoForge",error); }
            if (System.getenv("FERRUM_AUDIT") != null) audit(server,MinecraftServer.class);
            postLifecycle(new net.neoforged.neoforge.event.server.ServerAboutToStartEvent(server));
            postLifecycle(new net.neoforged.neoforge.event.server.ServerStartingEvent(server));
        }
        return standIn;
    }

    private static void postLifecycle(net.neoforged.bus.api.Event event) {
        try { net.neoforged.neoforge.common.NeoForge.EVENT_BUS.post(event); }
        catch (RuntimeException | LinkageError error) { LOGGER.error("A mod failed on {}",event.getClass().getSimpleName(),error); }
    }

    private static boolean serverStarted;

    // TEMP-AUDIT
    static void audit(Object target,Class<?> stop) {
        for (var method : target.getClass().getMethods()) {
            if (method.getParameterCount() != 0 || java.lang.reflect.Modifier.isStatic(method.getModifiers()) || method.getDeclaringClass() == Object.class) continue;
            if (!stop.isAssignableFrom(method.getDeclaringClass())) continue;
            String n = method.getName();
            if (n.startsWith("set") || n.startsWith("save") || n.startsWith("stop") || n.startsWith("tick") || n.startsWith("halt") || n.startsWith("run") || n.equals("close") || n.startsWith("wait") || n.startsWith("notify") || n.startsWith("reload") || n.startsWith("start") || n.startsWith("init") || n.startsWith("clear") || n.startsWith("remove") || n.startsWith("exit") || n.startsWith("shutdown") || n.startsWith("kick") || n.startsWith("create") || n.startsWith("load") || n.startsWith("spin") || n.startsWith("prepare") || n.startsWith("update") || n.startsWith("sync") || n.startsWith("restore") || n.startsWith("pause") || n.startsWith("resume") || n.startsWith("tryAdd") || n.startsWith("explode") || n.startsWith("nextSub") || n.startsWith("block") || n.startsWith("get") == false && n.startsWith("is") == false && n.startsWith("has") == false && !n.equals("overworld") && !n.equals("registryAccess") && !n.equals("dimension") && !n.equals("players")) continue;
            try { method.invoke(target); }
            catch (java.lang.reflect.InvocationTargetException e) { LOGGER.warn("AUDIT {}.{} -> {}",target.getClass().getSimpleName(),n,e.getCause().toString()); }
            catch (Throwable e) { }
        }
    }

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
        @Override public boolean isRunning() { return true; }
        @Override public boolean isStopped() { return false; }
        @Override public boolean isDedicatedServer() { return true; }
        @Override public int getTickCount() { var level = LEVELS.values().stream().findFirst(); return level.isPresent() ? (int) level.get().getGameTime() : 0; }
        @Override public Iterable<ServerLevel> getAllLevels() { return new ArrayList<ServerLevel>(LEVELS.values()); }
        @Override public int getPlayerCount() { return PlayerBridge.all().size(); }
        @Override public net.minecraft.world.level.gamerules.GameRules getGameRules() { return GAME_RULES; }
        // Weather and time belong to Rust's world.
        @Override public void setWeatherParameters(int clear,int rain,boolean raining,boolean thundering) { }
    }

    private static final net.minecraft.world.level.gamerules.GameRules GAME_RULES =
        new net.minecraft.world.level.gamerules.GameRules(net.minecraft.world.level.WorldDataConfiguration.DEFAULT.enabledFeatures());

    static JsonElement call(JsonObject request) { return JsonParser.parseString(BridgeRuntime.world(request.toString())); }

    private static JsonElement request(String operation, BlockPos pos) {
        var request = new JsonObject();
        request.addProperty("op",operation);
        if (pos != null) { request.addProperty("x",pos.getX()); request.addProperty("y",pos.getY()); request.addProperty("z",pos.getZ()); }
        return call(request);
    }

    private static CompoundTag decode(String data) throws IOException {
        byte[] bytes = Base64.getDecoder().decode(data);
        if (bytes.length>12*1024*1024) throw new IOException("Block entity NBT exceeds 12 MiB");
        return NbtIo.read(new DataInputStream(new ByteArrayInputStream(bytes)),NbtAccounter.create(12*1024*1024));
    }
    private static String encode(CompoundTag tag) throws IOException {
        var bytes = new ByteArrayOutputStream();
        NbtIo.write(tag,new DataOutputStream(bytes));
        return Base64.getEncoder().encodeToString(bytes.toByteArray());
    }

    private Entry entity(BlockPos pos, BlockState state, String saved) throws Exception {
        var old = entities.get(pos);
        if (old != null && old.state.getBlock()==state.getBlock()) {
            old.be.setBlockState(state);
            old.state=state;
            if (!saved.equals(old.saved) && !saved.isEmpty()) {
                old.be.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING,REGISTRIES,decode(saved)));
            }
            return old;
        }
        if (old != null) old.be.setRemoved();
        if (!(state.getBlock() instanceof EntityBlock block)) { entities.remove(pos); return null; }
        var be = block.newBlockEntity(pos,state);
        if (be == null) { entities.remove(pos); return null; }
        be.setLevel(this);
        if (!saved.isEmpty()) be.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING,REGISTRIES,decode(saved)));
        var entry = new Entry(be,state,saved);
        entities.put(pos.immutable(),entry);
        be.clearRemoved();
        be.onLoad();
        return entry;
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
            levelLoaded(this);
            if (System.getenv("FERRUM_AUDIT") != null) audit(this,Level.class);
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
        level.configure(batch.get("minY").getAsInt(),batch.get("height").getAsInt(),batch.get("dimension").getAsString());
        var active = new LinkedHashSet<BlockPos>();
        try {
            for (var element : batch.getAsJsonArray("entities")) {
                var data=element.getAsJsonObject();
                var pos = new BlockPos(data.get("x").getAsInt(),data.get("y").getAsInt(),data.get("z").getAsInt());
                active.add(pos);
                level.entity(pos,Block.stateById(data.get("state").getAsInt()),data.get("nbt").getAsString());
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
                catch (RuntimeException error) { LOGGER.error("Block entity tick failed at {}",pos,error); }
            }
            var result = new JsonArray();
            for (var entry : level.entities.entrySet()) {
                var pos=entry.getKey();
                var be=entry.getValue();
                if (level.getBlockState(pos).getBlock()!=be.state.getBlock()) continue;
                be.saved=encode(be.be.saveWithFullMetadata(REGISTRIES));
                var data=new JsonObject();
                data.addProperty("x",pos.getX()); data.addProperty("y",pos.getY()); data.addProperty("z",pos.getZ());
                data.addProperty("nbt",be.saved);
                result.add(data);
            }
            var reply = new JsonObject();
            reply.add("entities",result);
            reply.add("packets",Outbox.drain());
            level.chunks.saveDataSometimes();
            return reply.toString();
        } catch (Exception | Error error) { level.entities.clear(); throw error; }
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
    @Override public long getGameTime() { return request("time",null).getAsLong(); }
    @Override public RandomSource getRandom() { return rustRandom; }
    @Override public int getMinY() { return minimumY; }
    @Override public int getHeight() { return levelHeight; }
    @Override public int getSeaLevel() { return request("seaLevel",null).getAsInt(); }
    @Override public RegistryAccess registryAccess() { return REGISTRIES; }
    @Override public net.minecraft.world.item.crafting.RecipeManager recipeAccess() { return BridgeContent.recipes; }
    @Override public FuelValues fuelValues() { return BridgeContent.fuels; }
    @Override public net.minecraft.world.flag.FeatureFlagSet enabledFeatures() { return net.minecraft.world.level.WorldDataConfiguration.DEFAULT.enabledFeatures(); }
    @Override public BlockState getBlockState(BlockPos pos) { return Block.stateById(request("get",pos).getAsInt()); }
    @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    @Override public boolean setBlock(BlockPos pos, BlockState state,int flags,int recursion) {
        var data=new JsonObject(); data.addProperty("op","set");
        data.addProperty("x",pos.getX()); data.addProperty("y",pos.getY()); data.addProperty("z",pos.getZ());
        data.addProperty("state",Block.getId(state)); data.addProperty("flags",flags);
        return JsonParser.parseString(BridgeRuntime.world(data.toString())).getAsBoolean();
    }
    @Override public boolean setBlock(BlockPos pos,BlockState state,int flags) { return setBlock(pos,state,flags,512); }
    @Override public BlockEntity getBlockEntity(BlockPos pos) {
        var state=getBlockState(pos);
        var existing=entities.get(pos);
        if (existing != null && existing.state.getBlock()==state.getBlock()) return existing.be;
        var nbt=request("be",pos);
        if (nbt.isJsonNull()) return null;
        try { var entry=entity(pos,state,nbt.getAsString()); return entry==null ? null : entry.be; }
        catch(Exception error) { throw new IllegalStateException("Cannot load Rust block entity at "+pos,error); }
    }
    @Override public void blockEntityChanged(BlockPos pos) { request("dirty",pos); }
    @Override public void updateNeighbourForOutputSignal(BlockPos pos,Block block) { request("comparator",pos); }
    @Override public void sendBlockUpdated(BlockPos pos,BlockState old,BlockState state,int flags) { request("update",pos); }
    @Override public boolean hasChunk(int x,int z) { return request("loaded",new BlockPos(x<<4,minimumY,z<<4)).getAsBoolean(); }
    @Override public LevelChunk getChunk(int x,int z) { return RustChunk.create(this,x,z); }
    @Override public ServerChunkCache getChunkSource() { return chunks; }
    @Override public ChunkAccess getChunk(int x,int z,ChunkStatus status,boolean create) { return hasChunk(x,z) ? getChunk(x,z) : null; }
    @Override public <T,C> T getCapability(BlockCapability<T,C> capability,BlockPos pos,C context) {
        return capability.getCapability(this,pos,getBlockState(pos),getBlockEntity(pos),context);
    }
    @Override public <T,C> T getCapability(BlockCapability<T,C> capability,BlockPos pos,BlockState state,BlockEntity be,C context) {
        return capability.getCapability(this,pos,state==null ? getBlockState(pos) : state,be==null ? getBlockEntity(pos) : be,context);
    }

    private static final class Entry {
        final BlockEntity be; BlockState state; String saved;
        Entry(BlockEntity be,BlockState state,String saved) { this.be=be; this.state=state; this.saved=saved; }
    }
    private LevelChunkSection[] sections;
    private LevelChunkSection[] sections() {
        int count = levelHeight >> 4;
        if (sections == null || sections.length != count) {
            try {
                sections = new LevelChunkSection[count];
                for (int i = 0; i < count; i++) sections[i] = (LevelChunkSection) UNSAFE.allocateInstance(RustSection.class);
            } catch (InstantiationException error) { throw new IllegalStateException(error); }
        }
        return sections;
    }
    private static final class RustSection extends LevelChunkSection {
        private RustSection() { super(null,null); }
        @Override public boolean hasFluid() { return true; }
        @Override public boolean hasOnlyAir() { return false; }
    }
    private static final class RustChunk extends LevelChunk {
        private RustLevel owner;
        private ChunkPos position;
        private RustChunk(RustLevel level,int x,int z) { super(level,new ChunkPos(x,z)); }
        static RustChunk create(RustLevel level,int x,int z) {
            try { var chunk=(RustChunk)UNSAFE.allocateInstance(RustChunk.class); chunk.owner=level; chunk.position=new ChunkPos(x,z); return chunk; }
            catch(InstantiationException error) { throw new IllegalStateException(error); }
        }
        @Override public Level getLevel() { return owner; }
        @Override public ChunkPos getPos() { return position; }
        @Override public int getMinY() { return owner.minimumY; }
        @Override public int getHeight() { return owner.levelHeight; }
        // Sections are not stored; each reports fluid so callers fall back to per-block queries through the Rust world.
        @Override public LevelChunkSection[] getSections() { return owner.sections(); }
        @Override public Map<BlockPos,BlockEntity> getBlockEntities() {
            var result = new LinkedHashMap<BlockPos,BlockEntity>();
            owner.entities.forEach((pos,entry) -> {
                if ((pos.getX()>>4)==position.x() && (pos.getZ()>>4)==position.z()) result.put(pos,entry.be);
            });
            return Collections.unmodifiableMap(result);
        }
        @Override public BlockState getBlockState(BlockPos pos) { return owner.getBlockState(pos); }
        @Override public BlockState setBlockState(BlockPos pos,BlockState state,int flags) { var old=getBlockState(pos); owner.setBlock(pos,state,flags); return old; }
        @Override public BlockEntity getBlockEntity(BlockPos pos) { return owner.getBlockEntity(pos); }
        @Override public BlockEntity getBlockEntity(BlockPos pos,EntityCreationType type) { return getBlockEntity(pos); }
        @Override public void markUnsaved() { request("dirty",new BlockPos(position.x()<<4,owner.minimumY,position.z()<<4)); }
        @Override public net.minecraft.server.level.FullChunkStatus getFullStatus() { return net.minecraft.server.level.FullChunkStatus.ENTITY_TICKING; }
    }
    /** Entity broadcasts go to Rust's tracker, which knows who sees the entity. */
    private static final class RustChunkMap extends net.minecraft.server.level.ChunkMap {
        private RustChunkMap() { super(null,null,null,null,null,null,null,null,null,null,null,0,false); }
        @Override public void sendToTrackingPlayers(net.minecraft.world.entity.Entity entity,net.minecraft.network.protocol.Packet<? super net.minecraft.network.protocol.game.ClientGamePacketListener> packet) { Outbox.toTracking(entity.getId(),false,packet); }
        @Override public void sendToTrackingPlayersAndSelf(net.minecraft.world.entity.Entity entity,net.minecraft.network.protocol.Packet<? super net.minecraft.network.protocol.game.ClientGamePacketListener> packet) { Outbox.toTracking(entity.getId(),true,packet); }
    }
    private static final class RustChunkSource extends ServerChunkCache {
        private RustLevel owner;
        private RustChunkSource(RustLevel level) { super(level,null,null,null,Runnable::run,null,0,0,false,null,()->null); }
        @Override public LevelChunk getChunkNow(int x,int z) { return owner.hasChunk(x,z) ? owner.getChunk(x,z) : null; }
        @Override public ChunkAccess getChunk(int x,int z,ChunkStatus status,boolean create) { return owner.getChunk(x,z,status,create); }
        @Override public boolean hasChunk(int x,int z) { return owner.hasChunk(x,z); }
        @Override public Level getLevel() { return owner; }
        @Override public void blockChanged(BlockPos pos) { request("dirty",pos); }
        @Override public void sendToTrackingPlayers(net.minecraft.world.entity.Entity entity,net.minecraft.network.protocol.Packet<? super net.minecraft.network.protocol.game.ClientGamePacketListener> packet) { Outbox.toTracking(entity.getId(),false,packet); }
        @Override public void sendToTrackingPlayersAndSelf(net.minecraft.world.entity.Entity entity,net.minecraft.network.protocol.Packet<? super net.minecraft.network.protocol.game.ClientGamePacketListener> packet) { Outbox.toTracking(entity.getId(),true,packet); }
        private net.minecraft.world.level.storage.SavedDataStorage dataStorage;
        private int saveCountdown = SAVE_INTERVAL_BATCHES;
        /** Mod saved data (not blocks or entities) is the one thing Java persists; Rust owns the world itself. */
        @Override public synchronized net.minecraft.world.level.storage.SavedDataStorage getDataStorage() {
            if (dataStorage == null) {
                var folder = net.neoforged.fml.loading.FMLPaths.GAMEDIR.get().resolve("ferrum-saveddata").resolve(owner.dimensionName.replace(':','_')).resolve("data");
                dataStorage = new net.minecraft.world.level.storage.SavedDataStorage(owner,folder,net.minecraft.util.datafix.DataFixers.getDataFixer(),REGISTRIES);
            }
            return dataStorage;
        }
        synchronized void saveData() { if (dataStorage != null) dataStorage.saveAndJoin(); }
        synchronized void saveDataSometimes() {
            if (dataStorage != null && --saveCountdown <= 0) { saveCountdown = SAVE_INTERVAL_BATCHES; dataStorage.scheduleSave(); }
        }
        @Override public void tick(java.util.function.BooleanSupplier time,boolean chunks) { throw new UnsupportedOperationException("Rust owns chunk ticking"); }
        @Override public void save(boolean flush) { throw new UnsupportedOperationException("Rust owns chunk persistence"); }
    }
}

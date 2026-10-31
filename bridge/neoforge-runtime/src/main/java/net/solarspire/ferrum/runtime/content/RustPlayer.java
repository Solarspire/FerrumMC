package net.solarspire.ferrum.runtime.content;

import com.google.common.collect.ImmutableList;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.mojang.authlib.GameProfile;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFutureListener;
import java.lang.reflect.Modifier;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Base64;
import java.util.EnumMap;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Optional;
import java.util.UUID;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.PacketFlow;
import net.minecraft.network.protocol.common.ClientboundCustomPayloadPacket;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.ServerPlayerGameMode;
import net.minecraft.server.network.ServerGamePacketListenerImpl;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.ai.attributes.AttributeMap;
import net.minecraft.world.entity.ai.attributes.DefaultAttributes;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.inventory.InventoryMenu;
import net.minecraft.world.inventory.PlayerEnderChestContainer;
import net.minecraft.world.item.ItemCooldowns;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.entity.EntityInLevelCallback;
import net.minecraft.world.phys.Vec3;

/**
 * A ServerPlayer view of a Rust player. No constructor, network stack, advancement or save state exists:
 * Rust owns identity, position and inventory and pushes them in before mod code runs.
 */
final class RustPlayer extends ServerPlayer {
    private static final sun.misc.Unsafe UNSAFE;
    private static final java.lang.invoke.MethodHandle PLAYER_TICK;

    static {
        try {
            var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
            field.setAccessible(true);
            UNSAFE = (sun.misc.Unsafe) field.get(null);
            PLAYER_TICK = java.lang.invoke.MethodHandles.privateLookupIn(ServerPlayer.class, java.lang.invoke.MethodHandles.lookup())
                .findSpecial(Player.class, "tick", java.lang.invoke.MethodType.methodType(void.class), ServerPlayer.class);
        } catch (ReflectiveOperationException error) { throw new ExceptionInInitializerError(error); }
    }

    private GameType rustGameType = GameType.SURVIVAL;
    /** The stacks Java held after the last step, by slot; see PlayerBridge.sync. */
    ItemStack[] retained;
    private RustConnection rustConnection;
    boolean joined;
    /** Whether this view holds Rust's whole inventory, so syncs may send only the slots that changed. */
    boolean inventoryBaseline;
    /** Copies of the stacks Rust holds as of the last sync; a view stack that differs changed outside a sync. */
    ItemStack[] synced;
    boolean wasDead;
    JsonObject pendingImpulse;

    private RustPlayer() { super(null, null, null, null); }

    /** Stands in for Entity's constructor: identity, position state and the shared synched data of an allocated instance. */
    static void initEntity(net.minecraft.world.entity.Entity player, net.minecraft.world.entity.EntityType<?> type, UUID uuid, int entityId, RustLevel level) throws Exception {
        fill(player);
        set(player, net.minecraft.world.entity.Entity.class, "type", type);
        set(player, net.minecraft.world.entity.Entity.class, "id", entityId);
        set(player, net.minecraft.world.entity.Entity.class, "level", level);
        set(player, net.minecraft.world.entity.Entity.class, "uuid", uuid);
        set(player, net.minecraft.world.entity.Entity.class, "stringUUID", uuid.toString());
        set(player, net.minecraft.world.entity.Entity.class, "random", net.minecraft.util.RandomSource.create());
        set(player, net.minecraft.world.entity.Entity.class, "passengers", ImmutableList.of());
        set(player, net.minecraft.world.entity.Entity.class, "levelCallback", EntityInLevelCallback.NULL);
        set(player, net.minecraft.world.entity.Entity.class, "position", Vec3.ZERO);
        set(player, net.minecraft.world.entity.Entity.class, "deltaMovement", Vec3.ZERO);
        set(player, net.minecraft.world.entity.Entity.class, "stuckSpeedMultiplier", Vec3.ZERO);
        set(player, net.minecraft.world.entity.Entity.class, "lastKnownSpeed", Vec3.ZERO);
        set(player, net.minecraft.world.entity.Entity.class, "blockPosition", net.minecraft.core.BlockPos.ZERO);
        set(player, net.minecraft.world.entity.Entity.class, "chunkPosition", net.minecraft.world.level.ChunkPos.ZERO);
        set(player, net.minecraft.world.entity.Entity.class, "pistonDeltas", new double[] {0, 0, 0});
        var dimensions = type.getDimensions();
        set(player, net.minecraft.world.entity.Entity.class, "dimensions", dimensions);
        set(player, net.minecraft.world.entity.Entity.class, "eyeHeight", dimensions.eyeHeight());
        set(player, net.minecraft.world.entity.Entity.class, "customData", net.minecraft.world.item.component.CustomData.EMPTY);
        set(player, net.minecraft.world.entity.Entity.class, "bb", new net.minecraft.world.phys.AABB(0, 0, 0, 0, 0, 0));
        var builder = new SynchedEntityData.Builder(player);
        // Entity's constructor defines these before the subclass hook runs.
        builder.define(shared("DATA_SHARED_FLAGS_ID"), (byte) 0);
        builder.define(shared("DATA_AIR_SUPPLY_ID"), 300);
        builder.define(shared("DATA_CUSTOM_NAME_VISIBLE"), false);
        builder.define(shared("DATA_CUSTOM_NAME"), Optional.empty());
        builder.define(shared("DATA_SILENT"), false);
        builder.define(shared("DATA_NO_GRAVITY"), false);
        builder.define(shared("DATA_POSE"), net.minecraft.world.entity.Pose.STANDING);
        builder.define(shared("DATA_TICKS_FROZEN"), 0);
        var define = net.minecraft.world.entity.Entity.class.getDeclaredMethod("defineSynchedData", SynchedEntityData.Builder.class);
        define.setAccessible(true);
        define.invoke(player, builder);
        set(player, net.minecraft.world.entity.Entity.class, "entityData", builder.build());
    }

    static RustPlayer create(UUID uuid, String name, RustLevel level, int entityId) throws Exception {
        var player = (RustPlayer) UNSAFE.allocateInstance(RustPlayer.class);
        initEntity(player, net.minecraft.world.entity.EntityTypes.PLAYER, uuid, entityId, level);
        set(player, net.minecraft.world.entity.LivingEntity.class, "attributes", new AttributeMap(DefaultAttributes.getSupplier(net.minecraft.world.entity.EntityTypes.PLAYER)));
        set(player, net.minecraft.world.entity.LivingEntity.class, "combatTracker", new net.minecraft.world.damagesource.CombatTracker(player));
        set(player, net.minecraft.world.entity.LivingEntity.class, "walkAnimation", new net.minecraft.world.entity.WalkAnimationState());
        set(player, net.minecraft.world.entity.LivingEntity.class, "elytraAnimationState", new net.minecraft.world.entity.ElytraAnimationState(player));
        set(player, net.minecraft.world.entity.Entity.class, "fluidInteraction", new net.minecraft.world.entity.EntityFluidInteraction(java.util.Set.of()));
        // Mod hooks on collectEquipmentChanges read this map for every slot, as vanilla's constructor guarantees.
        var lastEquipment = new EnumMap<EquipmentSlot, ItemStack>(EquipmentSlot.class);
        for (var slot : EquipmentSlot.values()) lastEquipment.put(slot, ItemStack.EMPTY);
        set(player, net.minecraft.world.entity.LivingEntity.class, "lastEquipmentItems", lastEquipment);
        var equipment = new net.minecraft.world.entity.player.PlayerEquipment(player);
        set(player, net.minecraft.world.entity.LivingEntity.class, "equipment", equipment);
        var inventory = new Inventory(player, equipment);
        set(player, Player.class, "inventory", inventory);
        set(player, Player.class, "abilities", new Abilities());
        set(player, Player.class, "foodData", new FoodData());
        // The server variant tells the client about cooldowns the mod's item sets.
        set(player, Player.class, "cooldowns", new net.minecraft.world.item.ServerItemCooldowns(player));
        set(player, Player.class, "enderChestInventory", new PlayerEnderChestContainer());
        set(player, Player.class, "gameProfile", new GameProfile(uuid, name));
        set(player, Player.class, "inventoryMenu", new InventoryMenu(inventory, true, player));
        player.containerMenu = player.inventoryMenu;
        var mode = (ServerPlayerGameMode) UNSAFE.allocateInstance(ServerPlayerGameMode.class);
        set(mode, ServerPlayerGameMode.class, "player", player);
        set(mode, ServerPlayerGameMode.class, "level", level);
        set(player, ServerPlayer.class, "gameMode", mode);
        set(player, ServerPlayer.class, "server", RustLevel.standInServer());
        player.retained = new ItemStack[41];
        initMixinFields(player);
        // Mods read statistics (HBM boss spawning); Rust owns them, so the Java counter stays empty.
        var stats = (net.minecraft.stats.ServerStatsCounter) UNSAFE.allocateInstance(net.minecraft.stats.ServerStatsCounter.class);
        set(stats, net.minecraft.stats.StatsCounter.class, "stats", new it.unimi.dsi.fastutil.objects.Object2IntOpenHashMap<net.minecraft.stats.Stat<?>>());
        set(stats, net.minecraft.stats.ServerStatsCounter.class, "dirty", new java.util.HashSet<net.minecraft.stats.Stat<?>>());
        set(player, ServerPlayer.class, "stats", stats);
        set(player, ServerPlayer.class, "chunkTrackingView", net.minecraft.server.level.ChunkTrackingView.EMPTY);
        set(player, ServerPlayer.class, "advancements", RustAdvancements.create(player));
        // ServerPlayer's constructor starts it at zero; sweep and crit checks read it.
        set(player, ServerPlayer.class, "lastKnownClientMovement", Vec3.ZERO);
        // The menu sync machinery is made by ServerPlayer's constructor; its packets go out through the connection.
        var synchronizer = Class.forName("net.minecraft.server.level.ServerPlayer$1").getDeclaredConstructor(ServerPlayer.class);
        synchronizer.setAccessible(true);
        set(player, ServerPlayer.class, "containerSynchronizer", synchronizer.newInstance(player));
        // Rust owns advancement triggers and statistics, so the stock listener's criteria hooks have nothing to do here.
        set(player, ServerPlayer.class, "containerListener", new net.minecraft.world.inventory.ContainerListener() {
            @Override public void slotChanged(net.minecraft.world.inventory.AbstractContainerMenu menu, int slot, ItemStack stack) { }
            @Override public void dataChanged(net.minecraft.world.inventory.AbstractContainerMenu menu, int id, int value) { }
        });
        player.rustConnection = RustConnection.create(player);
        player.connection = player.rustConnection;
        return player;
    }

    RustConnection connection() { return rustConnection; }

    /** The player's NeoForge attachments as the player file stores them; Rust saves the player. */
    net.minecraft.nbt.CompoundTag saveAttachments() {
        var output = net.minecraft.world.level.storage.TagValueOutput.createWithContext(net.minecraft.util.ProblemReporter.DISCARDING, registryAccess());
        serializeAttachments(output);
        return output.buildResult();
    }

    void loadAttachments(net.minecraft.nbt.CompoundTag saved) {
        deserializeAttachments(net.minecraft.world.level.storage.TagValueInput.create(net.minecraft.util.ProblemReporter.DISCARDING, registryAccess(), saved));
    }

    /** Rust owns advancements: criteria triggers find no listeners here and awards do nothing. */
    private static final class RustAdvancements extends net.minecraft.server.PlayerAdvancements {
        private RustAdvancements() { super(null, null, null, null, null); }
        static RustAdvancements create(ServerPlayer player) throws Exception {
            var advancements = (RustAdvancements) UNSAFE.allocateInstance(RustAdvancements.class);
            var owner = net.minecraft.server.PlayerAdvancements.class;
            set(advancements, owner, "progress", new java.util.HashMap<>());
            set(advancements, owner, "visible", new java.util.HashSet<>());
            set(advancements, owner, "progressChanged", new java.util.HashSet<>());
            set(advancements, owner, "rootsToUpdate", new java.util.HashSet<>());
            set(advancements, owner, "activeTriggers", new java.util.HashMap<>());
            set(advancements, owner, "tree", new net.minecraft.advancements.AdvancementTree());
            set(advancements, owner, "player", player);
            return advancements;
        }
        @Override public boolean award(net.minecraft.advancements.AdvancementHolder advancement, String criterion) { return false; }
        @Override public boolean revoke(net.minecraft.advancements.AdvancementHolder advancement, String criterion) { return false; }
        @Override public void flushDirty(ServerPlayer player, boolean showAdvancements) { }
        @Override public void save() { }
        @Override public void setSelectedTab(net.minecraft.advancements.AdvancementHolder advancement) { }
    }

    private static final org.slf4j.Logger LOGGER = org.slf4j.LoggerFactory.getLogger(RustPlayer.class);
    private static final java.util.Set<String> TICK_FAILURES = java.util.concurrent.ConcurrentHashMap.newKeySet();
    private static final EquipmentSlot[] SLOT_EQUIPMENT = {
        EquipmentSlot.FEET, EquipmentSlot.LEGS, EquipmentSlot.CHEST, EquipmentSlot.HEAD, EquipmentSlot.OFFHAND };

    /**
     * Run the transformed Player/LivingEntity tick, bypassing ServerPlayer's network, food and advancement loop.
     * Items tick first and on their own so a failing mixin hook cannot stop gun or armor timers.
     */
    void tickMods() {
        // Right after login Rust may not have loaded the player's chunk yet; mod hooks read the blocks around them.
        if (!level().hasChunkAt(blockPosition())) return;
        tickCount++;
        tickInventory();
        try { PLAYER_TICK.invokeExact((ServerPlayer) this); }
        catch (Throwable error) { tickFailed("player tick", error); }
    }

    /** Logs the first failure of each kind with its stack and later ones as a count, so a broken hook cannot flood the log. */
    private void tickFailed(String stage, Throwable error) {
        if (error instanceof VirtualMachineError fatal) throw fatal;
        var key = stage + ":" + error;
        if (TICK_FAILURES.add(key)) LOGGER.error("Mod {} failed for {}", stage, getGameProfile().name(), error);
        else LOGGER.debug("Mod {} failed again for {}: {}", stage, getGameProfile().name(), error.toString());
    }

    private void tickInventory() {
        var inventory = getInventory();
        for (int slot = 0; slot <= 40; slot++) {
            var stack = inventory.getItem(slot);
            if (stack.isEmpty()) continue;
            EquipmentSlot equipment = slot < 36 ? (slot == inventory.getSelectedSlot() ? EquipmentSlot.MAINHAND : null) : SLOT_EQUIPMENT[slot - 36];
            try { stack.inventoryTick(level(), this, equipment); }
            catch (Throwable error) { tickFailed("inventory tick in slot " + slot, error); }
        }
    }

    void refreshEquipment() throws ReflectiveOperationException {
        var method = net.minecraft.world.entity.LivingEntity.class.getDeclaredMethod("detectEquipmentUpdates");
        method.setAccessible(true);
        method.invoke(this);
    }

    // Rust owns movement, effects and survival simulation. LivingEntity.tick still runs mixins and equipment hooks;
    // items were ticked by tickInventory.
    @Override public void baseTick() { }
    @Override public void aiStep() { }
    @Override protected boolean updateIsUnderwater() { return false; }
    @Override protected void updatePlayerPose() { }

    static Object allocate(Class<?> type) throws InstantiationException { return UNSAFE.allocateInstance(type); }

    /** Pulls Rust's position, rotation and game mode into the Java view before mod code runs. */
    void apply(JsonObject state) {
        // Entity.setPos would notify the level's waypoint manager, which Rust's level does not have.
        double x = state.get("x").getAsDouble(), y = state.get("y").getAsDouble(), z = state.get("z").getAsDouble();
        var position = new Vec3(x, y, z);
        var block = net.minecraft.core.BlockPos.containing(position);
        try {
            set(this, net.minecraft.world.entity.Entity.class, "position", position);
            set(this, net.minecraft.world.entity.Entity.class, "blockPosition", block);
            set(this, net.minecraft.world.entity.Entity.class, "chunkPosition", new net.minecraft.world.level.ChunkPos(block.getX() >> 4, block.getZ() >> 4));
            set(this, net.minecraft.world.entity.Entity.class, "bb", getDimensions(net.minecraft.world.entity.Pose.STANDING).makeBoundingBox(position));
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException(error);
        }
        setYRot(state.get("yaw").getAsFloat());
        setXRot(state.get("pitch").getAsFloat());
        setYHeadRot(state.get("yaw").getAsFloat());
        rustGameType = GameType.byId(state.get("gamemode").getAsInt());
        if (state.has("abilities")) {
            // Rust owns abilities; Java's copy mirrors them so mods read the real mode and flight state.
            var rust = state.getAsJsonObject("abilities");
            var abilities = getAbilities();
            abilities.invulnerable = rust.get("invulnerable").getAsBoolean();
            abilities.flying = rust.get("flying").getAsBoolean();
            abilities.mayfly = rust.get("mayfly").getAsBoolean();
            abilities.instabuild = rust.get("instabuild").getAsBoolean();
            abilities.mayBuild = rust.get("mayBuild").getAsBoolean();
            abilities.setFlyingSpeed(rust.get("flySpeed").getAsFloat());
            abilities.setWalkingSpeed(rust.get("walkSpeed").getAsFloat());
        }
        if (state.has("health")) setHealth(state.get("health").getAsFloat());
        if (state.has("fallDistance")) fallDistance = state.get("fallDistance").getAsFloat();
        if (state.has("ground")) setOnGround(state.get("ground").getAsBoolean());
        if (state.has("vx")) {
            try {
                var motion = new Vec3(state.get("vx").getAsDouble(),state.get("vy").getAsDouble(),state.get("vz").getAsDouble());
                set(this,net.minecraft.world.entity.Entity.class,"deltaMovement",motion);
                // The client reports its own movement; Rust's velocity is the closest Java sees.
                setKnownMovement(motion);
            }
            catch (ReflectiveOperationException error) { throw new IllegalStateException(error); }
        }
        if (state.has("attackTicks")) {
            try { set(this,net.minecraft.world.entity.LivingEntity.class,"attackStrengthTicker",state.get("attackTicks").getAsInt()); }
            catch (ReflectiveOperationException error) { throw new IllegalStateException(error); }
        }
        if (state.has("sneaking")) setShiftKeyDown(state.get("sneaking").getAsBoolean());
        try {
            set(gameMode, ServerPlayerGameMode.class, "gameModeForPlayer", rustGameType);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException(error);
        }
    }

    @Override public boolean isCreative() { return rustGameType == GameType.CREATIVE; }
    @Override public boolean isSpectator() { return rustGameType == GameType.SPECTATOR; }
    // Rust owns statistics; the Java view has no counters.
    @Override public void awardStat(net.minecraft.stats.Stat<?> stat, int count) { }
    @Override public void causeFoodExhaustion(float amount) { }
    @Override public void setIgnoreFallDamageFromCurrentImpulse(boolean ignore, Vec3 position) {
        super.setIgnoreFallDamageFromCurrentImpulse(ignore,position);
        pendingImpulse = new JsonObject();
        pendingImpulse.addProperty("ignore",ignore);
        pendingImpulse.addProperty("y",position.y);
        pendingImpulse.addProperty("grace",40);
    }
    @Override public boolean hurtServer(net.minecraft.server.level.ServerLevel level, net.minecraft.world.damagesource.DamageSource source, float amount) { return EntityBridge.damage(this, source, amount); }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static net.minecraft.network.syncher.EntityDataAccessor shared(String name) throws ReflectiveOperationException {
        var field = net.minecraft.world.entity.Entity.class.getDeclaredField(name);
        field.setAccessible(true);
        return (net.minecraft.network.syncher.EntityDataAccessor) field.get(null);
    }

    static void set(Object target, Class<?> owner, String name, Object value) throws ReflectiveOperationException {
        var field = owner.getDeclaredField(name);
        long offset = UNSAFE.objectFieldOffset(field);
        if (field.getType() == float.class) UNSAFE.putFloat(target, offset, ((Number) value).floatValue());
        else if (field.getType() == int.class) UNSAFE.putInt(target, offset, ((Number) value).intValue());
        else if (field.getType() == boolean.class) UNSAFE.putBoolean(target, offset, (Boolean) value);
        else if (field.getType() == long.class) UNSAFE.putLong(target, offset, ((Number) value).longValue());
        else if (field.getType() == double.class) UNSAFE.putDouble(target, offset, ((Number) value).doubleValue());
        else UNSAFE.putObject(target, offset, value);
    }

    /**
     * Field initializers that mods mixed into LivingEntity's constructor (HBM armor mods and knockback scale);
     * allocation skips them and the hooks read the fields as non-null.
     */
    static void initMixinFields(Object entity) throws ReflectiveOperationException {
        for (Class<?> type = entity.getClass(); type != Object.class; type = type.getSuperclass()) {
            for (var field : type.getDeclaredFields()) {
                if (Modifier.isStatic(field.getModifiers()) || !field.getName().startsWith("hbm$")) continue;
                long offset = UNSAFE.objectFieldOffset(field);
                if (field.getType().getName().equals("com.hbm.items.armor.ArmorMods")) {
                    UNSAFE.putObject(entity, offset, field.getType().getField("EMPTY").get(null));
                } else if (field.getName().equals("hbm$knockbackMultiplier")) {
                    UNSAFE.putDouble(entity, offset, Double.NaN);
                }
            }
        }
    }

    /** Stands in for the constructors that did not run: empty collections and items, nothing else. */
    @SuppressWarnings({"rawtypes", "unchecked"})
    static void fill(Object player) throws ReflectiveOperationException {
        for (Class<?> type = player.getClass(); type != Object.class; type = type.getSuperclass()) {
            for (var field : type.getDeclaredFields()) {
                var kind = field.getType();
                if (Modifier.isStatic(field.getModifiers()) || kind.isPrimitive() || kind.isArray()) continue;
                Object value = null;
                if (kind == ItemStack.class) value = ItemStack.EMPTY;
                else if (kind == Optional.class) value = Optional.empty();
                else if (kind == EnumMap.class) value = new EnumMap(EquipmentSlot.class);
                else if (kind == java.util.Map.class) value = new HashMap();
                else if (kind == java.util.Set.class) value = new HashSet();
                else if (kind == java.util.List.class || kind == java.util.Collection.class) value = new ArrayList();
                else if (kind == java.util.Deque.class) value = new ArrayDeque();
                else if (!kind.isInterface() && !Modifier.isAbstract(kind.getModifiers()) && kind.getName().startsWith("java.util.")
                    || kind == net.minecraft.network.protocol.game.VecDeltaCodec.class) {
                    try {
                        var constructor = kind.getDeclaredConstructor();
                        constructor.setAccessible(true);
                        value = constructor.newInstance();
                    } catch (ReflectiveOperationException | RuntimeException ignored) { }
                }
                if (value != null) UNSAFE.putObject(player, UNSAFE.objectFieldOffset(field), value);
            }
        }
    }

    /** Collects packets mod code sends to this player: custom payloads keep their channel, vanilla packets go through the outbox. */
    static final class RustConnection extends ServerGamePacketListenerImpl {
        private io.netty.channel.embedded.EmbeddedChannel channel;

        private RustConnection() { super(null, null, null, null); }

        static RustConnection create(RustPlayer player) throws Exception {
            var listener = (RustConnection) UNSAFE.allocateInstance(RustConnection.class);
            var connection = new net.minecraft.network.Connection(PacketFlow.SERVERBOUND);
            // Mods (HBM) write encoded packets straight into the connection's channel; the channel keeps them for Rust.
            listener.channel = new io.netty.channel.embedded.EmbeddedChannel(connection);
            set(listener, net.minecraft.server.network.ServerCommonPacketListenerImpl.class, "connection", connection);
            set(listener, net.minecraft.server.network.ServerCommonPacketListenerImpl.class, "server", RustLevel.standInServer());
            set(listener, net.minecraft.server.network.ServerCommonPacketListenerImpl.class, "connectionType",
                net.neoforged.neoforge.network.connection.ConnectionType.NEOFORGE);
            listener.player = player;
            return listener;
        }

        /** Moves what mods wrote straight into the connection's channel into the ordered outbox. */
        void flush() {
            channel.runPendingTasks();
            Object message;
            while ((message = channel.readOutbound()) != null) {
                if (message instanceof Packet<?> packet) {
                    send(packet);
                } else if (message instanceof io.netty.buffer.ByteBuf raw) {
                    rawPayload(raw);
                } else {
                    PlayerBridge.dropped("channel message " + message.getClass().getName());
                }
            }
        }

        /** An encoded clientbound custom payload packet: packet id, channel, then the payload. */
        private void rawPayload(io.netty.buffer.ByteBuf raw) {
            try {
                var buffer = new net.minecraft.network.FriendlyByteBuf(raw);
                buffer.readVarInt();
                String channel = buffer.readUtf();
                var entry = new JsonObject();
                entry.addProperty("uuid", player.getUUID().toString());
                entry.addProperty("channel", channel);
                entry.addProperty("data", Base64.getEncoder().encodeToString(ByteBufUtil.getBytes(buffer)));
                Outbox.add(entry);
            } catch (RuntimeException error) {
                PlayerBridge.dropped("raw packet " + error);
            } finally {
                raw.release();
            }
        }

        @Override public void send(Packet<?> packet) {
            if (!(packet instanceof ClientboundCustomPayloadPacket custom)) {
                try { Outbox.toPlayer(player.getUUID(), packet); }
                catch (RuntimeException error) { PlayerBridge.dropped(packet.type() + ": " + error); }
                return;
            }
            var buffer = new RegistryFriendlyByteBuf(Unpooled.buffer(), BridgeContent.worldRegistries);
            try {
                ClientboundCustomPayloadPacket.GAMEPLAY_STREAM_CODEC.encode(buffer, custom);
                String channel = buffer.readUtf();
                var entry = new JsonObject();
                entry.addProperty("uuid", player.getUUID().toString());
                entry.addProperty("channel", channel);
                entry.addProperty("data", Base64.getEncoder().encodeToString(ByteBufUtil.getBytes(buffer)));
                Outbox.add(entry);
            } finally {
                buffer.release();
            }
        }

        @Override public void send(Packet<?> packet, ChannelFutureListener listener) { send(packet); }
        @Override public boolean hasChannel(net.minecraft.resources.Identifier id) { return true; }
        @Override public boolean hasChannel(net.minecraft.network.protocol.common.custom.CustomPacketPayload.Type<?> type) { return true; }
        @Override public boolean hasChannel(net.minecraft.network.protocol.common.custom.CustomPacketPayload payload) { return true; }
        @Override public void disconnect(net.minecraft.network.chat.Component reason) { PlayerBridge.dropped("disconnect: " + reason.getString()); }
    }
}

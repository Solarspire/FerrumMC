package net.solarspire.ferrum.runtime.content;

import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import io.netty.buffer.Unpooled;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Base64;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CompletableFuture;
import java.util.function.Supplier;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.RegistryOps;
import net.minecraft.nbt.NbtIo;
import net.minecraft.network.ConnectionProtocol;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.PacketFlow;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.network.ConfigurationTask;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.component.CustomData;
import net.minecraft.world.item.context.UseOnContext;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;
import net.neoforged.neoforge.network.handling.IPayloadContext;
import net.neoforged.neoforge.network.handling.IPayloadHandler;
import net.neoforged.neoforge.network.registration.NetworkRegistry;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Per-player view of a Rust player for NeoForge code: Rust sends a snapshot and events, Java runs mod handlers
 * on the calling (tick) thread against a {@link RustPlayer}, and returns packets plus inventory changes.
 * Rust stays authoritative: only stacks that differ from the snapshot come back.
 */
public final class PlayerBridge {
    private static final Logger LOGGER = LoggerFactory.getLogger(PlayerBridge.class);
    private static final Gson JSON = new Gson();
    private static final String MOD_COMPONENTS = "ferrum_mod_components";
    private static final int MAIN_SLOTS = 36;
    private static final int OFF_HAND_SLOT = 40;
    private static final int[] SLOTS = slots();
    private static final Map<UUID, RustPlayer> PLAYERS = new ConcurrentHashMap<>();
    private static final java.util.Set<String> DROPPED = ConcurrentHashMap.newKeySet();

    private PlayerBridge() {}

    private static int[] slots() {
        int[] slots = new int[MAIN_SLOTS + 1];
        for (int i = 0; i < MAIN_SLOTS; i++) slots[i] = i;
        slots[MAIN_SLOTS] = OFF_HAND_SLOT;
        return slots;
    }

    /** Stand-in for the server's player list: mod code broadcasts through it, Rust delivers. */
    static net.minecraft.server.players.PlayerList playerList() throws Exception {
        return (net.minecraft.server.players.PlayerList) RustPlayer.allocate(RustPlayerList.class);
    }

    static List<ServerPlayer> all() { return new ArrayList<>(PLAYERS.values()); }

    /** Raw packets mods wrote into player channels move into the outbox, in the order mod code wrote them. */
    static void flushChannels() {
        for (var player : PLAYERS.values()) player.connection().flush();
    }

    private static final class RustPlayerList extends net.minecraft.server.dedicated.DedicatedPlayerList {
        private RustPlayerList() { super(null, null, null); }

        @Override public List<ServerPlayer> getPlayers() { return all(); }

        @Override public void broadcastAll(net.minecraft.network.protocol.Packet<?> packet) {
            for (var player : PLAYERS.values()) player.connection().send(packet);
        }

        @Override public void broadcastAll(net.minecraft.network.protocol.Packet<?> packet,
            net.minecraft.resources.ResourceKey<net.minecraft.world.level.Level> dimension) { broadcastAll(packet); }

        @Override public void broadcast(net.minecraft.world.entity.player.Player except, double x, double y, double z, double radius,
            net.minecraft.resources.ResourceKey<net.minecraft.world.level.Level> dimension, net.minecraft.network.protocol.Packet<?> packet) {
            for (var player : PLAYERS.values()) {
                if (player == except) continue;
                double dx = x - player.getX(), dy = y - player.getY(), dz = z - player.getZ();
                if (dx * dx + dy * dy + dz * dz < radius * radius) player.connection().send(packet);
            }
        }
    }

    /** A player's Java view built from a Rust entity snapshot, for level queries outside that player's own events. */
    static RustPlayer view(RustLevel level, UUID uuid, JsonObject snapshot) throws Exception {
        var player = PLAYERS.get(uuid);
        if (player == null || player.level() != level) {
            player = RustPlayer.create(uuid, snapshot.get("name").getAsString(), level, snapshot.get("id").getAsInt());
            PLAYERS.put(uuid, player);
        }
        player.apply(snapshot);
        return player;
    }

    static List<ServerPlayer> playersIn(RustLevel level) {
        var result = new ArrayList<ServerPlayer>();
        for (var player : PLAYERS.values()) if (player.level() == level) result.add(player);
        return result;
    }

    public static String play(String input) throws Exception {
        var request = JSON.fromJson(input, JsonObject.class);
        var op = request.get("op").getAsString();
        if (op.equals("components_encode")) return Components.encode(decode(request.get("nbt").getAsString())).toString();
        if (op.equals("components_decode")) return "{\"nbt\":\"" + encode(Components.decode(request.getAsJsonArray("components"))) + "\"}";
        if (op.equals("tick_post")) {
            TrackingBridge.flushSync();
            var reply = new JsonObject();
            reply.add("packets", Outbox.drain());
            return reply.toString();
        }
        var id = request.has("uuid") ? UUID.fromString(request.get("uuid").getAsString()) : null;
        if (op.equals("leave")) {
            TrackingBridge.forget(id);
            PLAYERS.remove(id);
            return "{}";
        }
        var level = RustLevel.forWorld(request.get("world").getAsString());
        synchronized (level) {
            level.configure(request.get("minY").getAsInt(), request.get("height").getAsInt(), request.get("dimension").getAsString());
            if (op.equals("entities")) {
                var result = EntityBridge.tick(level, request);
                result.add("packets", Outbox.drain());
                return result.toString();
            }
            var player = PLAYERS.get(id);
            if (player == null || player.level() != level) {
                player = RustPlayer.create(id, request.get("name").getAsString(), level, request.get("entityId").getAsInt());
                PLAYERS.put(id, player);
            }
            return sync(player, request).toString();
        }
    }

    private static JsonObject sync(RustPlayer player, JsonObject request) throws Exception {
        var before = new ItemStack[OFF_HAND_SLOT + 1];
        player.apply(request);
        var inventory = player.getInventory();
        for (int slot : SLOTS) inventory.setItem(slot, ItemStack.EMPTY);
        var snapshot = request.getAsJsonObject("inventory");
        for (var entry : snapshot.entrySet()) {
            int slot = Integer.parseInt(entry.getKey());
            var stack = stackFromRust(decode(entry.getValue().getAsString()));
            // Mods keep transient state on the stack object (HBM gun timers), so an unchanged stack must stay the same object.
            var kept = player.retained[slot];
            inventory.setItem(slot, kept != null && ItemStack.matches(kept, stack) ? kept : stack);
        }
        for (int slot : SLOTS) before[slot] = inventory.getItem(slot).copy();
        for (int slot : SLOTS) player.retained[slot] = inventory.getItem(slot);
        inventory.setSelectedSlot(request.get("selected").getAsInt());

        var results = new JsonArray();
        for (var element : request.getAsJsonArray("events")) {
            var event = element.getAsJsonObject();
            var result = new JsonObject();
            try {
                switch (event.get("type").getAsString()) {
                    case "payload" -> result.addProperty("handled", dispatch(player, event));
                    case "use" -> result.addProperty("consumed", use(player, hand(event)));
                    case "use_on" -> result.addProperty("consumed", useOn(player, event));
                    case "use_block" -> result.addProperty("consumed", useBlock(player, event));
                    case "menu_packet" -> MenuBridge.packet(player, event.get("data").getAsString());
                    case "menu_tick" -> MenuBridge.tick(player);
                    case "tracking" -> TrackingBridge.apply(player, event);
                    case "tick" -> {
                        player.tickCount++;
                        inventory.tick();
                    }
                    default -> throw new IllegalArgumentException("Unknown player event " + event);
                }
            } catch (RuntimeException | LinkageError error) {
                LOGGER.error("Mod player event {} failed for {}", event.get("type"), player.getGameProfile().name(), error);
                result.addProperty("error", error.toString());
            }
            results.add(result);
        }

        var reply = new JsonObject();
        var changes = new JsonObject();
        for (int slot : SLOTS) {
            var after = inventory.getItem(slot);
            if (!ItemStack.matches(before[slot], after)) changes.addProperty(Integer.toString(slot), encode(stackToRust(after)));
        }
        reply.add("inventory", changes);
        // Other players' packets (sounds, effects) leave in whichever step runs next; each entry names its target.
        reply.add("packets", Outbox.drain());
        reply.addProperty("menu", MenuBridge.openId(player));
        reply.add("results", results);
        return reply;
    }

    private static InteractionHand hand(JsonObject event) {
        return event.get("hand").getAsInt() == 0 ? InteractionHand.MAIN_HAND : InteractionHand.OFF_HAND;
    }

    /** Mirrors ServerPlayerGameMode.useItem for items Rust cannot run. */
    private static boolean use(RustPlayer player, InteractionHand hand) {
        var stack = player.getItemInHand(hand);
        if (stack.isEmpty() || player.getCooldowns().isOnCooldown(stack)) return false;
        InteractionResult result = stack.use(player.level(), player, hand);
        if (result instanceof InteractionResult.Success success && success.heldItemTransformedTo() != null) {
            player.setItemInHand(hand, success.heldItemTransformedTo());
        }
        return result.consumesAction();
    }

    /** Block items stay with Rust so modded and vanilla placement keep one code path. */
    private static boolean useOn(RustPlayer player, JsonObject event) {
        var hand = hand(event);
        var stack = player.getItemInHand(hand);
        if (stack.isEmpty() || stack.getItem() instanceof BlockItem || player.getCooldowns().isOnCooldown(stack)) return false;
        var hit = hit(event);
        return stack.useOn(new UseOnContext(player.level(), player, hand, stack, hit)).consumesAction();
    }

    private static BlockHitResult hit(JsonObject event) {
        var pos = new BlockPos(event.get("x").getAsInt(), event.get("y").getAsInt(), event.get("z").getAsInt());
        var cursor = new Vec3(pos.getX() + event.get("cx").getAsDouble(), pos.getY() + event.get("cy").getAsDouble(),
            pos.getZ() + event.get("cz").getAsDouble());
        return new BlockHitResult(cursor, Direction.from3DDataValue(event.get("face").getAsInt()), pos, event.get("inside").getAsBoolean());
    }

    /** Mirrors the block half of ServerPlayerGameMode.useItemOn: the block's useItemOn, then useWithoutItem for the main hand. */
    private static boolean useBlock(RustPlayer player, JsonObject event) {
        var hand = hand(event);
        var hit = hit(event);
        var level = player.level();
        var state = level.getBlockState(hit.getBlockPos());
        if (!state.getBlock().isEnabled(level.enabledFeatures())) return false;
        boolean holding = !player.getMainHandItem().isEmpty() || !player.getOffhandItem().isEmpty();
        if (player.isSecondaryUseActive() && holding) return false;
        var result = state.useItemOn(player.getItemInHand(hand), level, player, hand, hit);
        if (result.consumesAction()) return true;
        return result instanceof InteractionResult.TryEmptyHandInteraction && hand == InteractionHand.MAIN_HAND
            && state.useWithoutItem(level, player, hit).consumesAction();
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static boolean dispatch(RustPlayer player, JsonObject event) throws Exception {
        var id = Identifier.parse(event.get("channel").getAsString());
        StreamCodec codec = NetworkRegistry.getCodec(id, ConnectionProtocol.PLAY, PacketFlow.SERVERBOUND);
        var handlers = (Map<ConnectionProtocol, Map<Identifier, IPayloadHandler<?>>>) handlerTable();
        var handler = handlers.getOrDefault(ConnectionProtocol.PLAY, Map.of()).get(id);
        if (codec == null || handler == null) return false;
        var data = Base64.getDecoder().decode(event.get("data").getAsString());
        var buffer = new RegistryFriendlyByteBuf(Unpooled.wrappedBuffer(data), BridgeContent.worldRegistries);
        var payload = (CustomPacketPayload) codec.decode(buffer);
        if (buffer.isReadable()) throw new IllegalStateException("Payload " + id + " left " + buffer.readableBytes() + " unread bytes");
        ((IPayloadHandler) handler).handle(payload, new Context(player));
        return true;
    }

    private static Object handlerTable() throws ReflectiveOperationException {
        var field = NetworkRegistry.class.getDeclaredField("SERVERBOUND_HANDLERS");
        field.setAccessible(true);
        return field.get(null);
    }

    static void dropped(String what) {
        if (DROPPED.add(what)) LOGGER.warn("Bridged player dropped unsupported clientbound {}", what);
    }

    static CompoundTag decode(String data) throws IOException {
        byte[] bytes = Base64.getDecoder().decode(data);
        return NbtIo.read(new DataInputStream(new ByteArrayInputStream(bytes)), NbtAccounter.create(1024 * 1024));
    }

    static String encode(CompoundTag tag) throws IOException {
        var bytes = new ByteArrayOutputStream();
        NbtIo.write(tag, new DataOutputStream(bytes));
        return Base64.getEncoder().encodeToString(bytes.toByteArray());
    }

    /**
     * Rust's item stack NBT: id, count and components named like Minecraft's. Rust only models some components, so
     * mod-defined ones travel inside custom data under {@link #MOD_COMPONENTS} and persist with the stack.
     */
    @SuppressWarnings({"unchecked", "rawtypes"})
    private static ItemStack stackFromRust(CompoundTag tag) {
        var item = BuiltInRegistries.ITEM.getOptional(Identifier.parse(tag.getStringOr("id", "minecraft:air")));
        if (item.isEmpty()) return ItemStack.EMPTY;
        var stack = new ItemStack(item.get(), tag.getIntOr("count", 1));
        var components = tag.getCompoundOrEmpty("components");
        var custom = components.getCompound("minecraft:custom_data").or(() -> components.getCompound("custom_data"));
        if (custom.isPresent()) {
            var data = custom.get().copy();
            var mod = data.getCompoundOrEmpty(MOD_COMPONENTS);
            data.remove(MOD_COMPONENTS);
            if (!data.isEmpty()) stack.set(DataComponents.CUSTOM_DATA, CustomData.of(data));
            var ops = RegistryOps.create(NbtOps.INSTANCE, BridgeContent.worldRegistries);
            for (var name : mod.keySet()) {
                var type = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse(name));
                if (type == null || type.isTransient()) continue;
                var value = type.codecOrThrow().parse(ops, mod.get(name)).resultOrPartial(error -> LOGGER.warn("Bad {} data: {}", name, error));
                value.ifPresent(parsed -> stack.set((net.minecraft.core.component.DataComponentType) type, parsed));
            }
        }
        int damage = components.getIntOr("minecraft:damage", components.getIntOr("damage", 0));
        if (damage > 0 && stack.isDamageableItem()) stack.setDamageValue(damage);
        return stack;
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    static CompoundTag stackToRust(ItemStack stack) {
        var tag = new CompoundTag();
        if (stack.isEmpty()) {
            tag.putBoolean("empty", true);
            return tag;
        }
        tag.putString("id", BuiltInRegistries.ITEM.getKey(stack.getItem()).toString());
        tag.putInt("count", stack.getCount());
        var components = new CompoundTag();
        var custom = stack.get(DataComponents.CUSTOM_DATA);
        var data = custom == null ? new CompoundTag() : custom.copyTag();
        var mod = new CompoundTag();
        var ops = RegistryOps.create(NbtOps.INSTANCE, BridgeContent.worldRegistries);
        for (var entry : stack.getComponentsPatch().entrySet()) {
            var type = (net.minecraft.core.component.DataComponentType) entry.getKey();
            var key = BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(type);
            if (key == null || key.getNamespace().equals("minecraft") || type.isTransient() || entry.getValue().isEmpty()) continue;
            type.codecOrThrow().encodeStart(ops, entry.getValue().get()).resultOrPartial(error -> LOGGER.warn("Cannot store {}: {}", key, error))
                .ifPresent(encoded -> mod.put(key.toString(), (net.minecraft.nbt.Tag) encoded));
        }
        if (!mod.isEmpty()) data.put(MOD_COMPONENTS, mod);
        if (!data.isEmpty()) components.put("minecraft:custom_data", data);
        if (stack.getDamageValue() > 0) components.putInt("minecraft:damage", stack.getDamageValue());
        tag.put("components", components);
        return tag;
    }

    /** Payload handlers already run on the tick thread, so deferred work runs immediately. */
    private record Context(RustPlayer holder) implements IPayloadContext {
        @Override public net.neoforged.neoforge.common.extensions.ICommonPacketListener listener() { return holder.connection(); }
        @Override public net.minecraft.world.entity.player.Player player() { return holder; }
        @Override public CompletableFuture<Void> enqueueWork(Runnable task) {
            task.run();
            return CompletableFuture.completedFuture(null);
        }
        @Override public <T> CompletableFuture<T> enqueueWork(Supplier<T> task) { return CompletableFuture.completedFuture(task.get()); }
        @Override public PacketFlow flow() { return PacketFlow.SERVERBOUND; }
        @Override public void handle(CustomPacketPayload payload) { throw new UnsupportedOperationException("Nested payload dispatch"); }
        @Override public void finishCurrentTask(ConfigurationTask.Type type) { }
    }
}

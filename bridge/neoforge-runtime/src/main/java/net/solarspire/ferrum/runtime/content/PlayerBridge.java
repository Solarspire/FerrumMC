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
import net.minecraft.world.level.block.Block;
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
    /** Names of the vanilla prototype components of a mod item that Rust holds only for its own gameplay. */
    private static final String PROTOTYPE_COMPONENTS = "ferrum_prototype_components";
    private static final int MAIN_SLOTS = 36;
    private static final int OFF_HAND_SLOT = 40;
    private static final int[] SLOTS = slots();
    private static final Map<UUID, RustPlayer> PLAYERS = new ConcurrentHashMap<>();
    private static final java.util.Set<String> DROPPED = ConcurrentHashMap.newKeySet();

    private PlayerBridge() {}

    private static int[] slots() {
        int[] slots = new int[OFF_HAND_SLOT + 1];
        for (int i = 0; i <= OFF_HAND_SLOT; i++) slots[i] = i;
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
        @Override public Map<UUID, ServerPlayer> getPlayersByUUID() { return new java.util.HashMap<>(PLAYERS); }
        @Override public ServerPlayer getPlayer(UUID id) { return PLAYERS.get(id); }
        @Override public ServerPlayer getPlayer(String name) {
            for (var player : PLAYERS.values()) if (player.getGameProfile().name().equalsIgnoreCase(name)) return player;
            return null;
        }
        @Override public ServerPlayer getPlayerByName(String name) { return getPlayer(name); }
        @Override public int getPlayerCount() { return PLAYERS.size(); }
        @Override public net.minecraft.stats.ServerStatsCounter getPlayerStats(net.minecraft.world.entity.player.Player player) {
            return player instanceof ServerPlayer server ? server.getStats() : null;
        }

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
        if (op.equals("craft")) return craft(request).toString();
        if (op.equals("cook")) return cook(request).toString();
        if (op.equals("tick_post")) {
            SafeEvents.post(new net.neoforged.neoforge.event.tick.ServerTickEvent.Post(() -> false, RustLevel.standInServer()));
            var reply = new JsonObject();
            reply.add("packets", Outbox.drain());
            return reply.toString();
        }
        if (op.equals("tick_pre")) {
            SafeEvents.post(new net.neoforged.neoforge.event.tick.ServerTickEvent.Pre(() -> false, RustLevel.standInServer()));
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
            try { return playIn(level, op, id, request); }
            finally { level.flushVanilla(); }
        }
    }

    private static String playIn(RustLevel level, String op, UUID id, JsonObject request) throws Exception {
        {
            level.configure(request);
            if (op.equals("level_pre") || op.equals("level_post")) {
                if (op.equals("level_pre")) {
                    level.syncChunks(request.getAsJsonArray("chunks"));
                    SafeEvents.post(new net.neoforged.neoforge.event.tick.LevelTickEvent.Pre(() -> false, level));
                    level.runTickHooks(false);
                } else {
                    level.runTickHooks(true);
                    SafeEvents.post(new net.neoforged.neoforge.event.tick.LevelTickEvent.Post(() -> false, level));
                }
                var reply = new JsonObject();
                reply.add("packets", Outbox.drain());
                return reply.toString();
            }
            if (op.equals("entities")) {
                var result = EntityBridge.tick(level, request);
                result.add("packets", Outbox.drain());
                return result.toString();
            }
            if (op.equals("block_placed")) return level.blockPlaced(request, id == null ? null : PLAYERS.get(id)).toString();
            if (op.equals("correct_tool")) {
                var reply = new JsonObject();
                var tool = stackFromRust(decode(request.get("tool").getAsString()));
                var state = Block.stateById(request.get("state").getAsInt());
                reply.addProperty("correct", tool.isCorrectToolForDrops(state));
                reply.addProperty("speed", tool.getDestroySpeed(state));
                return reply.toString();
            }
            if (op.equals("block_drops")) return level.blockDrops(request).toString();
            if (op.equals("random_ticks")) {
                var reply = level.randomTicks(request);
                reply.add("packets",Outbox.drain());
                return reply.toString();
            }
            if (op.equals("block_removing") || op.equals("block_changed") || op.equals("neighbor_changed") || op.equals("scheduled_tick")) {
                var reply = switch (op) {
                    case "block_removing" -> level.blockRemoving(request);
                    case "block_changed" -> level.blockChanged(request);
                    case "neighbor_changed" -> level.neighborChanged(request);
                    default -> level.scheduledTick(request);
                };
                reply.add("packets",Outbox.drain());
                return reply.toString();
            }
            if (op.equals("living_event")) {
                var entity = level.getEntity(request.get("entity").getAsInt());
                var reply = entity instanceof net.minecraft.world.entity.LivingEntity living ? livingEvent(living, request) : new JsonObject();
                reply.add("packets", Outbox.drain());
                return reply.toString();
            }
            if (op.equals("world")) return "{}";
            if (op.equals("summon")) {
                var reply = new JsonObject();
                var type = BuiltInRegistries.ENTITY_TYPE.getOptional(net.minecraft.resources.Identifier.parse(request.get("type").getAsString()));
                var entity = type.isEmpty() ? null : type.get().create(level, net.minecraft.world.entity.EntitySpawnReason.COMMAND);
                if (entity != null) {
                    entity.snapTo(request.get("x").getAsDouble(), request.get("y").getAsDouble(), request.get("z").getAsDouble());
                    if (level.addFreshEntity(entity)) reply.addProperty("id", entity.getId());
                }
                reply.add("packets", Outbox.drain());
                return reply.toString();
            }
            if (op.equals("hurt_entity")) {
                var reply = new JsonObject();
                var target = level.getEntity(request.get("entity").getAsInt());
                reply.addProperty("hurt", target != null && EntityBridge.isBridged(target)
                    && target.hurtServer(level, damageSource(level, request), request.get("amount").getAsFloat()));
                reply.add("packets", Outbox.drain());
                return reply.toString();
            }
            var player = PLAYERS.get(id);
            if (player == null) {
                player = RustPlayer.create(id, request.get("name").getAsString(), level, request.get("entityId").getAsInt());
                PLAYERS.put(id, player);
            } else if (player.level() != level) {
                var previous = player.level().dimension();
                RustPlayer.set(player,net.minecraft.world.entity.Entity.class,"level",level);
                RustPlayer.set(player.gameMode,net.minecraft.server.level.ServerPlayerGameMode.class,"level",level);
                SafeEvents.post(new net.neoforged.neoforge.event.entity.player.PlayerEvent.PlayerChangedDimensionEvent(player,previous,level.dimension()));
            }
            return sync(player, request).toString();
        }
    }

    /** Rust's damage on a bridged entity, with the damage type and entities Rust named. */
    private static net.minecraft.world.damagesource.DamageSource damageSource(RustLevel level, JsonObject request) {
        var types = level.registryAccess().lookupOrThrow(net.minecraft.core.registries.Registries.DAMAGE_TYPE);
        var name = request.get("damageType").getAsString();
        var type = types.get(net.minecraft.resources.ResourceKey.create(net.minecraft.core.registries.Registries.DAMAGE_TYPE, net.minecraft.resources.Identifier.parse(name)))
            .orElseGet(() -> types.getOrThrow(net.minecraft.world.damagesource.DamageTypes.GENERIC));
        int direct = request.get("direct").getAsInt(), attacker = request.get("attacker").getAsInt();
        return new net.minecraft.world.damagesource.DamageSource(type, direct < 0 ? null : level.getEntity(direct), attacker < 0 ? null : level.getEntity(attacker));
    }

    /** Recipe lookups need a level for matches(); recipes never reach world state, so any level view serves. */
    private static net.minecraft.world.level.Level recipeLevel() throws Exception {
        var levels = RustLevel.standInServer().getAllLevels().iterator();
        if (!levels.hasNext()) throw new IllegalStateException("No level for recipe lookups yet");
        return levels.next();
    }

    /** The mod crafting recipe for a grid: its result and the remainders by grid slot. */
    private static JsonObject craft(JsonObject request) throws Exception {
        int width = request.get("width").getAsInt(), height = request.get("height").getAsInt();
        var grid = request.getAsJsonArray("grid");
        if (width < 1 || height < 1 || width * height != grid.size() || grid.size() > 9) throw new IllegalArgumentException("Bad crafting grid");
        var items = new ArrayList<ItemStack>();
        for (var slot : grid) items.add(slot.getAsString().isEmpty() ? ItemStack.EMPTY : stackFromRust(decode(slot.getAsString())));
        var positioned = net.minecraft.world.item.crafting.CraftingInput.ofPositioned(width, height, items);
        var input = positioned.input();
        var reply = new JsonObject();
        var level = recipeLevel();
        var recipe = BridgeContent.recipes.getRecipeFor(net.minecraft.world.item.crafting.RecipeType.CRAFTING, input, level);
        if (recipe.isEmpty()) return reply;
        var result = recipe.get().value().assemble(input);
        if (result.isEmpty()) return reply;
        reply.addProperty("result", encode(stackToRust(result)));
        var remaining = recipe.get().value().getRemainingItems(input);
        var remainders = new JsonArray();
        for (int i = 0; i < width * height; i++) remainders.add("");
        for (int y = 0; y < input.height(); y++) for (int x = 0; x < input.width(); x++) {
            var stack = remaining.get(x + y * input.width());
            if (!stack.isEmpty()) remainders.set(x + positioned.left() + (y + positioned.top()) * width, new com.google.gson.JsonPrimitive(encode(stackToRust(stack))));
        }
        reply.add("remainders", remainders);
        return reply;
    }

    /** The mod cooking recipe of one furnace kind for an input item. */
    private static JsonObject cook(JsonObject request) throws Exception {
        var type = switch (request.get("kind").getAsString()) {
            case "blasting" -> net.minecraft.world.item.crafting.RecipeType.BLASTING;
            case "smoking" -> net.minecraft.world.item.crafting.RecipeType.SMOKING;
            case "campfire_cooking" -> net.minecraft.world.item.crafting.RecipeType.CAMPFIRE_COOKING;
            default -> net.minecraft.world.item.crafting.RecipeType.SMELTING;
        };
        var input = new net.minecraft.world.item.crafting.SingleRecipeInput(stackFromRust(decode(request.get("stack").getAsString())));
        var reply = new JsonObject();
        var recipe = BridgeContent.recipes.getRecipeFor((net.minecraft.world.item.crafting.RecipeType<net.minecraft.world.item.crafting.AbstractCookingRecipe>) (Object) type, input, recipeLevel());
        if (recipe.isEmpty()) return reply;
        var result = recipe.get().value().assemble(input);
        if (result.isEmpty()) return reply;
        reply.addProperty("id", recipe.get().id().identifier().toString());
        reply.addProperty("result", BuiltInRegistries.ITEM.getKey(result.getItem()).toString());
        reply.addProperty("count", result.getCount());
        reply.addProperty("time", recipe.get().value().cookingTime());
        reply.addProperty("experience", recipe.get().value().experience());
        return reply;
    }

    private static final int ATTACHMENT_SAVE_INTERVAL = 100;

    private static JsonObject sync(RustPlayer player, JsonObject request) throws Exception {
        var before = new ItemStack[OFF_HAND_SLOT + 1];
        player.apply(request);
        var inventory = player.getInventory();
        // Rust sends the slots that changed since the last sync, with "" for an emptied one, or all of them.
        var snapshot = request.getAsJsonObject("inventory");
        boolean full = request.get("full").getAsBoolean();
        if (!full && !player.inventoryBaseline) {
            // A view made for a level query has no inventory yet.
            var fetch = new JsonObject();
            fetch.addProperty("op", "player_inventory");
            fetch.addProperty("uuid", player.getUUID().toString());
            snapshot = ((RustLevel) player.level()).bridgeCall(fetch).getAsJsonObject();
            full = true;
        }
        if (full) for (int slot : SLOTS) if (!snapshot.has(Integer.toString(slot))) inventory.setItem(slot, ItemStack.EMPTY);
        var stale = new boolean[OFF_HAND_SLOT + 1];
        var sent = new boolean[OFF_HAND_SLOT + 1];
        if (full) java.util.Arrays.fill(sent, true);
        for (var entry : snapshot.entrySet()) {
            int slot = Integer.parseInt(entry.getKey());
            if (slot < 0 || slot > OFF_HAND_SLOT) throw new IllegalArgumentException("Rust sent inventory slot " + slot);
            sent[slot] = true;
            var data = entry.getValue().getAsString();
            if (data.isEmpty()) { inventory.setItem(slot, ItemStack.EMPTY); continue; }
            var tag = decode(data);
            var stack = stackFromRust(tag);
            // A mod item Rust made itself lacks the prototype values Rust gameplay reads, so Java sends them once.
            stale[slot] = !stack.isEmpty() && !BuiltInRegistries.ITEM.getKey(stack.getItem()).getNamespace().equals("minecraft")
                && !tag.getCompoundOrEmpty("components").getCompoundOrEmpty("minecraft:custom_data").contains(PROTOTYPE_COMPONENTS);
            // Mods keep transient state on the stack object (HBM gun timers), so an unchanged stack must stay the same object.
            var kept = player.retained[slot];
            inventory.setItem(slot, kept != null && ItemStack.matches(kept, stack) ? kept : stack);
        }
        if (player.synced == null) player.synced = new ItemStack[OFF_HAND_SLOT + 1];
        for (int slot : SLOTS) {
            if (sent[slot] || player.synced[slot] == null) player.synced[slot] = inventory.getItem(slot).copy();
            // Against Rust's copy, so a change made outside a sync (a level event) is reported too.
            before[slot] = player.synced[slot];
        }
        for (int slot : SLOTS) player.retained[slot] = inventory.getItem(slot);
        player.inventoryBaseline = true;
        inventory.setSelectedSlot(request.get("selected").getAsInt());
        if (request.has("attachments") && !player.joined) player.loadAttachments(decode(request.get("attachments").getAsString()));
        if (!player.joined) {
            player.joined = true;
            SafeEvents.post(new net.neoforged.neoforge.event.entity.player.PlayerEvent.PlayerLoggedInEvent(player));
        } else if (player.wasDead && player.getHealth() > 0) {
            SafeEvents.post(new net.neoforged.neoforge.event.entity.player.PlayerEvent.PlayerRespawnEvent(player,false));
        }
        player.wasDead = player.getHealth() <= 0;
        var motionBefore = player.getDeltaMovement();
        var results = new JsonArray();
        for (var element : request.getAsJsonArray("events")) {
            var event = element.getAsJsonObject();
            var result = new JsonObject();
            try {
                switch (event.get("type").getAsString()) {
                    case "hurt", "death", "drops" -> result = livingEvent(player, event);
                    case "attack" -> {
                        var target = player.level().getEntity(event.get("target").getAsInt());
                        if (target != null) {
                            player.refreshEquipment();
                            player.attack(target);
                        }
                    }
                    case "attack_event" -> {
                        var target = player.level().getEntity(event.get("target").getAsInt());
                        result.addProperty("canceled", target != null && SafeEvents.post(
                            new net.neoforged.neoforge.event.entity.player.AttackEntityEvent(player, target)).isCanceled());
                    }
                    case "piercing" -> player.postPiercingAttack();
                    case "break_pre" -> {
                        var pos = new BlockPos(event.get("x").getAsInt(),event.get("y").getAsInt(),event.get("z").getAsInt());
                        var state = player.level().getBlockState(pos);
                        var canceled = SafeEvents.post(
                            new net.neoforged.neoforge.event.level.block.BreakBlockEvent(player.level(),pos,state,player)).isCanceled();
                        result.addProperty("canceled",canceled);
                        if (!canceled && !BuiltInRegistries.BLOCK.getKey(state.getBlock()).getNamespace().equals("minecraft")) state.getBlock().playerWillDestroy(player.level(),pos,state,player);
                    }
                    case "mine" -> {
                        var pos = new BlockPos(event.get("x").getAsInt(),event.get("y").getAsInt(),event.get("z").getAsInt());
                        var state = Block.stateById(event.get("state").getAsInt());
                        player.getMainHandItem().mineBlock(player.level(),state,pos,player);
                    }
                    case "right_click" -> {
                        var hit = hit(event);
                        var interaction = SafeEvents.post(new net.neoforged.neoforge.event.entity.player.PlayerInteractEvent.RightClickBlock(player,hand(event),hit.getBlockPos(),hit));
                        result.addProperty("canceled",interaction.isCanceled());
                        result.addProperty("block",interaction.getUseBlock() != net.minecraft.util.TriState.FALSE);
                        result.addProperty("item",interaction.getUseItem() != net.minecraft.util.TriState.FALSE);
                    }
                    case "toss" -> {
                        var stack = stackFromRust(decode(event.get("stack").getAsString()));
                        var item = new net.minecraft.world.entity.item.ItemEntity(player.level(),player.getX(),player.getY(),player.getZ(),stack);
                        var toss = SafeEvents.post(new net.neoforged.neoforge.event.entity.item.ItemTossEvent(item,player));
                        result.addProperty("canceled",toss.isCanceled());
                        result.addProperty("stack",encode(stackToRust(item.getItem())));
                    }
                    case "payload" -> result.addProperty("handled", dispatch(player, event));
                    case "use" -> result.addProperty("consumed", use(player, hand(event)));
                    case "close_menu" -> { if (player.containerMenu != player.inventoryMenu) player.closeContainer(); }
                    case "leave" -> {
                        player.doCloseContainer();
                        SafeEvents.post(new net.neoforged.neoforge.event.entity.player.PlayerEvent.PlayerLoggedOutEvent(player));
                    }
                    case "release" -> {
                        result.addProperty("using", player.isUsingItem());
                        if (player.isUsingItem()) player.releaseUsingItem();
                    }
                    case "use_on" -> result.addProperty("consumed", useOn(player, event));
                    case "use_block" -> result.addProperty("consumed", useBlock(player, event));
                    case "menu_packet" -> MenuBridge.packet(player, event.get("data").getAsString());
                    case "menu_tick" -> MenuBridge.tick(player);
                    case "tracking" -> TrackingBridge.apply(player, event);
                    case "tick" -> player.tickMods();
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
            if (!ItemStack.matches(before[slot], after) || stale[slot]) {
                player.synced[slot] = after.copy();
                changes.addProperty(Integer.toString(slot), encode(stackToRust(after)));
            }
        }
        reply.add("inventory", changes);
        reply.add("packets", Outbox.drain());
        reply.addProperty("menu", MenuBridge.openId(player));
        reply.add("results", results);
        // Rust writes them into the player file; refreshed now and then and on leave.
        boolean leaving = false;
        for (var element : request.getAsJsonArray("events")) leaving |= element.getAsJsonObject().get("type").getAsString().equals("leave");
        if ((leaving || player.tickCount % ATTACHMENT_SAVE_INTERVAL == 0) && player.hasAttachments()) reply.addProperty("attachments", encode(player.saveAttachments()));
        if (!motionBefore.equals(player.getDeltaMovement())) {
            var motion = new JsonArray();
            motion.add(player.getDeltaMovement().x); motion.add(player.getDeltaMovement().y); motion.add(player.getDeltaMovement().z);
            reply.add("velocity",motion);
        }
        if (player.pendingImpulse != null) { reply.add("impulse",player.pendingImpulse); player.pendingImpulse = null; }
        return reply;
    }

    private static net.minecraft.world.damagesource.DamageSource damageSource(net.minecraft.world.entity.LivingEntity living, JsonObject event) {
        var key = net.minecraft.resources.ResourceKey.create(net.minecraft.core.registries.Registries.DAMAGE_TYPE,Identifier.parse(event.get("damageType").getAsString()));
        var holder = BridgeContent.worldRegistries.lookupOrThrow(net.minecraft.core.registries.Registries.DAMAGE_TYPE).get(key)
            .orElseGet(() -> BridgeContent.worldRegistries.lookupOrThrow(net.minecraft.core.registries.Registries.DAMAGE_TYPE).getOrThrow(net.minecraft.world.damagesource.DamageTypes.GENERIC));
        var direct = event.has("direct") ? living.level().getEntity(event.get("direct").getAsInt()) : null;
        var attacker = event.has("attacker") ? living.level().getEntity(event.get("attacker").getAsInt()) : null;
        return new net.minecraft.world.damagesource.DamageSource(holder,direct,attacker);
    }

    /** Run injected mod guards at their vanilla entry points; Rust applies the resulting damage once. */
    private static void injected(net.minecraft.world.entity.LivingEntity living, String suffix, Object... arguments) throws Exception {
        for (Class<?> type = living.getClass(); type != Object.class; type = type.getSuperclass()) {
            for (var method : type.getDeclaredMethods()) {
                if (!method.getName().endsWith(suffix) || method.getParameterCount() != arguments.length) continue;
                method.setAccessible(true);
                try { method.invoke(living,arguments); }
                catch (java.lang.reflect.InvocationTargetException error) {
                    if (error.getCause() instanceof RuntimeException runtime) throw runtime;
                    throw error;
                }
            }
        }
    }

    private static JsonObject livingEvent(net.minecraft.world.entity.LivingEntity living, JsonObject event) throws Exception {
        var reply = new JsonObject();
        var source = damageSource(living,event);
        var type = event.has("event") ? event.get("event").getAsString() : event.get("type").getAsString();
        if (type.equals("hurt")) {
            float amount = event.get("amount").getAsFloat();
            var guard = new org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable<Boolean>("hurtServer",true);
            if (living instanceof RustPlayer) injected(living,"de$armorHurt",living.level(),source,amount,guard);
            if (!guard.isCancelled()) injected(living,"hbm$armorCancel",living.level(),source,amount,guard);
            if (guard.isCancelled()) { reply.addProperty("canceled",true); return reply; }
            var incoming = SafeEvents.post(new net.neoforged.neoforge.event.entity.living.LivingIncomingDamageEvent(living,
                new net.neoforged.neoforge.common.damagesource.DamageContainer(source,amount)));
            reply.addProperty("canceled",incoming.isCanceled());
            reply.addProperty("amount",incoming.getAmount());
        } else if (type.equals("death")) {
            var guard = new org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable<Boolean>("checkTotemDeathProtection",true);
            injected(living,"de$lastStand",source,guard);
            boolean saved = guard.isCancelled() && Boolean.TRUE.equals(guard.getReturnValue());
            boolean canceled = saved || SafeEvents.post(new net.neoforged.neoforge.event.entity.living.LivingDeathEvent(living,source)).isCanceled();
            reply.addProperty("canceled",canceled);
            reply.addProperty("health",living.getHealth());
        } else if (type.equals("drops")) {
            var drops = new ArrayList<net.minecraft.world.entity.item.ItemEntity>();
            for (var element : event.getAsJsonArray("drops")) {
                var drop = element.getAsJsonObject();
                var item = new net.minecraft.world.entity.item.ItemEntity(living.level(),drop.get("x").getAsDouble(),drop.get("y").getAsDouble(),drop.get("z").getAsDouble(),stackFromRust(decode(drop.get("stack").getAsString())));
                item.setDeltaMovement(drop.get("vx").getAsDouble(),drop.get("vy").getAsDouble(),drop.get("vz").getAsDouble());
                item.setPickUpDelay(drop.get("delay").getAsInt());
                drops.add(item);
            }
            var dropEvent = SafeEvents.post(new net.neoforged.neoforge.event.entity.living.LivingDropsEvent(living,source,drops,event.get("recentlyHit").getAsBoolean()));
            var result = new JsonArray();
            if (!dropEvent.isCanceled()) {
                for (var item : dropEvent.getDrops()) {
                    var drop = new JsonObject();
                    drop.addProperty("x",item.getX()); drop.addProperty("y",item.getY()); drop.addProperty("z",item.getZ());
                    drop.addProperty("vx",item.getDeltaMovement().x); drop.addProperty("vy",item.getDeltaMovement().y); drop.addProperty("vz",item.getDeltaMovement().z);
                    drop.addProperty("delay",10);
                    drop.addProperty("stack",encode(stackToRust(item.getItem())));
                    result.add(drop);
                }
            }
            reply.add("drops",result);
        }
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
     * the ones Java owns (mod-defined, and vanilla ones Java set such as a container's contents) travel inside custom
     * data under {@link #MOD_COMPONENTS} and persist with the stack.
     */
    @SuppressWarnings({"unchecked", "rawtypes"})
    static ItemStack stackFromRust(CompoundTag tag) {
        var item = BuiltInRegistries.ITEM.getOptional(Identifier.parse(tag.getStringOr("id", "minecraft:air")));
        if (item.isEmpty()) return ItemStack.EMPTY;
        var stack = new ItemStack(item.get(), tag.getIntOr("count", 1));
        var components = tag.getCompoundOrEmpty("components");
        var custom = components.getCompound("minecraft:custom_data").or(() -> components.getCompound("custom_data"));
        // Exact Java values and prototype defaults win over Rust's possibly lossy copies.
        var skipped = new java.util.HashSet<String>();
        if (custom.isPresent()) {
            var data = custom.get().copy();
            var mod = data.getCompoundOrEmpty(MOD_COMPONENTS);
            data.remove(MOD_COMPONENTS);
            for (var name : data.getListOrEmpty(PROTOTYPE_COMPONENTS)) name.asString().ifPresent(skipped::add);
            data.remove(PROTOTYPE_COMPONENTS);
            skipped.addAll(mod.keySet());
            if (!data.isEmpty()) stack.set(DataComponents.CUSTOM_DATA, CustomData.of(data));
            var ops = RegistryOps.create(NbtOps.INSTANCE, BridgeContent.worldRegistries);
            for (var name : mod.keySet()) {
                var type = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse(name));
                if (type == null || type.isTransient()) continue;
                var value = type.codecOrThrow().parse(ops, mod.get(name)).resultOrPartial(error -> LOGGER.warn("Bad {} data: {}", name, error));
                value.ifPresent(parsed -> stack.set((net.minecraft.core.component.DataComponentType) type, parsed));
            }
        }
        var ops = RegistryOps.create(NbtOps.INSTANCE, BridgeContent.worldRegistries);
        for (var name : components.keySet()) {
            if (name.equals("minecraft:custom_data") || name.equals("custom_data") || skipped.contains(name)) continue;
            boolean removed = name.startsWith("!");
            var type = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse(removed ? name.substring(1) : name));
            if (type == null || type.isTransient()) continue;
            if (removed) stack.remove(type);
            else type.codecOrThrow().parse(ops, components.get(name)).resultOrPartial(error -> LOGGER.warn("Bad {} data: {}", name, error))
                .ifPresent(parsed -> stack.set((net.minecraft.core.component.DataComponentType) type, parsed));
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
        var defaults = new net.minecraft.nbt.ListTag();
        var ops = RegistryOps.create(NbtOps.INSTANCE, BridgeContent.worldRegistries);
        var patched = new java.util.HashSet<Object>();
        for (var entry : stack.getComponentsPatch().entrySet()) patched.add(entry.getKey());
        boolean modded = !BuiltInRegistries.ITEM.getKey(stack.getItem()).getNamespace().equals("minecraft");
        // Defaults matter to Rust gameplay too: mod weapons/tools otherwise inherit the stand-in stone item's data.
        // Vanilla items have real Rust prototypes, so only their patch is sent.
        for (var entry : stack.getComponents()) {
            var type = (net.minecraft.core.component.DataComponentType) entry.type();
            var key = BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(type);
            if (key == null || type == DataComponents.CUSTOM_DATA || type.isTransient()) continue;
            boolean prototype = !patched.contains(type);
            if (prototype && !modded) continue;
            type.codecOrThrow().encodeStart(ops, entry.value()).resultOrPartial(error -> LOGGER.warn("Cannot store {}: {}", key, error))
                .ifPresent(encoded -> {
                    if (key.getNamespace().equals("minecraft")) {
                        components.put(key.toString(), (net.minecraft.nbt.Tag) encoded);
                        // Rust's copy of a prototype value can be lossy, so Java and the client keep their own.
                        if (prototype) defaults.add(net.minecraft.nbt.StringTag.valueOf(key.toString()));
                    } else mod.put(key.toString(), (net.minecraft.nbt.Tag) encoded);
                });
        }
        for (var entry : stack.getComponentsPatch().entrySet()) {
            var type = (net.minecraft.core.component.DataComponentType) entry.getKey();
            var key = BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(type);
            // Rust keeps custom data and damage natively; every other component Java set rides along so it survives.
            if (key == null || type == DataComponents.CUSTOM_DATA || type == DataComponents.DAMAGE || type.isTransient()) continue;
            if (entry.getValue().isEmpty()) {
                components.put("!" + key, new CompoundTag());
                continue;
            }
            type.codecOrThrow().encodeStart(ops, entry.getValue().get()).resultOrPartial(error -> LOGGER.warn("Cannot store {}: {}", key, error))
                .ifPresent(encoded -> mod.put(key.toString(), (net.minecraft.nbt.Tag) encoded));
        }
        if (!mod.isEmpty()) data.put(MOD_COMPONENTS, mod);
        if (modded) data.put(PROTOTYPE_COMPONENTS, defaults);
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

package net.solarspire.ferrum.runtime.content;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.util.ArrayList;
import java.util.Base64;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentSkipListMap;
import java.util.function.Consumer;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket;
import net.minecraft.resources.Identifier;
import net.minecraft.util.AbortableIterationConsumer;
import net.minecraft.world.damagesource.DamageSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.HumanoidArm;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.ai.attributes.AttributeMap;
import net.minecraft.world.entity.ai.attributes.DefaultAttributes;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.entity.EntityInLevelCallback;
import net.minecraft.world.level.entity.EntityTypeTest;
import net.minecraft.world.level.entity.LevelEntityGetter;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Mod entities that Java code spawns. Rust allocates the identity and mirrors position and tracking; the Java instance
 * is ticked by Rust's tick loop and answers level queries from its own state, everything else from Rust's world.
 */
final class EntityBridge {
    private static final Logger LOGGER = LoggerFactory.getLogger(EntityBridge.class);
    /** Java instances of bridged entities by Rust entity id; ascending ids are the tick order. */
    private static final ConcurrentSkipListMap<Integer, Entity> MOD = new ConcurrentSkipListMap<>();
    private static final Set<Integer> REMOVED = ConcurrentHashMap.newKeySet();
    /** Bridged entities Rust has listed as alive at least once. */
    private static final Set<Integer> LISTED = ConcurrentHashMap.newKeySet();
    private static final Set<String> DROPPED = ConcurrentHashMap.newKeySet();

    private EntityBridge() {}

    static boolean add(RustLevel level, Entity entity) {
        // Vanilla-typed entities are Rust's own kinds: Rust spawns the real thing and the Java instance is only the request.
        if (entity instanceof ItemEntity item) return spawnItem(item);
        if (entity instanceof ExperienceOrb orb) return spawnExperience(orb);
        var key = BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType());
        if (key.getNamespace().equals("minecraft") || entity instanceof Player) {
            if (DROPPED.add(key.toString())) LOGGER.warn("Dropping Java-spawned {}; only mod entity types are bridged", key);
            return false;
        }
        // The entity took its id from Rust in its constructor, see RustLevel.getNextEntityId.
        int id = entity.getId();
        var spawn = new JsonObject();
        spawn.addProperty("op", "spawn_entity");
        spawn.addProperty("id", id);
        spawn.addProperty("uuid", entity.getUUID().toString());
        spawn.addProperty("type", key.toString());
        var position = entity.position();
        var motion = entity.getDeltaMovement();
        spawn.addProperty("x", position.x);
        spawn.addProperty("y", position.y);
        spawn.addProperty("z", position.z);
        spawn.addProperty("vx", motion.x);
        spawn.addProperty("vy", motion.y);
        spawn.addProperty("vz", motion.z);
        spawn.addProperty("yaw", entity.getYRot());
        spawn.addProperty("pitch", entity.getXRot());
        var meta = metadata(entity);
        if (meta != null) spawn.addProperty("meta", meta);
        if (!RustLevel.call(spawn).getAsBoolean()) return false;
        entity.setLevelCallback(new Callback(id));
        MOD.put(id, entity);
        entity.onAddedToLevel();
        return true;
    }

    private static final java.lang.reflect.Field PICKUP_DELAY = pickupDelayField();

    private static java.lang.reflect.Field pickupDelayField() {
        try {
            var field = ItemEntity.class.getDeclaredField("pickupDelay");
            field.setAccessible(true);
            return field;
        } catch (ReflectiveOperationException error) { throw new ExceptionInInitializerError(error); }
    }

    private static JsonObject spawnRequest(String op, Entity entity) {
        var request = new JsonObject();
        request.addProperty("op", op);
        var position = entity.position();
        var motion = entity.getDeltaMovement();
        request.addProperty("x", position.x);
        request.addProperty("y", position.y);
        request.addProperty("z", position.z);
        request.addProperty("vx", motion.x);
        request.addProperty("vy", motion.y);
        request.addProperty("vz", motion.z);
        return request;
    }

    /** Rust spawns the item entity; item stacks cross as Rust NBT, see PlayerBridge.stackToRust. */
    private static boolean spawnItem(ItemEntity item) {
        var stack = item.getItem();
        if (stack.isEmpty()) return false;
        var request = spawnRequest("spawn_item", item);
        try {
            request.addProperty("stack", PlayerBridge.encode(PlayerBridge.stackToRust(stack)));
            request.addProperty("delay", PICKUP_DELAY.getInt(item));
        } catch (ReflectiveOperationException | java.io.IOException error) {
            throw new IllegalStateException("Cannot hand item entity to Rust", error);
        }
        return RustLevel.call(request).getAsBoolean();
    }

    private static boolean spawnExperience(ExperienceOrb orb) {
        var request = spawnRequest("spawn_experience", orb);
        request.addProperty("amount", orb.getValue());
        return RustLevel.call(request).getAsBoolean();
    }

    /** The entity's non-default synched data as an encoded set-entity-data packet, or null when everything is default. */
    private static String metadata(Entity entity) {
        var values = entity.getEntityData().getNonDefaultValues();
        if (values == null || values.isEmpty()) return null;
        return Base64.getEncoder().encodeToString(Outbox.encode(new ClientboundSetEntityDataPacket(entity.getId(), values)));
    }

    private record Callback(int id) implements EntityInLevelCallback {
        @Override public void onMove() { }
        @Override public void onRemove(Entity.RemovalReason reason) {
            LISTED.remove(id);
            if (MOD.remove(id) != null) REMOVED.add(id);
        }
    }

    private static Set<Integer> ids(JsonObject batch, String name) {
        var result = new HashSet<Integer>();
        for (var element : batch.getAsJsonArray(name)) result.add(element.getAsInt());
        return result;
    }

    /** Ticks the level's bridged entities in id order and reports what Rust mirrors: state, removals and dirty data. */
    /** Rebuilds a mod entity Rust loaded from its chunk, with the id Rust gave it. */
    private static void load(RustLevel level, JsonObject data) {
        int id = data.get("id").getAsInt();
        try {
            var type = BuiltInRegistries.ENTITY_TYPE.getOptional(Identifier.parse(data.get("type").getAsString()));
            var entity = type.isEmpty() ? null : type.get().create(level, net.minecraft.world.entity.EntitySpawnReason.LOAD);
            if (entity == null) { REMOVED.add(id); return; }
            if (data.has("nbt") && !data.get("nbt").isJsonNull()) {
                entity.load(net.minecraft.world.level.storage.TagValueInput.create(net.minecraft.util.ProblemReporter.DISCARDING,
                    level.registryAccess(), PlayerBridge.decode(data.get("nbt").getAsString())));
            } else {
                entity.snapTo(data.get("x").getAsDouble(), data.get("y").getAsDouble(), data.get("z").getAsDouble());
            }
            entity.setId(id);
            entity.setUUID(UUID.fromString(data.get("uuid").getAsString()));
            entity.setLevelCallback(new Callback(id));
            MOD.put(id, entity);
            entity.onAddedToLevel();
        } catch (Exception error) {
            LOGGER.error("Cannot rebuild saved mod entity {} #{}; it is removed", data.get("type").getAsString(), id, error);
            REMOVED.add(id);
        }
    }

    /** Ticks between saves of one entity's Java data. */
    private static final int SAVE_INTERVAL = 100;

    private static String save(Entity entity) {
        try {
            var output = net.minecraft.world.level.storage.TagValueOutput.createWithContext(net.minecraft.util.ProblemReporter.DISCARDING, entity.registryAccess());
            entity.saveWithoutId(output);
            return PlayerBridge.encode(output.buildResult());
        } catch (Exception error) {
            LOGGER.warn("Cannot save mod entity #{}", entity.getId(), error);
            return null;
        }
    }

    static JsonObject tick(RustLevel level, JsonObject batch) {
        var fresh = new HashSet<Integer>();
        if (batch.has("load")) for (var element : batch.getAsJsonArray("load")) {
            load(level, element.getAsJsonObject());
            fresh.add(element.getAsJsonObject().get("id").getAsInt());
        }
        var alive = ids(batch, "alive");
        var ticking = ids(batch, "ticking");
        var result = new JsonObject();
        var states = new JsonArray();
        for (var entity : new ArrayList<>(MOD.values())) {
            if (entity.level() != level) continue;
            int id = entity.getId();
            if (!alive.contains(id)) {
                // Rust adds a spawned entity to its world on a later tick; until Rust listed it once, it is not gone.
                if (!LISTED.contains(id)) continue;
                LISTED.remove(id);
                // Rust dropped it (unloaded chunk): no removal report back.
                MOD.remove(id);
                entity.setLevelCallback(EntityInLevelCallback.NULL);
                entity.setRemoved(Entity.RemovalReason.UNLOADED_TO_CHUNK);
                continue;
            }
            LISTED.add(id);
            if (!ticking.contains(id) || entity.isRemoved()) continue;
            try {
                entity.tick();
            } catch (RuntimeException | LinkageError error) {
                LOGGER.error("Bridged entity {} #{} failed its tick and was removed", BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()), id, error);
                entity.discard();
            }
        }
        for (var entity : MOD.values()) {
            if (entity.level() != level) continue;
            var state = new JsonObject();
            var position = entity.position();
            var motion = entity.getDeltaMovement();
            state.addProperty("id", entity.getId());
            state.addProperty("x", position.x);
            state.addProperty("y", position.y);
            state.addProperty("z", position.z);
            state.addProperty("vx", motion.x);
            state.addProperty("vy", motion.y);
            state.addProperty("vz", motion.z);
            state.addProperty("yaw", entity.getYRot());
            state.addProperty("pitch", entity.getXRot());
            state.addProperty("ground", entity.onGround());
            if (fresh.contains(entity.getId()) || (entity.tickCount + entity.getId()) % SAVE_INTERVAL == 0) {
                var saved = save(entity);
                if (saved != null) state.addProperty("nbt", saved);
            }
            var data = entity.getEntityData();
            if (data.isDirty()) {
                var dirty = data.packDirty();
                if (dirty != null && !dirty.isEmpty()) Outbox.toTracking(entity.getId(), false, new ClientboundSetEntityDataPacket(entity.getId(), dirty));
                var meta = metadata(entity);
                if (meta != null) state.addProperty("meta", meta);
            }
            states.add(state);
        }
        var removed = new JsonArray();
        for (var id : new ArrayList<>(REMOVED)) {
            REMOVED.remove(id);
            removed.add(id);
        }
        result.add("states", states);
        result.add("removed", removed);
        return result;
    }

    /** Forwards damage to Rust, which owns every health value; false when Rust rejected or did not know the target. */
    static boolean damage(Entity target, DamageSource source, float amount) {
        var request = new JsonObject();
        request.addProperty("op", "damage");
        request.addProperty("entity", target.getId());
        request.addProperty("amount", amount);
        request.addProperty("type", source.typeHolder().unwrapKey().map(key -> key.identifier().toString()).orElse("minecraft:generic"));
        request.addProperty("attacker", source.getEntity() == null ? -1 : source.getEntity().getId());
        request.addProperty("direct", source.getDirectEntity() == null ? -1 : source.getDirectEntity().getId());
        return RustLevel.call(request).getAsBoolean();
    }

    static boolean isBridged(Entity entity) { return MOD.get(entity.getId()) == entity; }

    /** Views of Rust mobs by entity id, kept so mods that hold a target see one object across ticks. */
    private static final Map<Integer, RustProxy> PROXIES = new ConcurrentHashMap<>();
    private static final int MAX_PROXIES = 4096;

    private static Entity resolve(RustLevel level, JsonObject snapshot) {
        int id = snapshot.get("id").getAsInt();
        var bridged = MOD.get(id);
        if (bridged != null) return bridged;
        var uuid = UUID.fromString(snapshot.get("uuid").getAsString());
        if (snapshot.get("kind").getAsString().equals("player")) {
            try { return PlayerBridge.view(level, uuid, snapshot); }
            catch (Exception error) { throw new IllegalStateException("Cannot view Rust player " + uuid, error); }
        }
        try {
            var cached = PROXIES.get(id);
            if (cached != null && cached.level() == level && cached.getUUID().equals(uuid)
                && BuiltInRegistries.ENTITY_TYPE.getKey(cached.getType()).toString().equals(snapshot.get("type").getAsString())) {
                cached.apply(snapshot);
                return cached;
            }
            var proxy = RustProxy.create(level, snapshot);
            if (PROXIES.size() >= MAX_PROXIES) PROXIES.clear();
            PROXIES.put(id, proxy);
            return proxy;
        }
        catch (ReflectiveOperationException error) { throw new IllegalStateException("Cannot view Rust entity " + id, error); }
    }

    /** Level entity queries: bridged entities from Java state, players and mobs as views of Rust's entities. */
    static final class Getter implements LevelEntityGetter<Entity> {
        private final RustLevel level;

        Getter(RustLevel level) { this.level = level; }

        private List<Entity> within(AABB box) {
            var found = new ArrayList<Entity>();
            for (var entity : MOD.values()) {
                if (entity.level() == level && !entity.isRemoved() && entity.getBoundingBox().intersects(box)) found.add(entity);
            }
            var request = new JsonObject();
            request.addProperty("op", "entities_in");
            request.addProperty("minX", box.minX);
            request.addProperty("minY", box.minY);
            request.addProperty("minZ", box.minZ);
            request.addProperty("maxX", box.maxX);
            request.addProperty("maxY", box.maxY);
            request.addProperty("maxZ", box.maxZ);
            for (var element : RustLevel.call(request).getAsJsonArray()) found.add(resolve(level, element.getAsJsonObject()));
            return found;
        }

        private Entity one(JsonObject request) {
            request.addProperty("op", "entity");
            var reply = RustLevel.call(request);
            return reply.isJsonNull() ? null : resolve(level, reply.getAsJsonObject());
        }

        @Override public Entity get(int id) {
            var bridged = MOD.get(id);
            if (bridged != null) return bridged.level() == level ? bridged : null;
            var request = new JsonObject();
            request.addProperty("id", id);
            return one(request);
        }

        @Override public Entity get(UUID uuid) {
            for (var entity : MOD.values()) if (entity.getUUID().equals(uuid) && entity.level() == level) return entity;
            var request = new JsonObject();
            request.addProperty("uuid", uuid.toString());
            return one(request);
        }

        @Override public Iterable<Entity> getAll() {
            var all = new ArrayList<Entity>();
            for (var entity : MOD.values()) if (entity.level() == level) all.add(entity);
            all.addAll(PlayerBridge.playersIn(level));
            return all;
        }

        @Override public <U extends Entity> void get(EntityTypeTest<Entity, U> test, AbortableIterationConsumer<U> consumer) {
            for (var entity : getAll()) {
                var cast = test.tryCast(entity);
                if (cast != null && consumer.accept(cast).shouldAbort()) return;
            }
        }

        @Override public void get(AABB box, Consumer<Entity> consumer) { within(box).forEach(consumer); }

        @Override public <U extends Entity> void get(EntityTypeTest<Entity, U> test, AABB box, AbortableIterationConsumer<U> consumer) {
            for (var entity : within(box)) {
                var cast = test.tryCast(entity);
                if (cast != null && consumer.accept(cast).shouldAbort()) return;
            }
        }
    }

    /** A Rust mob, item or projectile as Java code sees it: identity, bounds and health, with damage sent back to Rust. */
    static final class RustProxy extends LivingEntity {
        private RustProxy() { super(null, null); }

        @SuppressWarnings("unchecked")
        static RustProxy create(RustLevel level, JsonObject snapshot) throws ReflectiveOperationException {
            try {
                var proxy = (RustProxy) RustPlayer.allocate(RustProxy.class);
                var type = BuiltInRegistries.ENTITY_TYPE.getOptional(Identifier.parse(snapshot.get("type").getAsString())).orElse(net.minecraft.world.entity.EntityTypes.ZOMBIE);
                RustPlayer.initEntity(proxy, type, UUID.fromString(snapshot.get("uuid").getAsString()), snapshot.get("id").getAsInt(), level);
                RustPlayer.set(proxy, LivingEntity.class, "attributes", new AttributeMap(DefaultAttributes.getSupplier(net.minecraft.world.entity.EntityTypes.PLAYER)));
                RustPlayer.set(proxy, LivingEntity.class, "equipment", new net.minecraft.world.entity.EntityEquipment());
                proxy.apply(snapshot);
                return proxy;
            } catch (Exception error) {
                if (error instanceof ReflectiveOperationException reflective) throw reflective;
                throw new IllegalStateException(error);
            }
        }

        private void apply(JsonObject snapshot) throws ReflectiveOperationException {
            var position = new Vec3(snapshot.get("x").getAsDouble(), snapshot.get("y").getAsDouble(), snapshot.get("z").getAsDouble());
            var block = net.minecraft.core.BlockPos.containing(position);
            RustPlayer.set(this, Entity.class, "position", position);
            RustPlayer.set(this, Entity.class, "blockPosition", block);
            RustPlayer.set(this, Entity.class, "chunkPosition", new net.minecraft.world.level.ChunkPos(block.getX() >> 4, block.getZ() >> 4));
            RustPlayer.set(this, Entity.class, "bb", new AABB(snapshot.get("minX").getAsDouble(), snapshot.get("minY").getAsDouble(), snapshot.get("minZ").getAsDouble(),
                snapshot.get("maxX").getAsDouble(), snapshot.get("maxY").getAsDouble(), snapshot.get("maxZ").getAsDouble()));
            setYRot(snapshot.get("yaw").getAsFloat());
            setXRot(snapshot.get("pitch").getAsFloat());
            setHealth(snapshot.get("health").getAsFloat());
        }

        @Override public HumanoidArm getMainArm() { return HumanoidArm.RIGHT; }

        @Override public boolean hurtServer(net.minecraft.server.level.ServerLevel level, DamageSource source, float amount) { return damage(this, source, amount); }
    }
}

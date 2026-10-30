package net.solarspire.ferrum.runtime.content;

import com.google.gson.JsonObject;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import java.lang.reflect.Method;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import net.minecraft.world.level.ChunkPos;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Tells mods which chunks a Rust player has loaded. Rust owns chunk sending; mods that sync block entities only to
 * the players watching them (HBM's ChunkTrackerIndex) learn about watchers through the hooks the real chunk sender
 * calls, so this replays those hooks from the diffs Rust reports.
 */
final class TrackingBridge {
    private static final Logger LOGGER = LoggerFactory.getLogger(TrackingBridge.class);
    private static final Map<UUID, State> STATES = new ConcurrentHashMap<>();
    private static Method sent;
    private static Method dropped;
    private static Method pump;
    private static Method flush;
    private static boolean resolved;

    private record State(RustPlayer player, RustLevel level, LongOpenHashSet chunks) {}

    private TrackingBridge() {}

    private static synchronized boolean available() {
        if (!resolved) {
            resolved = true;
            if (net.neoforged.fml.ModList.get().isLoaded("hbm")) {
                try {
                    var index = Class.forName("com.hbm.packet.ChunkTrackerIndex");
                    sent = index.getMethod("onChunkSent", net.minecraft.server.level.ServerLevel.class, net.minecraft.server.level.ServerPlayer.class, long.class);
                    dropped = index.getMethod("onChunkDropped", net.minecraft.server.level.ServerLevel.class, net.minecraft.server.level.ServerPlayer.class, long.class);
                } catch (ReflectiveOperationException error) {
                    LOGGER.warn("HBM chunk tracking hooks are missing; its block entity sync will not reach players", error);
                }
            }
        }
        return sent != null;
    }

    private static void call(Method hook, State state, long chunk) {
        try {
            hook.invoke(null, state.level, state.player, chunk);
        } catch (ReflectiveOperationException | RuntimeException error) {
            LOGGER.warn("Chunk tracking hook failed for {}", state.player.getGameProfile().name(), error);
        }
    }

    /** Applies a Rust diff of the chunks the player watches; a different level means the old set is gone. */
    static void apply(RustPlayer player, JsonObject event) {
        if (!available()) return;
        var id = player.getUUID();
        var level = (RustLevel) player.level();
        var state = STATES.get(id);
        if (state != null && state.level != level) {
            forget(id);
            state = null;
        }
        if (state == null) {
            state = new State(player, level, new LongOpenHashSet());
            STATES.put(id, state);
        }
        for (var element : event.getAsJsonArray("removed")) {
            var pair = element.getAsJsonArray();
            long chunk = ChunkPos.pack(pair.get(0).getAsInt(), pair.get(1).getAsInt());
            if (state.chunks.remove(chunk)) call(dropped, state, chunk);
        }
        for (var element : event.getAsJsonArray("added")) {
            var pair = element.getAsJsonArray();
            long chunk = ChunkPos.pack(pair.get(0).getAsInt(), pair.get(1).getAsInt());
            if (state.chunks.add(chunk)) call(sent, state, chunk);
        }
    }

    /**
     * Mods flush batched block entity sync on NeoForge's server tick event, which Rust's tick never posts, and the
     * event's other listeners need a real server. HBM's two flush calls run here once per tick instead.
     */
    static void flushSync() {
        if (!available()) return;
        try {
            if (pump == null) {
                pump = Class.forName("com.hbm.packet.BeSyncTable").getMethod("pump");
                flush = Class.forName("com.hbm.packet.PacketWire").getMethod("flushPending");
            }
            pump.invoke(null);
            flush.invoke(null);
        } catch (ReflectiveOperationException | RuntimeException error) {
            LOGGER.warn("HBM sync flush failed", error);
        }
    }

    static void forget(UUID id) {
        var state = STATES.remove(id);
        if (state == null || !available()) return;
        for (long chunk : state.chunks) call(dropped, state, chunk);
    }
}

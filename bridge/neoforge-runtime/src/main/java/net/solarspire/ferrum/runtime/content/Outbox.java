package net.solarspire.ferrum.runtime.content;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import java.util.Base64;
import java.util.UUID;
import net.minecraft.network.ProtocolInfo;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.game.ClientGamePacketListener;
import net.minecraft.network.protocol.game.GameProtocols;
import net.neoforged.neoforge.network.connection.ConnectionType;

/**
 * Vanilla clientbound packets that Java code produced. They are encoded here with the 26.2 client protocol and
 * leave with the next Rust call, which resolves each entry's target to players and sends the bytes untranslated.
 */
final class Outbox {
    private static final Object LOCK = new Object();
    private static JsonArray pending = new JsonArray();
    private static ProtocolInfo<ClientGamePacketListener> protocol;

    private Outbox() {}

    @SuppressWarnings("unchecked")
    static byte[] encode(Packet<?> packet) {
        ProtocolInfo<ClientGamePacketListener> info;
        synchronized (LOCK) {
            if (protocol == null) {
                protocol = GameProtocols.CLIENTBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(BridgeContent.worldRegistries, ConnectionType.NEOFORGE));
            }
            info = protocol;
        }
        var buffer = Unpooled.buffer();
        try {
            ((StreamCodec<io.netty.buffer.ByteBuf, Packet<?>>) (Object) info.codec()).encode(buffer, packet);
            return ByteBufUtil.getBytes(buffer);
        } finally {
            buffer.release();
        }
    }

    /** Rust owns game mode (and the abilities Java may not change); Java's game mode events never reach a client. */
    private static boolean ownedByRust(Packet<?> packet) {
        return packet instanceof net.minecraft.network.protocol.game.ClientboundPlayerAbilitiesPacket
            || packet instanceof net.minecraft.network.protocol.game.ClientboundGameEventPacket event
                && event.getEvent() == net.minecraft.network.protocol.game.ClientboundGameEventPacket.CHANGE_GAME_MODE;
    }

    private static JsonObject entry(String to, Packet<?> packet) {
        var entry = new JsonObject();
        entry.addProperty("kind", "vanilla");
        entry.addProperty("to", to);
        entry.addProperty("data", Base64.getEncoder().encodeToString(encode(packet)));
        return entry;
    }

    static void add(JsonObject entry) {
        synchronized (LOCK) { pending.add(entry); }
    }

    static void toPlayer(UUID player, Packet<?> packet) {
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundPlayerAbilitiesPacket abilities) {
            // Only flight changes are taken; Rust applies them and sends its own abilities packet.
            var entry = new JsonObject();
            entry.addProperty("kind", "abilities");
            entry.addProperty("uuid", player.toString());
            entry.addProperty("mayfly", abilities.canFly());
            entry.addProperty("flying", abilities.isFlying());
            entry.addProperty("flySpeed", abilities.getFlyingSpeed());
            add(entry);
            return;
        }
        if (ownedByRust(packet)) return;
        var entry = entry("player", packet);
        entry.addProperty("uuid", player.toString());
        add(entry);
    }

    /** Players of the world within the radius of the point, except the entity that caused the packet. */
    static void toArea(String world, Packet<?> packet, double x, double y, double z, double radius, int exceptEntity) {
        if (ownedByRust(packet)) return;
        var entry = entry("area", packet);
        entry.addProperty("world", world);
        entry.addProperty("x", x);
        entry.addProperty("y", y);
        entry.addProperty("z", z);
        entry.addProperty("radius", radius);
        entry.addProperty("except", exceptEntity);
        add(entry);
    }

    /** Players that have the chunk loaded. */
    static void toChunk(int chunkX, int chunkZ, Packet<?> packet) {
        var entry = entry("chunk", packet);
        entry.addProperty("cx", chunkX);
        entry.addProperty("cz", chunkZ);
        add(entry);
    }

    /** Players that currently see the entity, and the entity itself when it is a player. */
    static void toTracking(int entity, boolean includeSelf, Packet<?> packet) {
        if (ownedByRust(packet)) return;
        var entry = entry("tracking", packet);
        entry.addProperty("entity", entity);
        entry.addProperty("self", includeSelf);
        add(entry);
    }

    static JsonArray drain() {
        // Raw packets mods wrote into player channels join the queue before it leaves.
        PlayerBridge.flushChannels();
        synchronized (LOCK) {
            var drained = pending;
            pending = new JsonArray();
            return drained;
        }
    }
}

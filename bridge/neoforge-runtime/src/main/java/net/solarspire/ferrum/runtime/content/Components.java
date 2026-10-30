package net.solarspire.ferrum.runtime.content;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import java.util.Base64;
import net.minecraft.core.component.DataComponentType;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtOps;
import net.minecraft.nbt.Tag;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.RegistryOps;
import net.neoforged.neoforge.network.connection.ConnectionType;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Mod item components between Rust's NBT form and the client wire form. Rust keeps them as NBT in the stack's custom
 * data; the client holds them as real typed components with the stream codec and id of the synced registry.
 */
final class Components {
    private static final Logger LOGGER = LoggerFactory.getLogger(Components.class);
    private static final int MAX_COMPONENT_BYTES = 1024 * 1024;

    private Components() {}

    private static RegistryFriendlyByteBuf buffer(byte[] data) {
        return new RegistryFriendlyByteBuf(data == null ? Unpooled.buffer() : Unpooled.wrappedBuffer(data), BridgeContent.worldRegistries, ConnectionType.NEOFORGE);
    }

    /** NBT components keyed by name to client component ids and encoded values; unknown or transient types are skipped. */
    @SuppressWarnings({"unchecked", "rawtypes"})
    static JsonArray encode(CompoundTag components) {
        var result = new JsonArray();
        var ops = RegistryOps.create(NbtOps.INSTANCE, BridgeContent.worldRegistries);
        for (var name : components.keySet()) {
            var type = (DataComponentType) BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse(name));
            if (type == null || type.isTransient()) continue;
            var value = type.codecOrThrow().parse(ops, components.get(name)).resultOrPartial(error -> LOGGER.warn("Bad {} data: {}", name, error));
            if (value.isEmpty()) continue;
            var buffer = buffer(null);
            try {
                ((StreamCodec) type.streamCodec()).encode(buffer, value.get());
                var entry = new JsonObject();
                entry.addProperty("id", BuiltInRegistries.DATA_COMPONENT_TYPE.getId(type));
                entry.addProperty("data", Base64.getEncoder().encodeToString(ByteBufUtil.getBytes(buffer)));
                result.add(entry);
            } catch (RuntimeException error) {
                LOGGER.warn("Cannot encode component {}", name, error);
            } finally {
                buffer.release();
            }
        }
        return result;
    }

    /** Client component ids and wire values back to NBT keyed by component name; a mod type Java does not know is skipped. */
    @SuppressWarnings({"unchecked", "rawtypes"})
    static CompoundTag decode(JsonArray wire) {
        var result = new CompoundTag();
        var ops = RegistryOps.create(NbtOps.INSTANCE, BridgeContent.worldRegistries);
        for (var element : wire) {
            var entry = element.getAsJsonObject();
            var type = (DataComponentType) BuiltInRegistries.DATA_COMPONENT_TYPE.byId(entry.get("id").getAsInt());
            if (type == null || type.isTransient()) continue;
            var data = Base64.getDecoder().decode(entry.get("data").getAsString());
            if (data.length > MAX_COMPONENT_BYTES) continue;
            var buffer = buffer(data);
            try {
                var value = ((StreamCodec) type.streamCodec()).decode(buffer);
                if (buffer.isReadable()) throw new IllegalStateException(buffer.readableBytes() + " unread bytes");
                var encoded = type.codecOrThrow().encodeStart(ops, value).resultOrPartial(error -> LOGGER.warn("Cannot store {}: {}", entry.get("id"), error));
                encoded.ifPresent(tag -> result.put(BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(type).toString(), (Tag) tag));
            } catch (RuntimeException error) {
                LOGGER.warn("Client sent an undecodable component {}", entry.get("id"), error);
            } finally {
                buffer.release();
            }
        }
        return result;
    }
}

import io.netty.buffer.Unpooled;
import java.io.DataInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.*;
import java.util.stream.Stream;
import net.minecraft.SharedConstants;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.protocol.configuration.ConfigurationProtocols;
import net.minecraft.network.protocol.configuration.ClientboundRegistryDataPacket;
import net.minecraft.network.protocol.common.ClientboundUpdateTagsPacket;
import net.minecraft.network.protocol.game.GameProtocols;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.packs.resources.ResourceProvider;
import net.minecraft.tags.TagNetworkSerialization.NetworkPayload;

/** Decodes captured traffic with the unmodified, locally downloaded vanilla codecs. */
public final class VanillaOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var config = ConfigurationProtocols.CLIENTBOUND_TEMPLATE.bind(FriendlyByteBuf::new).codec();
        var registries = new HashMap<ResourceKey<? extends Registry<?>>, RegistryDataLoader.NetworkedRegistryData>();
        var synchronizedKeys = new HashSet<ResourceKey<? extends Registry<?>>>();
        RegistryDataLoader.SYNCHRONIZED_REGISTRIES.forEach(data -> synchronizedKeys.add(data.key()));
        var tags = new HashMap<ResourceKey<? extends Registry<?>>, NetworkPayload>();
        var frames = new ArrayList<byte[]>();
        int configurationPackets = 0;
        try (var input = new DataInputStream(Files.newInputStream(Path.of(args[0])))) {
            while (input.available() > 0) {
                int state = input.readUnsignedByte();
                int length = input.readInt();
                if (length < 1 || length > 2097152) throw new IllegalArgumentException("Invalid capture length");
                byte[] bytes = input.readNBytes(length);
                if (bytes.length != length) throw new IllegalArgumentException("Truncated capture");
                if (state == 1) { frames.add(bytes); continue; }
                var buffer = new FriendlyByteBuf(Unpooled.wrappedBuffer(bytes));
                try {
                    var packet = config.decode(buffer);
                    if (buffer.isReadable()) throw new IllegalStateException("Trailing bytes in " + packet.type());
                    if (packet instanceof ClientboundRegistryDataPacket data) {
                        if (!synchronizedKeys.contains(data.registry())) throw new IllegalStateException("Unknown synchronized registry " + data.registry());
                        if (registries.put(data.registry(), new RegistryDataLoader.NetworkedRegistryData(data.entries(), NetworkPayload.EMPTY)) != null)
                            throw new IllegalStateException("Duplicate registry " + data.registry());
                    }
                    if (packet instanceof ClientboundUpdateTagsPacket data) {
                        var method = Arrays.stream(data.getClass().getMethods()).filter(m -> m.getName().equals("getTags") || m.getName().equals("tags")).findFirst().orElseThrow();
                        @SuppressWarnings("unchecked") var value = (Map<ResourceKey<? extends Registry<?>>, NetworkPayload>) method.invoke(data);
                        tags.putAll(value);
                    }
                    configurationPackets++;
                } finally { buffer.release(); }
            }
        }
        registries.replaceAll((key, value) -> new RegistryDataLoader.NetworkedRegistryData(value.elements(), tags.getOrDefault(key, NetworkPayload.EMPTY)));
        var builtin = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        builtin.registries().forEach(entry -> {
            var payload = tags.get(entry.key());
            if (payload != null) bindTags(entry.value(), payload);
        });
        var dynamic = RegistryDataLoader.load(registries, ResourceProvider.EMPTY, builtin.listRegistries().toList(), RegistryDataLoader.SYNCHRONIZED_REGISTRIES, Runnable::run).join();
        var access = new RegistryAccess.ImmutableRegistryAccess(Stream.concat(builtin.registries(), dynamic.registries())).freeze();
        BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(access).forEach(components -> components.apply());
        for (var key : tags.keySet()) {
            if (access.lookup(key).isEmpty()) throw new IllegalStateException("Unknown tag registry " + key);
        }
        access.registries().forEach(entry -> {
            var payload = tags.get(entry.key());
            if (payload != null) validateTags(entry.value(), payload);
        });
        var play = GameProtocols.CLIENTBOUND_TEMPLATE.bind(buf -> new RegistryFriendlyByteBuf(buf, access)).codec();
        var counts = new TreeMap<String, Integer>();
        var errors = new TreeSet<String>();
        for (byte[] bytes : frames) {
            var buffer = new RegistryFriendlyByteBuf(Unpooled.wrappedBuffer(bytes), access);
            try {
                var packet = play.decode(buffer);
                if (buffer.isReadable()) throw new IllegalStateException("Trailing " + buffer.readableBytes() + " bytes in " + packet.type());
                counts.merge(packet.type().toString(), 1, Integer::sum);
            } catch (Exception error) {
                var detail = new StringBuilder(error.toString());
                for (Throwable cause = error.getCause(); cause != null; cause = cause.getCause())
                    detail.append("; caused by ").append(cause);
                errors.add("packet ID " + (bytes[0] & 255) + " length " + bytes.length + ": " + detail);
            } finally { buffer.release(); }
        }
        if (!errors.isEmpty()) throw new IllegalStateException(String.join("\n", errors));
        System.out.println("PASS vanilla " + SharedConstants.getCurrentVersion().name() + ": " + configurationPackets + " configuration packets, " + registries.size() + " registries, " + frames.size() + " play packets, " + counts.size() + " packet types decoded without trailing bytes");
    }

    private static <T> void bindTags(Registry<T> registry, NetworkPayload payload) {
        validateTags(registry, payload);
        registry.prepareTagReload(payload.resolve(registry)).apply();
    }

    private static void validateTags(Registry<?> registry, NetworkPayload payload) {
        var buffer = new FriendlyByteBuf(Unpooled.buffer());
        try {
            try { payload.getClass().getMethod("write", FriendlyByteBuf.class).invoke(payload, buffer); }
            catch (NoSuchMethodException oldMethod) {
                @SuppressWarnings("unchecked") var codec = (net.minecraft.network.codec.StreamCodec<io.netty.buffer.ByteBuf, NetworkPayload>) NetworkPayload.class.getField("STREAM_CODEC").get(null);
                codec.encode(buffer, payload);
            }
            int count = buffer.readVarInt();
            for (int i = 0; i < count; i++) {
                var name = buffer.readIdentifier();
                for (int id : buffer.readVarIntArray())
                    if (registry.get(id).isEmpty()) throw new IllegalStateException("Invalid tag ID " + id + " in " + registry.key() + " / " + name);
            }
        } catch (ReflectiveOperationException error) { throw new IllegalStateException(error); }
        finally { buffer.release(); }
    }
}

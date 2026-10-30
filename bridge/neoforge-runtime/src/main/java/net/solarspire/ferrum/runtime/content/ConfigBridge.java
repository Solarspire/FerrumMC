package net.solarspire.ferrum.runtime.content;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.embedded.EmbeddedChannel;
import java.util.ArrayDeque;
import java.util.Base64;
import java.util.Map;
import java.util.Queue;
import java.util.concurrent.ConcurrentHashMap;
import net.minecraft.network.Connection;
import net.minecraft.network.DisconnectionDetails;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.PacketProcessor;
import net.minecraft.network.chat.Component;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.PacketFlow;
import net.minecraft.network.protocol.common.ClientboundCustomPayloadPacket;
import net.minecraft.network.protocol.common.ClientboundPingPacket;
import net.minecraft.network.protocol.common.ServerboundClientInformationPacket;
import net.minecraft.network.protocol.common.ServerboundCustomClickActionPacket;
import net.minecraft.network.protocol.common.ServerboundCustomPayloadPacket;
import net.minecraft.network.protocol.common.ServerboundKeepAlivePacket;
import net.minecraft.network.protocol.common.ServerboundPongPacket;
import net.minecraft.network.protocol.common.ServerboundResourcePackPacket;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.network.protocol.configuration.ConfigurationProtocols;
import net.minecraft.network.protocol.configuration.ServerConfigurationPacketListener;
import net.minecraft.network.protocol.configuration.ServerboundAcceptCodeOfConductPacket;
import net.minecraft.network.protocol.configuration.ServerboundFinishConfigurationPacket;
import net.minecraft.network.protocol.configuration.ServerboundSelectKnownPacks;
import net.minecraft.network.protocol.cookie.ServerboundCookieResponsePacket;
import net.minecraft.server.network.ConfigurationTask;
import net.minecraft.server.network.config.SynchronizeRegistriesTask;
import net.minecraft.server.packs.repository.KnownPack;
import net.neoforged.fml.ModLoader;
import net.neoforged.neoforge.network.ConfigurationInitialization;
import net.neoforged.neoforge.network.connection.ConnectionType;
import net.neoforged.neoforge.network.event.RegisterConfigurationTasksEvent;
import net.neoforged.neoforge.network.payload.CommonRegisterPayload;
import net.neoforged.neoforge.network.payload.CommonVersionPayload;
import net.neoforged.neoforge.network.payload.MinecraftRegisterPayload;
import net.neoforged.neoforge.network.payload.MinecraftUnregisterPayload;
import net.neoforged.neoforge.network.payload.ModdedNetworkQueryPayload;
import net.neoforged.neoforge.network.registration.NetworkRegistry;

/**
 * Runs NeoForge's configuration negotiation and tasks for a Rust-owned connection.
 * Mirrors the NeoForge-patched ServerConfigurationPacketListenerImpl; Rust owns the socket
 * and the transition to play; registry and tag synchronization runs here so mod datapack entries reach the client.
 */
final class ConfigBridge {
    private static final int MAX_PAYLOAD = 1024 * 1024;
    private static final ConfigurationTask.Type VANILLA_SYNC = new ConfigurationTask.Type("synchronize_registries");
    private static final ConfigurationTask.Type JOIN_WORLD = new ConfigurationTask.Type("join_world");
    private static final Map<Long, Session> SESSIONS = new ConcurrentHashMap<>();
    private static final org.slf4j.Logger LOGGER = org.slf4j.LoggerFactory.getLogger(ConfigBridge.class);
    private static PacketProcessor processor;

    private ConfigBridge() {}

    static String handle(String requestJson) {
        JsonObject request = JsonParser.parseString(requestJson).getAsJsonObject();
        long id = request.get("connection").getAsLong();
        String op = request.get("op").getAsString();
        if (op.equals("close")) {
            Session closed = SESSIONS.remove(id);
            if (closed != null) closed.channel.close();
            return "{}";
        }
        if (processor == null) processor = new PacketProcessor(Thread.currentThread());
        Session session = op.equals("open") ? new Session() : SESSIONS.get(id);
        if (session == null) throw new IllegalStateException("No NeoForge configuration session for connection " + id);
        try {
            switch (op) {
                case "open" -> {
                    SESSIONS.put(id, session);
                    session.startConfiguration();
                }
                case "pong" -> session.onPong();
                case "payload" -> {
                    byte[] body = Base64.getDecoder().decode(request.get("data").getAsString());
                    if (body.length > MAX_PAYLOAD) throw new IllegalArgumentException("Custom payload exceeds 1 MiB");
                    var packet = ServerboundCustomPayloadPacket.CONFIG_STREAM_CODEC.decode(
                        new FriendlyByteBuf(Unpooled.wrappedBuffer(body)));
                    session.handleCustomPayload(packet);
                }
                case "select_known_packs" -> session.selectKnownPacks();
                case "known_packs" -> {
                    session.synchronizeRegistries(request.getAsJsonArray("packs"));
                    session.finishCurrentTask(VANILLA_SYNC);
                }
                default -> throw new IllegalArgumentException("Unknown configuration operation " + op);
            }
            processor.processQueuedPackets();
            session.tickTasks();
        } catch (RuntimeException error) {
            LOGGER.error("NeoForge configuration step {} failed for connection {}", op, id, error);
            session.disconnect(Component.literal("NeoForge configuration failed: " + error));
        }
        JsonObject reply = session.drain();
        if (reply.has("disconnect")) {
            SESSIONS.remove(id);
            session.channel.close();
        }
        return reply.toString();
    }

    private static final class Session implements ServerConfigurationPacketListener {
        private final Connection connection = new Connection(PacketFlow.SERVERBOUND);
        private final EmbeddedChannel channel = new EmbeddedChannel(connection);
        private final Queue<ConfigurationTask> tasks = new ArrayDeque<>();
        private JsonArray packets = new JsonArray();
        private ConnectionType connectionType = ConnectionType.OTHER;
        private ConfigurationTask current;
        private String action;
        private String disconnect;
        private SynchronizeRegistriesTask registrySync;

        /** Vanilla's SynchronizeRegistriesTask: offers the server's packs, then sends registries and tags. */
        void selectKnownPacks() {
            registrySync = new SynchronizeRegistriesTask(BridgeContent.serverKnownPacks, BridgeContent.layeredRegistries);
            registrySync.start(this::send);
        }

        void synchronizeRegistries(JsonArray clientPacks) {
            if (registrySync == null) throw new IllegalStateException("Known packs received before they were requested");
            var packs = new java.util.ArrayList<KnownPack>();
            for (var pack : clientPacks) {
                var entry = pack.getAsJsonObject();
                packs.add(new KnownPack(entry.get("namespace").getAsString(), entry.get("id").getAsString(), entry.get("version").getAsString()));
            }
            registrySync.handleResponse(packs, this::send);
            registrySync = null;
        }

        void startConfiguration() {
            send(new MinecraftUnregisterPayload(NetworkRegistry.getInitialServerUnregisterChannels()));
            send(new MinecraftRegisterPayload(NetworkRegistry.getInitialListeningChannels(flow())));
            send(new ModdedNetworkQueryPayload(Map.of()));
            send(new ClientboundPingPacket(0));
        }

        void onPong() {
            // Client admission seam: a future Fabric path belongs in this switch.
            switch (connectionType) {
                case NEOFORGE -> { }
                default -> {
                    disconnect(Component.literal("This server requires NeoForge 26.2 with the server's mods."));
                    return;
                }
            }
            ConfigurationInitialization.configureEarlyTasks(this, tasks::add);
            tasks.add(new Marker(VANILLA_SYNC, "vanilla_sync"));
            tasks.addAll(ModLoader.postEventWithReturn(new RegisterConfigurationTasksEvent(this)).getConfigurationTasks());
            tasks.add(new Marker(JOIN_WORLD, "finish"));
            startNextTask();
        }

        JsonObject drain() {
            JsonObject reply = new JsonObject();
            reply.add("packets", packets);
            packets = new JsonArray();
            if (action != null) reply.addProperty("action", action);
            action = null;
            if (disconnect != null) reply.addProperty("disconnect", disconnect);
            reply.addProperty("neoforge", connectionType.isNeoForge());
            return reply;
        }

        /** Vanilla ticks the current task every server tick; tasks that finish on tick end here. */
        void tickTasks() {
            while (current != null && isAcceptingMessages() && current.tick()) finishCurrentTask(current.type());
        }

        private void startNextTask() {
            if (current != null) throw new IllegalStateException("Task " + current.type().id() + " has not finished yet");
            if (!isAcceptingMessages()) return;
            current = tasks.poll();
            if (current != null) current.start(this::send);
        }

        @Override
        public void finishCurrentTask(ConfigurationTask.Type type) {
            ConfigurationTask.Type currentType = current != null ? current.type() : null;
            if (!type.equals(currentType)) {
                throw new IllegalStateException("Unexpected request for task finish, current task: " + currentType + ", requested: " + type);
            }
            current = null;
            startNextTask();
        }

        @Override
        public void handleCustomPayload(ServerboundCustomPayloadPacket packet) {
            CustomPacketPayload payload = packet.payload();
            if (payload instanceof ModdedNetworkQueryPayload query) {
                connectionType = ConnectionType.NEOFORGE;
                NetworkRegistry.initializeNeoForgeConnection(this, query.queries());
            } else if (payload instanceof MinecraftRegisterPayload register) {
                NetworkRegistry.onMinecraftRegister(connection, register.newChannels());
            } else if (payload instanceof MinecraftUnregisterPayload unregister) {
                NetworkRegistry.onMinecraftUnregister(connection, unregister.forgottenChannels());
            } else if (payload instanceof CommonVersionPayload version) {
                NetworkRegistry.checkCommonVersion(this, version);
            } else if (payload instanceof CommonRegisterPayload register) {
                NetworkRegistry.onCommonRegister(this, register);
            } else if (NetworkRegistry.isModdedPayload(payload)) {
                NetworkRegistry.handleModdedPayload(this, packet);
            }
        }

        @Override
        public void send(Packet<?> packet) {
            if (disconnect != null) return;
            ByteBuf buffer = Unpooled.buffer();
            try {
                @SuppressWarnings("unchecked")
                var codec = (net.minecraft.network.codec.StreamCodec<ByteBuf, Packet<?>>) (Object) ConfigurationProtocols.CLIENTBOUND.codec();
                codec.encode(buffer, packet);
                if (buffer.readableBytes() > MAX_PAYLOAD + 64) {
                    throw new IllegalStateException("Configuration packet " + packet.type() + " exceeds the client payload limit");
                }
                packets.add(Base64.getEncoder().encodeToString(ByteBufUtil.getBytes(buffer)));
            } finally {
                buffer.release();
            }
        }

        @Override
        public void send(CustomPacketPayload payload) {
            send(new ClientboundCustomPayloadPacket(payload));
        }

        @Override
        public void send(Packet<?> packet, ChannelFutureListener listener) {
            send(packet);
        }

        @Override
        public void disconnect(Component reason) {
            if (disconnect != null) return;
            // Encoded here so the client shows NeoForge's translatable reason.
            send(new net.minecraft.network.protocol.common.ClientboundDisconnectPacket(reason));
            disconnect = reason.getString();
        }

        @Override
        public Connection getConnection() { return connection; }

        @Override
        public PacketProcessor getPacketProcessor() { return processor; }

        @Override
        public ConnectionType getConnectionType() { return connectionType; }

        @Override
        public PacketFlow flow() { return PacketFlow.SERVERBOUND; }

        @Override
        public boolean isAcceptingMessages() { return disconnect == null; }

        @Override
        public void onDisconnect(DisconnectionDetails details) {}

        // Rust handles the vanilla configuration packets on its own listener.
        @Override public void handleKeepAlive(ServerboundKeepAlivePacket packet) {}
        @Override public void handlePong(ServerboundPongPacket packet) {}
        @Override public void handleResourcePackResponse(ServerboundResourcePackPacket packet) {}
        @Override public void handleClientInformation(ServerboundClientInformationPacket packet) {}
        @Override public void handleCustomClickAction(ServerboundCustomClickActionPacket packet) {}
        @Override public void handleCookieResponse(ServerboundCookieResponsePacket packet) {}
        @Override public void handleConfigurationFinished(ServerboundFinishConfigurationPacket packet) {}
        @Override public void handleSelectKnownPacks(ServerboundSelectKnownPacks packet) {}
        @Override public void handleAcceptCodeOfConduct(ServerboundAcceptCodeOfConductPacket packet) {}

        private final class Marker implements ConfigurationTask {
            private final Type type;
            private final String marker;

            Marker(Type type, String marker) {
                this.type = type;
                this.marker = marker;
            }

            @Override
            public void start(java.util.function.Consumer<Packet<?>> sender) { action = marker; }

            @Override
            public Type type() { return type; }
        }
    }
}

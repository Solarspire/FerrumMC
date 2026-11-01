package net.solarspire.ferrum.runtime.probe.client;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import java.io.DataInputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.zip.DataFormatException;
import java.util.zip.Deflater;
import java.util.zip.Inflater;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.ConnectionProtocol;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.ProtocolInfo;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.common.ClientboundCustomPayloadPacket;
import net.minecraft.network.protocol.common.ClientboundDisconnectPacket;
import net.minecraft.network.protocol.common.ClientboundKeepAlivePacket;
import net.minecraft.network.protocol.common.ClientboundPingPacket;
import net.minecraft.network.protocol.common.ServerboundClientInformationPacket;
import net.minecraft.network.protocol.common.ServerboundCustomPayloadPacket;
import net.minecraft.network.protocol.common.ServerboundKeepAlivePacket;
import net.minecraft.network.protocol.common.ServerboundPongPacket;
import net.minecraft.network.protocol.common.custom.BrandPayload;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.network.protocol.common.custom.DiscardedPayload;
import net.minecraft.network.protocol.configuration.ClientboundFinishConfigurationPacket;
import net.minecraft.network.protocol.configuration.ClientboundRegistryDataPacket;
import net.minecraft.network.protocol.configuration.ClientboundSelectKnownPacks;
import net.minecraft.network.protocol.configuration.ConfigurationProtocols;
import net.minecraft.network.protocol.configuration.ServerboundFinishConfigurationPacket;
import net.minecraft.network.protocol.configuration.ServerboundSelectKnownPacks;
import net.minecraft.network.protocol.game.ClientboundLevelChunkWithLightPacket;
import net.minecraft.network.protocol.game.ClientboundPlayerPositionPacket;
import net.minecraft.network.protocol.game.GameProtocols;
import net.minecraft.network.protocol.game.ServerboundAcceptTeleportationPacket;
import net.minecraft.network.protocol.game.ClientboundChunkBatchFinishedPacket;
import net.minecraft.network.protocol.game.ServerboundChunkBatchReceivedPacket;
import net.minecraft.resources.Identifier;
import net.minecraft.server.level.ClientInformation;
import net.minecraft.util.Mth;
import net.minecraft.world.level.block.Block;
import net.neoforged.neoforge.network.connection.ConnectionType;
import net.neoforged.neoforge.network.payload.ExtensibleEnumAcknowledgePayload;
import net.neoforged.neoforge.network.payload.ExtensibleEnumDataPayload;
import net.neoforged.neoforge.network.payload.FeatureFlagAcknowledgePayload;
import net.neoforged.neoforge.network.payload.FeatureFlagDataPayload;
import net.neoforged.neoforge.network.payload.FrozenRegistryPayload;
import net.neoforged.neoforge.network.payload.FrozenRegistrySyncCompletedPayload;
import net.neoforged.neoforge.network.payload.FrozenRegistrySyncStartPayload;
import net.neoforged.neoforge.network.payload.KnownRegistryDataMapsPayload;
import net.neoforged.neoforge.network.payload.KnownRegistryDataMapsReplyPayload;
import net.neoforged.neoforge.network.payload.ModdedNetworkPayload;
import net.neoforged.neoforge.network.payload.ModdedNetworkQueryComponent;
import net.neoforged.neoforge.network.payload.ModdedNetworkQueryPayload;
import net.neoforged.neoforge.network.payload.ModdedNetworkSetupFailedPayload;
import net.neoforged.neoforge.network.registration.NetworkRegistry;
import net.neoforged.neoforge.network.registration.PayloadRegistration;

/**
 * Headless NeoForge 26.2 client for acceptance tests. Every configuration packet is decoded and
 * answered with the pinned NeoForge/Minecraft codecs, as a modded client would.
 */
public final class ClientProbe {
    private static final int PROTOCOL = 776;
    private final Socket socket;
    private final DataInputStream in;
    private final OutputStream out;
    private final JsonObject report = new JsonObject();
    private final JsonArray decodeFailures = new JsonArray();
    private final JsonArray configPayloads = new JsonArray();
    private int threshold = -1;
    private final Map<net.minecraft.resources.ResourceKey<? extends Registry<?>>, List<net.minecraft.core.RegistrySynchronization.PackedRegistryEntry>> syncedElements = new HashMap<>();
    private Map<net.minecraft.resources.ResourceKey<? extends Registry<?>>, net.minecraft.tags.TagNetworkSerialization.NetworkPayload> syncedTags = Map.of();
    private RegistryAccess gameRegistries;
    private int registryMismatches;
    private int frozenRegistries;

    private ClientProbe(Socket socket) throws IOException {
        this.socket = socket;
        this.in = new DataInputStream(socket.getInputStream());
        this.out = socket.getOutputStream();
    }

    /** Joins the server, waits for chunk 0,0 and checks the state at the given position. */
    public static String run(String host, int port, String blockName, int x, int y, int z, int timeoutMillis, String placeItem, String gunItem, String crateItem) throws Exception {
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress(host, port), timeoutMillis);
            socket.setSoTimeout(timeoutMillis);
            var probe = new ClientProbe(socket);
            probe.login();
            probe.configuration();
            probe.play(blockName, x, y, z, placeItem, gunItem, crateItem);
            if (!probe.carrying) return probe.report.toString();
            // The first session ends with the stack on the cursor of the open machine menu; the connection just drops.
            socket.close();
            Thread.sleep(2500);
            rejoin(host, port, timeoutMillis, probe.report, new net.minecraft.core.BlockPos(x, y, z));
            return probe.report.toString();
        }
    }

    private boolean carrying;

    /** A second login: the stack carried at disconnect is back in the inventory, and breaking a filled machine drops its contents. */
    private static void rejoin(String host, int port, int timeoutMillis, JsonObject report, net.minecraft.core.BlockPos machine) throws Exception {
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress(host, port), timeoutMillis);
            socket.setSoTimeout(timeoutMillis);
            var probe = new ClientProbe(socket);
            probe.login();
            probe.configuration();
            probe.rejoinPlay(report, machine);
        }
    }

    private void rejoinPlay(JsonObject into, net.minecraft.core.BlockPos machine) throws Exception {
        RegistryAccess access = gameRegistries != null ? gameRegistries : RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        ProtocolInfo<?> clientbound = GameProtocols.CLIENTBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(access, ConnectionType.NEOFORGE));
        ProtocolInfo<?> serverbound = GameProtocols.SERVERBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(access, ConnectionType.NEOFORGE), () -> true);
        var result = new JsonObject();
        into.add("rejoin", result);
        int[] cobble = {-1};
        boolean[] chunk = {false};
        pump(clientbound, serverbound, 60000, packet -> {
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket set && set.containerId() == 0) {
                cobble[0] = countOf(set.items(), 0, set.items().size(), net.minecraft.world.item.Items.COBBLESTONE);
            } else if (packet instanceof ClientboundLevelChunkWithLightPacket loaded && loaded.getX() == (machine.getX() >> 4) && loaded.getZ() == (machine.getZ() >> 4)) {
                chunk[0] = true;
            }
            return cobble[0] >= 0 && chunk[0];
        });
        result.addProperty("carriedBack", cobble[0] > 0);
        result.addProperty("cobblestone", cobble[0]);
        System.out.println("rejoin: " + result);
        command(serverbound, "gamemode creative @s");
        pump(clientbound, serverbound, 1000, packet -> false);
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.SAND, 16), new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.RAW_IRON, 16));
        var hit = new net.minecraft.world.phys.BlockHitResult(
            new net.minecraft.world.phys.Vec3(machine.getX() + 0.5, machine.getY() + 1.0, machine.getZ() + 0.5), net.minecraft.core.Direction.UP, machine, false);
        var menu = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, hit, ++useSequence));
        result.addProperty("machineOpened", menu.id() > 0);
        if (menu.id() <= 0) return;
        int machineSlots = menu.items().size() - 36;
        int[] filled = {0};
        for (int hotbar = 0; hotbar < 2; hotbar++) {
            sendPacket(serverbound, clickPacket(menu.id(), menu.stateId(), menu.items().size() - 9 + hotbar, 0, net.minecraft.world.inventory.ContainerInput.QUICK_MOVE));
            pump(clientbound, serverbound, 1500, packet -> {
                if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket slot && slot.getContainerId() == menu.id()
                    && slot.getSlot() < machineSlots && !slot.getItem().isEmpty()) filled[0]++;
                return false;
            });
        }
        result.addProperty("machineFilledSlots", filled[0]);
        closeMenu(clientbound, serverbound, menu);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
            net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK, machine, net.minecraft.core.Direction.EAST, ++useSequence));
        int[] drops = {0};
        pump(clientbound, serverbound, 5000, packet -> {
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundAddEntityPacket add && BuiltInRegistries.ENTITY_TYPE.getKey(add.getType()).toString().equals("minecraft:item")) drops[0]++;
            return false;
        });
        result.addProperty("machineDrops", drops[0]);
        System.out.println("rejoin: " + result);
    }

    private void login() throws IOException {
        send(buf -> {
            buf.writeVarInt(0);
            buf.writeVarInt(PROTOCOL);
            buf.writeUtf("127.0.0.1");
            buf.writeShort(25565);
            buf.writeVarInt(2);
        });
        send(buf -> {
            buf.writeVarInt(0);
            buf.writeUtf("NeoProbe");
            buf.writeUUID(UUID.nameUUIDFromBytes("OfflinePlayer:NeoProbe".getBytes()));
        });
        while (true) {
            FriendlyByteBuf packet = read();
            int id = packet.readVarInt();
            switch (id) {
                case 0 -> throw new IllegalStateException("Login disconnect: " + packet.readUtf());
                case 2 -> {
                    send(buf -> buf.writeVarInt(3));
                    return;
                }
                case 3 -> threshold = packet.readVarInt();
                case 4 -> {
                    int transaction = packet.readVarInt();
                    send(buf -> {
                        buf.writeVarInt(2);
                        buf.writeVarInt(transaction);
                        buf.writeBoolean(false);
                    });
                }
                default -> throw new IllegalStateException("Unexpected login packet " + id);
            }
        }
    }

    private static ClientInformation clientInformation() {
        ClientInformation defaults = ClientInformation.createDefault();
        String viewDistance = System.getenv("FERRUM_PROBE_VIEW_DISTANCE");
        if (viewDistance == null) return defaults;
        return new ClientInformation(defaults.language(), Integer.parseInt(viewDistance), defaults.chatVisibility(),
            defaults.chatColors(), defaults.modelCustomisation(), defaults.mainHand(), defaults.textFilteringEnabled(),
            defaults.allowsListing(), defaults.particleStatus());
    }

    private void configuration() throws IOException {
        ProtocolInfo<?> clientbound = ConfigurationProtocols.CLIENTBOUND;
        ProtocolInfo<?> serverbound = ConfigurationProtocols.SERVERBOUND;
        sendPacket(serverbound, new ServerboundCustomPayloadPacket(new BrandPayload("neoforge")));
        sendPacket(serverbound, new ServerboundClientInformationPacket(clientInformation()));
        boolean negotiated = false;
        boolean syncCompleted = false;
        int registryData = 0;
        while (true) {
            FriendlyByteBuf raw = read();
            Packet<?> packet = decode(clientbound, raw);
            if (packet == null) continue;
            System.out.println("config <- " + packet.type().id()
                + (packet instanceof ClientboundCustomPayloadPacket custom ? " " + custom.payload().type().id() : ""));
            if (packet instanceof ClientboundDisconnectPacket disconnect) {
                throw new IllegalStateException("Configuration disconnect: " + disconnect.reason().getString());
            } else if (packet instanceof ClientboundPingPacket ping) {
                sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
            } else if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
                sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            } else if (packet instanceof ClientboundSelectKnownPacks) {
                sendPacket(serverbound, new ServerboundSelectKnownPacks(List.of()));
            } else if (packet instanceof ClientboundRegistryDataPacket data) {
                registryData++;
                syncedElements.computeIfAbsent(data.registry(), key -> new ArrayList<>()).addAll(data.entries());
            } else if (packet instanceof net.minecraft.network.protocol.common.ClientboundUpdateTagsPacket tags) {
                syncedTags = tags.getTags();
            } else if (packet instanceof ClientboundFinishConfigurationPacket) {
                if (!negotiated) throw new IllegalStateException("Configuration finished without NeoForge channel negotiation");
                if (!syncCompleted || frozenRegistries == 0) throw new IllegalStateException("Configuration finished without frozen registry sync");
                sendPacket(serverbound, ServerboundFinishConfigurationPacket.INSTANCE);
                report.addProperty("negotiated", true);
                report.addProperty("frozenRegistries", frozenRegistries);
                report.addProperty("registryMismatches", registryMismatches);
                report.addProperty("registryData", registryData);
                collectGameRegistries();
                report.add("configPayloads", configPayloads);
                if (registryMismatches != 0) throw new IllegalStateException(registryMismatches + " synced registry IDs differ from the client's");
                return;
            } else if (packet instanceof ClientboundCustomPayloadPacket custom) {
                CustomPacketPayload payload = custom.payload();
                configPayloads.add(payload.type().id().toString());
                if (payload instanceof ModdedNetworkQueryPayload) {
                    sendPacket(serverbound, new ServerboundCustomPayloadPacket(new ModdedNetworkQueryPayload(localChannels())));
                } else if (payload instanceof ModdedNetworkPayload) {
                    negotiated = true;
                } else if (payload instanceof ModdedNetworkSetupFailedPayload failed) {
                    throw new IllegalStateException("Channel negotiation failed: " + failed.failureReasons());
                } else if (payload instanceof FrozenRegistrySyncStartPayload) {
                    frozenRegistries = 0;
                } else if (payload instanceof FrozenRegistryPayload frozen) {
                    checkSnapshot(frozen);
                } else if (payload instanceof FrozenRegistrySyncCompletedPayload) {
                    syncCompleted = true;
                    sendPacket(serverbound, new ServerboundCustomPayloadPacket(FrozenRegistrySyncCompletedPayload.INSTANCE));
                } else if (payload instanceof KnownRegistryDataMapsPayload known) {
                    Map<net.minecraft.resources.ResourceKey<? extends Registry<?>>, java.util.Collection<Identifier>> reply = new HashMap<>();
                    known.dataMaps().forEach((key, maps) -> reply.put(key, maps.stream().map(KnownRegistryDataMapsPayload.KnownDataMap::id).toList()));
                    sendPacket(serverbound, new ServerboundCustomPayloadPacket(new KnownRegistryDataMapsReplyPayload(reply)));
                } else if (payload instanceof ExtensibleEnumDataPayload) {
                    sendPacket(serverbound, new ServerboundCustomPayloadPacket(ExtensibleEnumAcknowledgePayload.INSTANCE));
                } else if (payload instanceof FeatureFlagDataPayload) {
                    sendPacket(serverbound, new ServerboundCustomPayloadPacket(FeatureFlagAcknowledgePayload.INSTANCE));
                }
            }
        }
    }

    /** The client's RegistryDataCollector.collectGameRegistries on what the server synchronized. */
    @SuppressWarnings({"unchecked", "rawtypes"})
    private void collectGameRegistries() {
        var staticAccess = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        Map<net.minecraft.resources.ResourceKey<? extends Registry<?>>, net.minecraft.resources.RegistryDataLoader.NetworkedRegistryData> elements = new HashMap<>();
        syncedElements.forEach((key, entries) -> elements.put(key,
            new net.minecraft.resources.RegistryDataLoader.NetworkedRegistryData(entries, net.minecraft.tags.TagNetworkSerialization.NetworkPayload.EMPTY)));
        List<Registry.PendingTags<?>> pending = new ArrayList<>();
        List<Registry.PendingTags<?>> staticPending = new ArrayList<>();
        syncedTags.forEach((key, payload) -> {
            if (payload.isEmpty()) return;
            if (net.minecraft.core.RegistrySynchronization.isNetworkable(key)) {
                var existing = elements.get(key);
                elements.put(key, new net.minecraft.resources.RegistryDataLoader.NetworkedRegistryData(
                    existing == null ? List.of() : existing.elements(), payload));
            } else {
                Registry registry = staticAccess.lookupOrThrow(key);
                var result = pending(registry, payload);
                pending.add(result);
                staticPending.add(result);
            }
        });
        var lookups = net.minecraft.tags.TagLoader.buildUpdatedLookups(staticAccess, pending);
        var dynamic = net.minecraft.resources.RegistryDataLoader.load(elements, net.minecraft.server.packs.resources.ResourceProvider.EMPTY,
            lookups, net.minecraft.resources.RegistryDataLoader.SYNCHRONIZED_REGISTRIES, Runnable::run).join();
        var composite = new RegistryAccess.ImmutableRegistryAccess(java.util.stream.Stream.concat(staticAccess.registries(), dynamic.registries())).freeze();
        staticPending.forEach(Registry.PendingTags::apply);
        gameRegistries = composite;
        // Throws "Missing element" when a mod's datapack entry (e.g. hbm:lc) never reached the client.
        net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(composite);
        JsonArray songs = new JsonArray();
        composite.lookupOrThrow(net.minecraft.core.registries.Registries.JUKEBOX_SONG).keySet().stream().sorted()
            .forEach(id -> songs.add(id.toString()));
        report.add("jukeboxSongs", songs);
        JsonObject counts = new JsonObject();
        syncedElements.forEach((key, entries) -> counts.addProperty(key.identifier().toString(), entries.size()));
        report.add("syncedRegistryEntries", counts);
        JsonObject order = new JsonObject();
        syncedElements.forEach((key, entries) -> {
            JsonArray ids = new JsonArray();
            entries.forEach(entry -> ids.add(entry.id().toString()));
            order.add(key.identifier().toString(), ids);
        });
        report.add("syncedRegistryOrder", order);
        report.addProperty("syncedTagRegistries", syncedTags.size());
        report.addProperty("collectedGameRegistries", true);
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static Registry.PendingTags<?> pending(Registry registry, net.minecraft.tags.TagNetworkSerialization.NetworkPayload payload) {
        return registry.prepareTagReload(payload.resolve(registry));
    }

    /**
     * Picks the item into hotbar slot 0 as the creative inventory does, then right-clicks the east
     * face of the block at the given position until the server answers with the placed block.
     */
    private void placeAgainst(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, String itemName, int x, int y, int z) throws IOException {
        var item = BuiltInRegistries.ITEM.getValue(Identifier.parse(itemName));
        var expectedBlock = Block.byItem(item);
        if (item == null || expectedBlock == net.minecraft.world.level.block.Blocks.AIR) {
            throw new IllegalArgumentException("Probe item " + itemName + " is not a block item");
        }
        var target = new net.minecraft.core.BlockPos(x + 1, y, z);
        var against = new net.minecraft.core.BlockPos(x, y, z);
        var hit = new net.minecraft.world.phys.BlockHitResult(
            new net.minecraft.world.phys.Vec3(x + 1.0, y + 0.5, z + 0.5), net.minecraft.core.Direction.EAST, against, false);
        long deadline = System.currentTimeMillis() + 30000;
        long nextUse = 0;
        int sequence = 0;
        // The block beside the target has a menu; sneaking suppresses its use so the held block item is placed.
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerInputPacket(new net.minecraft.world.entity.player.Input(false, false, false, false, false, true, false)));
        socket.setSoTimeout(1000);
        while (System.currentTimeMillis() < deadline) {
            if (System.currentTimeMillis() >= nextUse) {
                sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, new net.minecraft.world.item.ItemStack(item)));
                sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(0));
                sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, hit, ++sequence));
                nextUse = System.currentTimeMillis() + 2000;
            }
            Packet<?> packet;
            try {
                packet = decode(clientbound, read());
            } catch (java.net.SocketTimeoutException idle) {
                continue;
            }
            if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
                sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            } else if (packet instanceof ClientboundPingPacket ping) {
                sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
            } else if (packet instanceof ClientboundChunkBatchFinishedPacket) {
                sendPacket(serverbound, new ServerboundChunkBatchReceivedPacket(20.0F));
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundBlockUpdatePacket update && update.getPos().equals(target)) {
                var placed = update.getBlockState();
                report.addProperty("placedState", Block.getId(placed));
                report.addProperty("placedBlock", BuiltInRegistries.BLOCK.getKey(placed.getBlock()).toString());
                if (placed.getBlock() == expectedBlock) {
                    menuProbe("placed", clientbound, serverbound, target, false);
                    breakAndDrop(clientbound, serverbound, item, target);
                    return;
                }
                // A stale air update from a refused attempt is not the answer yet.
                if (!placed.isAir()) throw new IllegalStateException("Placed " + placed + " but expected " + expectedBlock);
            }
        }
        throw new IllegalStateException("Server never confirmed " + itemName + " placed at " + target);
    }

    private int useSequence = 1000;

    /** Handles the keep-alive style packets any waiting loop has to answer; true when the packet was one of them. */
    private boolean housekeeping(ProtocolInfo<?> serverbound, Packet<?> packet) throws IOException {
        if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
            sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
        } else if (packet instanceof ClientboundPingPacket ping) {
            sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
        } else if (packet instanceof ClientboundChunkBatchFinishedPacket) {
            sendPacket(serverbound, new ServerboundChunkBatchReceivedPacket(20.0F));
        } else if (packet instanceof ClientboundDisconnectPacket disconnect) {
            throw new IllegalStateException("Disconnected: " + disconnect.reason().getString());
        } else if (packet instanceof ClientboundPlayerPositionPacket position) {
            // The server ignores movement until a teleport is confirmed.
            sendPacket(serverbound, new ServerboundAcceptTeleportationPacket(position.id()));
            return false;
        } else {
            return false;
        }
        return true;
    }

    private interface Seen {
        boolean see(Packet<?> packet) throws IOException;
    }

    /** Reads packets for up to the given time, answering housekeeping; stops early once the callback returns true. */
    private boolean pump(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, long millis, Seen seen) throws IOException {
        long deadline = System.currentTimeMillis() + millis;
        socket.setSoTimeout(500);
        while (System.currentTimeMillis() < deadline) {
            Packet<?> packet;
            try {
                packet = decode(clientbound, read());
            } catch (java.net.SocketTimeoutException idle) {
                continue;
            }
            if (packet == null || housekeeping(serverbound, packet)) continue;
            watchBlocks(packet);
            if (seen.see(packet)) return true;
        }
        return false;
    }

    /**
     * Right-clicks a block with a Java-owned menu, checks the open-screen payload, the content sync and, when asked,
     * clicks, throws and closes; every step must leave the connection alive.
     */
    private void menuProbe(String label, ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, net.minecraft.core.BlockPos pos, boolean clicks) throws IOException {
        var result = new JsonObject();
        if (!report.has("menus")) report.add("menus", new JsonObject());
        report.getAsJsonObject("menus").add(label, result);
        var hit = new net.minecraft.world.phys.BlockHitResult(
            new net.minecraft.world.phys.Vec3(pos.getX() + 0.5, pos.getY() + 1.0, pos.getZ() + 0.5), net.minecraft.core.Direction.UP, pos, false);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerInputPacket(net.minecraft.world.entity.player.Input.EMPTY));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(0));
        if (clicks) {
            // Something to click on: the console makes the player creative shortly after joining, so the grant repeats.
            for (int i = 0; i < 4; i++) {
                sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.COBBLESTONE, 64)));
                pump(clientbound, serverbound, 800, packet -> false);
            }
        }
        int[] containerId = {-1};
        int[] stateId = {0};
        java.util.List<net.minecraft.world.item.ItemStack> content = new ArrayList<>();
        JsonArray hbmPayloads = new JsonArray();
        // The server ignores use packets until the client counts as loaded, so the click repeats until a menu opens.
        for (int attempt = 0; attempt < 8 && containerId[0] <= 0; attempt++) {
            sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, hit, ++useSequence));
            pump(clientbound, serverbound, 2500, packet -> {
                if (packet instanceof ClientboundCustomPayloadPacket custom) {
                    if (custom.payload() instanceof net.neoforged.neoforge.network.payload.AdvancedOpenScreenPayload open) {
                        containerId[0] = open.windowId();
                        var key = BuiltInRegistries.MENU.getKey(open.menuType());
                        result.addProperty("menuType", String.valueOf(key));
                        result.addProperty("menuName", open.name().getString());
                        result.addProperty("extraDataBytes", open.additionalData().length);
                        // HBM's BlockMenuContext: the block position then the dispatch id.
                        var extra = new FriendlyByteBuf(Unpooled.wrappedBuffer(open.additionalData()));
                        result.addProperty("extraPos", extra.readBlockPos().toString());
                    } else if (custom.payload().type().id().getNamespace().equals("hbm")) {
                        hbmPayloads.add(custom.payload().type().id().toString());
                    }
                } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundOpenScreenPacket open) {
                    containerId[0] = open.getContainerId();
                    result.addProperty("menuType", String.valueOf(BuiltInRegistries.MENU.getKey(open.getType())));
                } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket set && set.containerId() == containerId[0] && containerId[0] > 0) {
                    content.addAll(set.items());
                    stateId[0] = set.stateId();
                    result.addProperty("contentPackets", 1);
                    return true;
                }
                return false;
            });
        }
        result.addProperty("opened", containerId[0] > 0);
        result.addProperty("contentSlots", content.size());
        if (containerId[0] <= 0) {
            System.out.println("menu " + label + ": no menu opened " + result);
            return;
        }
        System.out.println("menu " + label + ": " + result);
        // Keep reading a while: the radar's map arrives through HBM's own sync payloads.
        pump(clientbound, serverbound, 3000, packet -> {
            if (packet instanceof ClientboundCustomPayloadPacket custom && custom.payload().type().id().getNamespace().equals("hbm")) {
                hbmPayloads.add(custom.payload().type().id().toString());
            }
            return false;
        });
        result.add("hbmPayloads", hbmPayloads);
        if (clicks) {
            int slot = -1;
            for (int i = Math.max(0, content.size() - 36); i < content.size(); i++) {
                if (!content.get(i).isEmpty()) { slot = i; break; }
            }
            result.addProperty("clickedSlot", slot);
            if (slot >= 0) {
                final int target = slot;
                for (int round = 0; round < 2; round++) {
                    sendPacket(serverbound, clickPacket(containerId[0], stateId[0], target, 0, net.minecraft.world.inventory.ContainerInput.PICKUP));
                    boolean acked = pump(clientbound, serverbound, 5000, packet ->
                        packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket s && s.getContainerId() == containerId[0]
                            || packet instanceof net.minecraft.network.protocol.game.ClientboundSetCursorItemPacket);
                    result.addProperty("clickAck" + round, acked);
                }
                sendPacket(serverbound, clickPacket(containerId[0], stateId[0], target, 1, net.minecraft.world.inventory.ContainerInput.THROW));
                int[] thrown = {-1};
                pump(clientbound, serverbound, 6000, packet -> {
                    if (packet instanceof net.minecraft.network.protocol.game.ClientboundAddEntityPacket add && BuiltInRegistries.ENTITY_TYPE.getKey(add.getType()).toString().equals("minecraft:item")) {
                        thrown[0] = add.getId();
                        return true;
                    }
                    return false;
                });
                result.addProperty("thrownItemEntity", thrown[0]);
            }
        }
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundContainerClosePacket(containerId[0]));
        // A keep-alive exchange after the close proves the connection survived the menu.
        pump(clientbound, serverbound, 1500, packet -> false);
        result.addProperty("closed", true);
    }

    private static Packet<?> clickPacket(int container, int state, int slot, int button, net.minecraft.world.inventory.ContainerInput input) {
        return new net.minecraft.network.protocol.game.ServerboundContainerClickPacket(container, state, (short) slot, (byte) button, input,
            it.unimi.dsi.fastutil.ints.Int2ObjectMaps.emptyMap(), net.minecraft.network.HashedStack.EMPTY);
    }

    /** Picks, breaks the placed block, then drops the held modded item; a closed socket means the server died. */
    private void breakAndDrop(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, net.minecraft.world.item.Item item, net.minecraft.core.BlockPos target) throws IOException {
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPickItemFromBlockPacket(target, true));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
            net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK, target, net.minecraft.core.Direction.EAST, 1000));
        long deadline = System.currentTimeMillis() + 15000;
        boolean broken = false;
        while (System.currentTimeMillis() < deadline && !broken) {
            Packet<?> packet;
            try {
                packet = decode(clientbound, read());
            } catch (java.net.SocketTimeoutException idle) {
                continue;
            }
            if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
                sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundBlockUpdatePacket update && update.getPos().equals(target) && update.getBlockState().isAir()) {
                broken = true;
            }
        }
        if (!broken) throw new IllegalStateException("Server never confirmed the break at " + target);
        report.addProperty("broken", true);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, new net.minecraft.world.item.ItemStack(item)));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
            net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.DROP_ALL_ITEMS, net.minecraft.core.BlockPos.ZERO, net.minecraft.core.Direction.DOWN));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(-1, new net.minecraft.world.item.ItemStack(item)));
        long settle = System.currentTimeMillis() + 4000;
        while (System.currentTimeMillis() < settle) {
            try {
                Packet<?> packet = decode(clientbound, read());
                if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
                    sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
                }
            } catch (java.net.SocketTimeoutException idle) {
                // quiet is fine; EOF would have thrown
            }
        }
        report.addProperty("dropped", true);
    }

    /** Remembers the custom data of the gun in hotbar slot 0 as the server last sent it. */
    private void watchGunStack(Packet<?> packet) {
        net.minecraft.world.item.ItemStack stack = null;
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket slot && slot.getContainerId() == 0 && slot.getSlot() == 36) {
            stack = slot.getItem();
        } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetPlayerInventoryPacket slot && slot.slot() == 0) {
            stack = slot.contents();
        } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket content && content.containerId() == 0 && content.items().size() > 36) {
            stack = content.items().get(36);
        }
        if (stack == null || stack.isEmpty()) return;
        var gunType = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse("hbm:gun_state"));
        if (gunType != null && stack.get(gunType) != null) { if (!gunComponent) System.out.println("gun component " + stack.get(gunType)); gunComponent = true; }
        report.addProperty("gunComponent", gunComponent);
        var custom = stack.get(net.minecraft.core.component.DataComponents.CUSTOM_DATA);
        gunData = (custom == null ? "" : custom.copyTag().toString()) + (gunType != null && stack.get(gunType) != null ? " | " + stack.get(gunType) : "");
        if (!gunStates.contains(gunData)) { gunStates.add(gunData); System.out.println("gun state " + gunData); }
    }

    private int bulletId = -1;
    private int bulletMoves;
    private boolean bulletData;
    private boolean bulletRemoved;
    private boolean gunComponent;
    private boolean gunSound;
    private boolean gunParticles;
    private int zombieHurt;

    private static int entityIdOf(Packet<?> packet) {
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundEntityPositionSyncPacket sync) return sync.id();
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundTeleportEntityPacket teleport) return teleport.id();
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket data) return data.id();
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundMoveEntityPacket move) {
            try {
                var field = net.minecraft.network.protocol.game.ClientboundMoveEntityPacket.class.getDeclaredField("entityId");
                field.setAccessible(true);
                return field.getInt(move);
            } catch (ReflectiveOperationException error) { throw new IllegalStateException(error); }
        }
        return -1;
    }

    /** Notes the bullet entity's life, the gun's sound and particles and the typed gun component as the client sees them. */
    private void watchGunWorld(Packet<?> packet) {
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundAddEntityPacket add) {
            var key = BuiltInRegistries.ENTITY_TYPE.getKey(add.getType());
            if (key.getNamespace().equals("hbm") && bulletId < 0) {
                bulletId = add.getId();
                report.addProperty("bulletType", key.toString());
                System.out.println("play <- bullet " + key + " #" + bulletId + " at " + add.getX() + "," + add.getY() + "," + add.getZ());
            }
        } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundRemoveEntitiesPacket remove) {
            if (bulletId >= 0 && remove.getEntityIds().contains(bulletId)) { bulletRemoved = true; System.out.println("play <- bullet removed after " + bulletMoves + " moves"); }
        } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundSoundPacket || packet instanceof net.minecraft.network.protocol.game.ClientboundSoundEntityPacket) {
            gunSound = true;
        } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundLevelParticlesPacket || packet instanceof net.minecraft.network.protocol.game.ClientboundLevelEventPacket) {
            gunParticles = true;
        } else if (bulletId >= 0 && entityIdOf(packet) == bulletId) {
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket) bulletData = true;
            else bulletMoves++;
        } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundEntityEventPacket || packet instanceof net.minecraft.network.protocol.game.ClientboundHurtAnimationPacket) {
            zombieHurt++;
        }
        report.addProperty("bulletMoves", bulletMoves);
        report.addProperty("bulletData", bulletData);
        report.addProperty("bulletRemoved", bulletRemoved);
        report.addProperty("gunSound", gunSound);
        report.addProperty("gunParticles", gunParticles);
        report.addProperty("entityEvents", zombieHurt);
    }

    private String gunData;
    private final List<String> gunStates = new ArrayList<>();

    /**
     * Gives the client a modded gun, selects it, sends HBM's own keybind payloads built through the
     * client's registered codec and checks the server changed the stack and answered with HBM payloads.
     */
    @SuppressWarnings({"unchecked", "rawtypes"})
    private void gunProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, String gunName) throws Exception {
        var item = BuiltInRegistries.ITEM.getValue(Identifier.parse(gunName));
        if (item == null) throw new IllegalArgumentException("Unknown gun item " + gunName);
        var keybindClass = Class.forName("com.hbm.packet.toserver.KeybindPayload");
        var enumClass = (Class<? extends Enum>) Class.forName("com.hbm.handler.HbmKeybinds$EnumKeybind");
        var constructor = keybindClass.getConstructor(enumClass, boolean.class);
        JsonArray received = new JsonArray();
        report.add("gunPayloads", received);
        socket.setSoTimeout(500);
        var ammo = BuiltInRegistries.ITEM.getValue(Identifier.parse("hbm:ammo_standard_p9_fmj"));
        if (ammo != null) sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(37, new net.minecraft.world.item.ItemStack(ammo, 64)));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, new net.minecraft.world.item.ItemStack(item)));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(0));
        String[] keys = {"GUN_SECONDARY", "GUN_SECONDARY", "RELOAD", "RELOAD", "GUN_PRIMARY", "GUN_PRIMARY", "GUN_PRIMARY", "GUN_PRIMARY", "GUN_PRIMARY", "GUN_PRIMARY"};
        int sent = 0;
        long nextKey = System.currentTimeMillis() + 3000;
        long deadline = System.currentTimeMillis() + 40000;
        String initial = null;
        while (System.currentTimeMillis() < deadline) {
            if (sent < keys.length && System.currentTimeMillis() >= nextKey) {
                if (initial == null) initial = gunData;
                var payload = (CustomPacketPayload) constructor.newInstance(Enum.valueOf(enumClass, keys[sent]), sent % 2 == 0);
                sendPacket(serverbound, new ServerboundCustomPayloadPacket(payload));
                sent++;
                nextKey = System.currentTimeMillis() + 2000;
            }
            Packet<?> packet;
            try {
                packet = decode(clientbound, read());
            } catch (java.net.SocketTimeoutException idle) {
                continue;
            }
            if (packet == null) continue;
            if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
                sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            } else if (packet instanceof ClientboundPingPacket ping) {
                sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
            } else if (packet instanceof ClientboundChunkBatchFinishedPacket) {
                sendPacket(serverbound, new ServerboundChunkBatchReceivedPacket(20.0F));
            } else if (packet instanceof ClientboundCustomPayloadPacket custom && custom.payload().type().id().getNamespace().equals("hbm")) {
                received.add(custom.payload().type().id().toString());
            } else if (packet instanceof ClientboundDisconnectPacket disconnect) {
                throw new IllegalStateException("Disconnected during the gun probe: " + disconnect.reason().getString());
            } else {
                watchGunStack(packet);
                watchGunWorld(packet);
            }
            if (sent == keys.length && bulletRemoved && gunSound && gunComponent && gunData != null && !gunData.equals(initial) && report.has("zombieDamaged")) break;
        }
        System.out.println("DUMP 0");
        System.out.println("DUMP 1");
        pump(clientbound, serverbound, 1500);
        report.addProperty("gunInitialData", String.valueOf(initial));
        report.addProperty("gunData", String.valueOf(gunData));
        report.addProperty("gunStateChanged", gunData != null && !gunData.equals(initial));
        report.addProperty("gunKeybindsSent", sent);
        report.addProperty("gunStates", String.join(" | ", gunStates));
    }


    private void watchArmorStack(net.minecraft.world.item.ItemStack stack) {
        if (stack == null || stack.isEmpty()) return;
        var energyType = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse("draconicnext:portable_energy"));
        var shieldType = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse("draconicnext:armor_shield"));
        report.addProperty("armorSeen", BuiltInRegistries.ITEM.getKey(stack.getItem()).toString());
        try {
            var energy = energyType == null ? null : stack.get(energyType);
            if (energy != null) report.addProperty("armorEnergy", (long) energy.getClass().getMethod("energy").invoke(energy));
            var shield = shieldType == null ? null : stack.get(shieldType);
            if (shield != null) {
                float points = (float) shield.getClass().getMethod("points").invoke(shield);
                if (points > (report.has("armorShieldMax") ? report.get("armorShieldMax").getAsFloat() : 0.0F)) report.addProperty("armorShieldMax", points);
            }
        } catch (ReflectiveOperationException error) { throw new IllegalStateException(error); }
    }

    /** Equips charged Draconic armor through the creative slot packet and checks the server keeps its charge and charges its shield. */
    private void armorProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, String itemName) throws Exception {
        var item = BuiltInRegistries.ITEM.getValue(Identifier.parse(itemName));
        var energyType = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse("draconicnext:portable_energy"));
        var dataClass = Class.forName("net.solarspire.draconicnext.common.item.PortableEnergyData");
        var charged = dataClass.getConstructor(long.class, int.class, int.class).newInstance(10_000_000L, 0, 0);
        var stack = new net.minecraft.world.item.ItemStack(item);
        stack.set((net.minecraft.core.component.DataComponentType) energyType, charged);
        socket.setSoTimeout(500);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(6, stack));
        long deadline = System.currentTimeMillis() + 8000;
        while (System.currentTimeMillis() < deadline) {
            Packet<?> packet;
            try { packet = decode(clientbound, read()); } catch (java.net.SocketTimeoutException idle) { continue; }
            if (packet == null) continue;
            if (packet instanceof ClientboundKeepAlivePacket keepAlive) sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            else if (packet instanceof ClientboundPingPacket ping) sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
            else if (packet instanceof ClientboundChunkBatchFinishedPacket) sendPacket(serverbound, new ServerboundChunkBatchReceivedPacket(20.0F));
            else if (packet instanceof ClientboundDisconnectPacket disconnect) throw new IllegalStateException("Disconnected during the armor probe: " + disconnect.reason().getString());
            else if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket slot && slot.getContainerId() == 0 && slot.getSlot() == 6) watchArmorStack(slot.getItem());
            else if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetPlayerInventoryPacket slot && slot.slot() == 38) watchArmorStack(slot.contents());
            else if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket content && content.containerId() == 0 && content.items().size() > 6) watchArmorStack(content.items().get(6));
            else if (packet instanceof ClientboundCustomPayloadPacket custom && custom.payload().type().id().toString().equals("draconicnext:shield_state")) report.addProperty("armorShieldPayload", true);
        }
        // A hit while the shield is charged must be absorbed by it: health stays up and the shield reports the hit.
        float[] health = {-1.0F};
        command(serverbound, "gamemode survival NeoProbe");
        pump(clientbound, serverbound, 500);
        command(serverbound, "damage NeoProbe 4 minecraft:generic");
        deadline = System.currentTimeMillis() + 4000;
        while (System.currentTimeMillis() < deadline) {
            Packet<?> packet;
            try { packet = decode(clientbound, read()); } catch (java.net.SocketTimeoutException idle) { continue; }
            if (packet == null) continue;
            if (packet instanceof ClientboundKeepAlivePacket keepAlive) sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            else if (packet instanceof ClientboundPingPacket ping) sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
            else if (packet instanceof ClientboundChunkBatchFinishedPacket) sendPacket(serverbound, new ServerboundChunkBatchReceivedPacket(20.0F));
            else if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetHealthPacket set) health[0] = set.getHealth();
            else if (packet instanceof ClientboundCustomPayloadPacket custom && custom.payload().type().id().toString().equals("draconicnext:shield_hit")) report.addProperty("armorShieldHit", true);
        }
        report.addProperty("armorHealthAfterHit", health[0]);
        // The test script reads the server's copy of the stacks once it sees this marker.
        System.out.println("DUMP 38");
        pump(clientbound, serverbound, 4000);
        report.addProperty("armorProbed", true);
    }


    /** Reads and answers keep-alives for the given time so the server keeps ticking the player. */
    private void pump(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, long millis) throws IOException {
        long end = System.currentTimeMillis() + millis;
        socket.setSoTimeout(300);
        while (System.currentTimeMillis() < end) {
            Packet<?> packet;
            try { packet = decode(clientbound, read()); } catch (java.net.SocketTimeoutException idle) { continue; }
            if (packet instanceof ClientboundKeepAlivePacket keepAlive) sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            else if (packet instanceof ClientboundPingPacket ping) sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
            else if (packet instanceof ClientboundChunkBatchFinishedPacket) sendPacket(serverbound, new ServerboundChunkBatchReceivedPacket(20.0F));
            else if (packet instanceof ClientboundDisconnectPacket disconnect) throw new IllegalStateException("Disconnected: " + disconnect.reason().getString());
            else if (packet instanceof ClientboundPlayerPositionPacket position) sendPacket(serverbound, new ServerboundAcceptTeleportationPacket(position.id()));
        }
    }

    /** A jab with a charged mod spear must lunge the player: the server pushes the player along their look direction. */
    private void spearProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, String spearName) throws Exception {
        var item = BuiltInRegistries.ITEM.getValue(Identifier.parse(spearName));
        var energyType = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse("draconicnext:portable_energy"));
        var dataClass = Class.forName("net.solarspire.draconicnext.common.item.PortableEnergyData");
        var stack = new net.minecraft.world.item.ItemStack(item);
        stack.set((net.minecraft.core.component.DataComponentType) energyType, dataClass.getConstructor(long.class, int.class, int.class).newInstance(10_000_000L, 0, 0));
        Class.forName("net.solarspire.draconicnext.common.tools.ConfigurableItem")
            .getMethod("set", net.minecraft.world.item.ItemStack.class, String.class, double.class).invoke(null, stack, "lungeLevel", 3.0);
        command(serverbound, "gamemode creative NeoProbe");
        pump(clientbound, serverbound, 800);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, stack));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(0));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.Rot(0.0F, -30.0F, true, false));
        pump(clientbound, serverbound, 1000);
        command(serverbound, "gamemode survival NeoProbe");
        pump(clientbound, serverbound, 1000);
        double[] push = {0.0};
        Seen watcher = packet -> {
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetEntityMotionPacket motion) {
                var delta = motion.movement();
                push[0] = Math.max(push[0], Math.sqrt(delta.x * delta.x + delta.y * delta.y + delta.z * delta.z));
            }
            return false;
        };
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
            net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.STAB, net.minecraft.core.BlockPos.ZERO, net.minecraft.core.Direction.DOWN, ++useSequence));
        pump(clientbound, serverbound, 3000, watcher);
        report.addProperty("spearLunge", push[0]);
    }

    /** A mod recipe in the player's 2x2 grid: Rust's crafting falls back to Java's recipe manager for the result. */
    /** Block states the client was told about, by position, from single and section updates. */
    private final Map<net.minecraft.core.BlockPos, net.minecraft.world.level.block.state.BlockState> seenStates = new HashMap<>();

    private void watchBlocks(Packet<?> packet) {
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundBlockUpdatePacket update) {
            seenStates.put(update.getPos().immutable(), update.getBlockState());
        } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundSectionBlocksUpdatePacket section) {
            section.runUpdates((pos, state) -> seenStates.put(pos.immutable(), state));
        }
    }

    private static String blockName(net.minecraft.world.level.block.state.BlockState state) {
        return BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString();
    }

    /** Moves a stack inside an open menu with two left clicks, the way a player drags it. */
    private void moveInMenu(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, OpenMenu menu, int from, int to) throws IOException {
        sendPacket(serverbound, clickPacket(menu.id(), menu.stateId(), from, 0, net.minecraft.world.inventory.ContainerInput.PICKUP));
        pump(clientbound, serverbound, 400, packet -> false);
        sendPacket(serverbound, clickPacket(menu.id(), menu.stateId(), to, 0, net.minecraft.world.inventory.ContainerInput.PICKUP));
        pump(clientbound, serverbound, 400, packet -> false);
    }

    private net.minecraft.world.phys.BlockHitResult topHit(net.minecraft.core.BlockPos pos) {
        return new net.minecraft.world.phys.BlockHitResult(new net.minecraft.world.phys.Vec3(pos.getX() + 0.5, pos.getY() + 1.0, pos.getZ() + 0.5), net.minecraft.core.Direction.UP, pos, false);
    }

    /**
     * HBM machines in real play: a powered electric furnace smelts through its menu, a multiblock placed from its item
     * fills its cells, opens its menu from a cell and tears down when the core breaks, and a battery feeds a furnace
     * through a cable.
     */
    private void machinesProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound) throws Exception {
        var result = new JsonObject();
        report.add("machines", result);
        if ("restart".equals(System.getenv("FERRUM_PROBE_MACHINES"))) {
            machinesAfterRestart(clientbound, serverbound, result);
            return;
        }
        socket.setSoTimeout(500);
        command(serverbound, "gamemode creative @s");
        command(serverbound, "fill -4 99 -2 4 99 10 minecraft:stone");
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerInputPacket(net.minecraft.world.entity.player.Input.EMPTY));
        pump(clientbound, serverbound, 1500, packet -> false);
        var item = (java.util.function.Function<String, net.minecraft.world.item.Item>) name -> BuiltInRegistries.ITEM.getValue(Identifier.parse(name));

        // 1. Electric furnace at 0,100,0: creative battery and raw iron in, an iron ingot out.
        var furnace = new net.minecraft.core.BlockPos(0, 100, 0);
        var furnaceResult = new JsonObject();
        result.add("furnace", furnaceResult);
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(item.apply("hbm:battery_creative")), new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.RAW_IRON, 4));
        var menu = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(furnace), ++useSequence));
        furnaceResult.addProperty("opened", menu.id() > 0);
        if (menu.id() > 0) {
            int hotbar = menu.items().size() - 9;
            furnaceResult.addProperty("containerSlots", hotbar - 27);
            moveInMenu(clientbound, serverbound, menu, hotbar, 0);
            moveInMenu(clientbound, serverbound, menu, hotbar + 1, 1);
            int[] data = {0};
            JsonArray payloads = new JsonArray();
            boolean smelted = pump(clientbound, serverbound, 20000, packet -> {
                if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetDataPacket set && set.getContainerId() == menu.id()) data[0]++;
                if (packet instanceof ClientboundCustomPayloadPacket custom && custom.payload().type().id().getNamespace().equals("hbm") && payloads.size() < 16) payloads.add(custom.payload().type().id().toString());
                if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket slot && slot.getContainerId() == menu.id() && slot.getSlot() == 2) {
                    return slot.getItem().is(net.minecraft.world.item.Items.IRON_INGOT);
                }
                if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket set && set.containerId() == menu.id() && set.items().size() > 2) {
                    return set.items().get(2).is(net.minecraft.world.item.Items.IRON_INGOT);
                }
                return false;
            });
            furnaceResult.addProperty("smelted", smelted);
            furnaceResult.addProperty("dataPackets", data[0]);
            furnaceResult.add("hbmPayloads", payloads);
            closeMenu(clientbound, serverbound, menu);
        }
        System.out.println("machines furnace: " + furnaceResult);

        // 2. Multiblock: the assembly machine placed on the floor south of the player.
        var multi = new JsonObject();
        result.add("multiblock", multi);
        var machineItem = item.apply("hbm:machine_assembly_machine");
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(machineItem), net.minecraft.world.item.ItemStack.EMPTY);
        var floor = new net.minecraft.core.BlockPos(0, 99, 6);
        seenStates.clear();
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(floor), ++useSequence));
        pump(clientbound, serverbound, 4000, packet -> { watchBlocks(packet); return false; });
        var cells = new ArrayList<net.minecraft.core.BlockPos>();
        net.minecraft.core.BlockPos core = null;
        for (var entry : seenStates.entrySet()) {
            var offset = entry.getKey().subtract(floor);
            if (!blockName(entry.getValue()).startsWith("hbm:") || Math.abs(offset.getX()) > 3 || offset.getY() < 1 || offset.getY() > 4 || offset.getZ() < -1 || offset.getZ() > 4) continue;
            cells.add(entry.getKey());
            if (entry.getValue().getBlock() == Block.byItem(machineItem)) core = entry.getKey();
        }
        multi.addProperty("hbmBlocks", cells.size());
        multi.addProperty("core", String.valueOf(core));
        if (core != null) multi.addProperty("coreState", seenStates.get(core).toString());
        net.minecraft.core.BlockPos cell = null;
        for (var pos : cells) if (!pos.equals(core) && (cell == null || pos.getY() > cell.getY())) cell = pos;
        multi.addProperty("cell", String.valueOf(cell));
        if (cell != null) {
            giveHotbar(clientbound, serverbound, net.minecraft.world.item.ItemStack.EMPTY, net.minecraft.world.item.ItemStack.EMPTY);
            final var at = cell;
            var cellMenu = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(at), ++useSequence));
            multi.addProperty("cellMenuOpened", cellMenu.id() > 0);
            multi.addProperty("cellMenuSlots", cellMenu.items().size());
            closeMenu(clientbound, serverbound, cellMenu);
        }
        if (core != null) {
            seenStates.clear();
            sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
                net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK, core, net.minecraft.core.Direction.UP, ++useSequence));
            pump(clientbound, serverbound, 4000, packet -> { watchBlocks(packet); return false; });
            int cleared = 0;
            var left = new JsonArray();
            for (var pos : cells) {
                var state = seenStates.get(pos);
                if (state != null && !blockName(state).startsWith("hbm:")) cleared++;
                else left.add(pos.toShortString() + "=" + (state == null ? "no update" : blockName(state)));
            }
            multi.addProperty("clearedOnBreak", cleared);
            multi.add("leftBehind", left);
            // Placed again and kept, so the restart check can open it from a cell.
            giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(machineItem), net.minecraft.world.item.ItemStack.EMPTY);
            sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(floor), ++useSequence));
            pump(clientbound, serverbound, 3000, packet -> false);
        }
        System.out.println("machines multiblock: " + multi);

        // 3. Energy: a battery block next to a cable that runs into a second electric furnace.
        var energy = new JsonObject();
        result.add("energy", energy);
        var furnace2 = new net.minecraft.core.BlockPos(-3, 100, 2);
        var cablePos = new net.minecraft.core.BlockPos(-3, 100, 1);
        var battery = new net.minecraft.core.BlockPos(-3, 100, 0);
        command(serverbound, "setblock -3 100 2 hbm:machine_electric_furnace");
        command(serverbound, "setblock -3 100 1 hbm:cable");
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(item.apply("hbm:machine_battery")), net.minecraft.world.item.ItemStack.EMPTY);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(battery.below()), ++useSequence));
        pump(clientbound, serverbound, 1500, packet -> false);
        energy.addProperty("cablePos", cablePos.toString());
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(item.apply("hbm:battery_creative")), new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.RAW_IRON, 4));
        var batteryMenu = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(battery), ++useSequence));
        energy.addProperty("batteryOpened", batteryMenu.id() > 0);
        if (batteryMenu.id() > 0) {
            moveInMenu(clientbound, serverbound, batteryMenu, batteryMenu.items().size() - 9, 0);
            closeMenu(clientbound, serverbound, batteryMenu);
        }
        // A battery provides only while powered in its default modes, as a player would set it with redstone.
        command(serverbound, "setblock -4 100 0 minecraft:redstone_block");
        pump(clientbound, serverbound, 1000, packet -> false);
        var furnaceMenu = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(furnace2), ++useSequence));
        energy.addProperty("furnaceOpened", furnaceMenu.id() > 0);
        if (furnaceMenu.id() > 0) {
            moveInMenu(clientbound, serverbound, furnaceMenu, furnaceMenu.items().size() - 8, 1);
            boolean smelted = pump(clientbound, serverbound, 20000, packet ->
                packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket slot && slot.getContainerId() == furnaceMenu.id() && slot.getSlot() == 2
                    && slot.getItem().is(net.minecraft.world.item.Items.IRON_INGOT));
            energy.addProperty("smeltedOnCable", smelted);
            energy.addProperty("cableState", String.valueOf(seenStates.get(cablePos)));
            energy.addProperty("batteryState", String.valueOf(seenStates.get(battery)));
            closeMenu(clientbound, serverbound, furnaceMenu);
        }
        System.out.println("machines energy: " + energy);

        // 4. Vanilla hoppers feed the powered furnace from above and pull its output from below, through its item capability.
        var hoppers = new JsonObject();
        result.add("hoppers", hoppers);
        command(serverbound, "setblock 0 99 0 minecraft:hopper");
        command(serverbound, "setblock 0 101 0 minecraft:hopper");
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.RAW_IRON, 3), net.minecraft.world.item.ItemStack.EMPTY);
        var top = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(new net.minecraft.core.BlockPos(0, 101, 0)), ++useSequence));
        hoppers.addProperty("topOpened", top.id() > 0);
        if (top.id() > 0) {
            moveInMenu(clientbound, serverbound, top, top.items().size() - 9, 0);
            closeMenu(clientbound, serverbound, top);
        }
        pump(clientbound, serverbound, 15000, packet -> false);
        var bottom = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND,
            new net.minecraft.world.phys.BlockHitResult(new net.minecraft.world.phys.Vec3(0.5, 99.5, 1.0), net.minecraft.core.Direction.SOUTH, new net.minecraft.core.BlockPos(0, 99, 0), false), ++useSequence));
        hoppers.addProperty("bottomOpened", bottom.id() > 0);
        hoppers.addProperty("pulledIngots", bottom.id() > 0 ? countOf(bottom.items(), 0, 5, net.minecraft.world.item.Items.IRON_INGOT) : -1);
        closeMenu(clientbound, serverbound, bottom);
        var topAgain = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(new net.minecraft.core.BlockPos(0, 101, 0)), ++useSequence));
        hoppers.addProperty("rawLeftOnTop", topAgain.id() > 0 ? countOf(topAgain.items(), 0, 5, net.minecraft.world.item.Items.RAW_IRON) : -1);
        closeMenu(clientbound, serverbound, topAgain);
        System.out.println("machines hoppers: " + hoppers);
    }

    /** The world the machines probe left behind, after a server restart. */
    private void machinesAfterRestart(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, JsonObject result) throws Exception {
        var restart = new JsonObject();
        result.add("restart", restart);
        socket.setSoTimeout(500);
        pump(clientbound, serverbound, 3000, packet -> false);
        giveHotbar(clientbound, serverbound, net.minecraft.world.item.ItemStack.EMPTY, net.minecraft.world.item.ItemStack.EMPTY);
        var cell = new net.minecraft.core.BlockPos(-1, 102, 7);
        var cellMenu = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(cell), ++useSequence));
        restart.addProperty("cellMenuOpened", cellMenu.id() > 0);
        closeMenu(clientbound, serverbound, cellMenu);
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.RAW_IRON, 2), net.minecraft.world.item.ItemStack.EMPTY);
        var furnace = new net.minecraft.core.BlockPos(-3, 100, 2);
        var menu = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, topHit(furnace), ++useSequence));
        restart.addProperty("furnaceOpened", menu.id() > 0);
        if (menu.id() > 0) {
            int kept = countOf(menu.items(), 2, 3, net.minecraft.world.item.Items.IRON_INGOT);
            restart.addProperty("keptIngots", kept);
            moveInMenu(clientbound, serverbound, menu, menu.items().size() - 9, 1);
            boolean smelted = pump(clientbound, serverbound, 20000, packet ->
                packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket slot && slot.getContainerId() == menu.id() && slot.getSlot() == 2
                    && slot.getItem().is(net.minecraft.world.item.Items.IRON_INGOT) && slot.getItem().getCount() > kept);
            restart.addProperty("smeltedOnCable", smelted);
            closeMenu(clientbound, serverbound, menu);
        }
        System.out.println("machines restart: " + restart);
    }

    private void craftProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound) throws Exception {
        var input = BuiltInRegistries.ITEM.getValue(Identifier.parse("hbm:asphalt_light"));
        var output = BuiltInRegistries.ITEM.getValue(Identifier.parse("hbm:asphalt"));
        command(serverbound, "gamemode creative NeoProbe");
        pump(clientbound, serverbound, 800);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, new net.minecraft.world.item.ItemStack(input, 4)));
        pump(clientbound, serverbound, 500);
        command(serverbound, "gamemode survival NeoProbe");
        pump(clientbound, serverbound, 1000);
        boolean[] crafted = {false};
        Seen watcher = packet -> {
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket slot && slot.getContainerId() == 0
                && slot.getSlot() == 0 && slot.getItem().is(output)) crafted[0] = true;
            return crafted[0];
        };
        sendPacket(serverbound, clickPacket(0, 0, 36, 0, net.minecraft.world.inventory.ContainerInput.PICKUP));
        pump(clientbound, serverbound, 500);
        sendPacket(serverbound, clickPacket(0, 0, 1, 1, net.minecraft.world.inventory.ContainerInput.PICKUP));
        pump(clientbound, serverbound, 3000, watcher);
        report.addProperty("modCraft", crafted[0]);
        // Everything goes back to the hotbar.
        sendPacket(serverbound, clickPacket(0, 0, 1, 0, net.minecraft.world.inventory.ContainerInput.PICKUP));
        sendPacket(serverbound, clickPacket(0, 0, 36, 0, net.minecraft.world.inventory.ContainerInput.PICKUP));
        pump(clientbound, serverbound, 800);
    }

    /** A creative slot holding an enchanted stack (client enchantment IDs) must reach the server without a disconnect. */
    private void enchantProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound) throws Exception {
        command(serverbound, "gamemode creative NeoProbe");
        pump(clientbound, serverbound, 800);
        var stack = new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.MACE);
        stack.enchant(gameRegistries.lookupOrThrow(net.minecraft.core.registries.Registries.ENCHANTMENT)
            .getOrThrow(net.minecraft.world.item.enchantment.Enchantments.WIND_BURST), 3);
        var book = new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.ENCHANTED_BOOK);
        book.set(net.minecraft.core.component.DataComponents.STORED_ENCHANTMENTS, stack.getEnchantments());
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(38, stack));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(39, book));
        pump(clientbound, serverbound, 1500);
        command(serverbound, "gamemode survival NeoProbe");
        pump(clientbound, serverbound, 800);
        report.addProperty("creativeEnchant", true);
    }

    /** Right-clicking a charged Draconic mace in the air must run its Java use: a meteor strike with a cooldown and motion. */
    private void maceProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound) throws Exception {
        var item = BuiltInRegistries.ITEM.getValue(Identifier.parse("draconicnext:draconic_mace"));
        var energyType = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse("draconicnext:portable_energy"));
        var dataClass = Class.forName("net.solarspire.draconicnext.common.item.PortableEnergyData");
        var stack = new net.minecraft.world.item.ItemStack(item);
        stack.set((net.minecraft.core.component.DataComponentType) energyType, dataClass.getConstructor(long.class, int.class, int.class).newInstance(10_000_000L, 0, 0));
        command(serverbound, "gamemode creative NeoProbe");
        pump(clientbound, serverbound, 800);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, stack));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(0));
        command(serverbound, "gamemode survival NeoProbe");
        command(serverbound, "tp NeoProbe 0.5 112 0.5");
        pump(clientbound, serverbound, 1500);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.PosRot(0.5, 112.0, 0.5, 0.0F, 60.0F, false, false));
        pump(clientbound, serverbound, 500);
        boolean[] seen = {false, false};
        Seen watcher = packet -> {
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundCooldownPacket) seen[0] = true;
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundSoundPacket sound && sound.getSound().value().location().getPath().equals("item.firecharge.use")) seen[1] = true;
            return seen[0] && seen[1];
        };
        // Falling: the meteor strike only starts in the air.
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.PosRot(0.5, 111.5, 0.5, 0.0F, 60.0F, false, false));
        pump(clientbound, serverbound, 300);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundUseItemPacket(net.minecraft.world.InteractionHand.MAIN_HAND, ++useSequence, 0.0F, 60.0F));
        pump(clientbound, serverbound, 3000, watcher);
        report.addProperty("maceCooldown", seen[0]);
        report.addProperty("maceSound", seen[1]);
        command(serverbound, "tp NeoProbe 0.5 101 0.5");
        pump(clientbound, serverbound, 1500);
    }

    /** Mines a stone block in survival with a charged mod tool; area tools must clear their whole area and drop items. */
    private void toolProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, String toolName) throws Exception {
        var item = BuiltInRegistries.ITEM.getValue(Identifier.parse(toolName));
        var energyType = BuiltInRegistries.DATA_COMPONENT_TYPE.getValue(Identifier.parse("draconicnext:portable_energy"));
        var dataClass = Class.forName("net.solarspire.draconicnext.common.item.PortableEnergyData");
        var stack = new net.minecraft.world.item.ItemStack(item);
        stack.set((net.minecraft.core.component.DataComponentType) energyType, dataClass.getConstructor(long.class, int.class, int.class).newInstance(10_000_000L, 0, 0));
        var configured = Class.forName("net.solarspire.draconicnext.common.tools.ConfigurableItem")
            .getMethod("set", net.minecraft.world.item.ItemStack.class, String.class, double.class).invoke(null, stack, "digAOE", 1.0);
        report.addProperty("toolConfigured", String.valueOf(configured));
        command(serverbound, "gamemode creative NeoProbe");
        command(serverbound, "fill 2 99 0 6 103 4 minecraft:stone");
        pump(clientbound, serverbound, 1500);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, stack));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(0));
        pump(clientbound, serverbound, 800);
        command(serverbound, "gamemode survival NeoProbe");
        pump(clientbound, serverbound, 1500);
        var target = new net.minecraft.core.BlockPos(2, 101, 2);
        // The area tool works out its face from where the player looks.
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundMovePlayerPacket.Rot(-90.0F, 29.2F, true, false));
        pump(clientbound, serverbound, 500);
        var broken = new java.util.HashSet<net.minecraft.core.BlockPos>();
        int[] drops = {0};
        Seen watcher = packet -> {
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundBlockUpdatePacket update) {
                if (update.getBlockState().isAir()) broken.add(update.getPos().immutable());
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundSectionBlocksUpdatePacket section) {
                section.runUpdates((pos, state) -> { if (state.isAir()) broken.add(pos.immutable()); });
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundAddEntityPacket add && add.getType() == BuiltInRegistries.ENTITY_TYPE.get(Identifier.parse("minecraft:item")).map(h -> h.value()).orElse(null)) {
                drops[0]++;
            }
            return false;
        };
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
            net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK, target, net.minecraft.core.Direction.WEST, ++useSequence));
        pump(clientbound, serverbound, 1500, watcher);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
            net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.STOP_DESTROY_BLOCK, target, net.minecraft.core.Direction.WEST, ++useSequence));
        pump(clientbound, serverbound, 4000, watcher);
        System.out.println("DUMP 0");
        pump(clientbound, serverbound, 1500);
        report.addProperty("toolTargetBroken", broken.contains(target));
        report.addProperty("toolBlocksBroken", broken.size());
        report.addProperty("toolItemDrops", drops[0]);
    }

    /** Hits a zombie next to the player with a fist and then a mod sword; the sword must hurt it more. */
    private void meleeProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, String swordName) throws Exception {
        var sword = BuiltInRegistries.ITEM.getValue(Identifier.parse(swordName));
        for (int wait = 0; meleeZombieId < 0 && wait < 20; wait++) pump(clientbound, serverbound, 500);
        report.addProperty("meleeZombieId", meleeZombieId);
        if (meleeZombieId < 0) return;
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(8));
        pump(clientbound, serverbound, 1500);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundAttackPacket(meleeZombieId));
        pump(clientbound, serverbound, 1500);
        float afterFist = meleeHealth;
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, new net.minecraft.world.item.ItemStack(sword)));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(0));
        pump(clientbound, serverbound, 1000);
        System.out.println("DUMP 0");
        pump(clientbound, serverbound, 1500);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundAttackPacket(meleeZombieId));
        pump(clientbound, serverbound, 1500);
        report.addProperty("meleeFistDamage", 20.0F - afterFist);
        report.addProperty("meleeSwordDamage", afterFist - meleeHealth);
    }

    private record OpenMenu(int id, int stateId, List<net.minecraft.world.item.ItemStack> items) {}

    /** Sends the trigger until the server opens a menu and syncs its content; id stays -1 when none opened. */
    private OpenMenu openMenu(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, java.util.function.Supplier<Packet<?>> trigger) throws IOException {
        int[] id = {-1};
        int[] state = {0};
        List<net.minecraft.world.item.ItemStack> items = new ArrayList<>();
        for (int attempt = 0; attempt < 8 && id[0] <= 0; attempt++) {
            sendPacket(serverbound, trigger.get());
            pump(clientbound, serverbound, 2500, packet -> {
                if (packet instanceof ClientboundCustomPayloadPacket custom && custom.payload() instanceof net.neoforged.neoforge.network.payload.AdvancedOpenScreenPayload open) {
                    id[0] = open.windowId();
                } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundOpenScreenPacket open) {
                    id[0] = open.getContainerId();
                } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket set && id[0] > 0 && set.containerId() == id[0]) {
                    items.addAll(set.items());
                    state[0] = set.stateId();
                    return true;
                }
                return false;
            });
        }
        return new OpenMenu(id[0], state[0], items);
    }

    private static int countOf(List<net.minecraft.world.item.ItemStack> items, int from, int to, net.minecraft.world.item.Item item) {
        int count = 0;
        for (int i = from; i < to && i < items.size(); i++) if (items.get(i).is(item)) count += items.get(i).getCount();
        return count;
    }

    private static boolean holdsStone(net.minecraft.world.item.ItemStack stack) {
        var contents = stack.get(net.minecraft.core.component.DataComponents.CONTAINER);
        return contents != null && contents.nonEmptyItemCopyStream().anyMatch(inner -> inner.is(net.minecraft.world.item.Items.COBBLESTONE));
    }

    private void command(ProtocolInfo<?> serverbound, String command) throws IOException {
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundChatCommandPacket(command));
    }

    private void giveHotbar(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, net.minecraft.world.item.ItemStack first, net.minecraft.world.item.ItemStack second) throws IOException {
        for (int i = 0; i < 3; i++) {
            sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(36, first));
            sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCreativeModeSlotPacket(37, second));
            sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(0));
            pump(clientbound, serverbound, 500, packet -> false);
        }
    }

    /** Moves the cobblestone stack of the player's hotbar into the open crate and closes the menu; true when the crate showed it. */
    private boolean fillCrateAndClose(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, OpenMenu menu) throws IOException {
        int crateSlots = menu.items().size() - 36;
        int source = -1;
        for (int i = crateSlots; i < menu.items().size(); i++) if (menu.items().get(i).is(net.minecraft.world.item.Items.COBBLESTONE)) { source = i; break; }
        if (source < 0) return false;
        sendPacket(serverbound, clickPacket(menu.id(), menu.stateId(), source, 0, net.minecraft.world.inventory.ContainerInput.QUICK_MOVE));
        boolean inserted = pump(clientbound, serverbound, 5000, packet ->
            packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket slot && slot.getContainerId() == menu.id()
                && slot.getSlot() < crateSlots && slot.getItem().is(net.minecraft.world.item.Items.COBBLESTONE));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundContainerClosePacket(menu.id()));
        pump(clientbound, serverbound, 1500, packet -> false);
        return inserted;
    }

    private void closeMenu(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, OpenMenu menu) throws IOException {
        if (menu.id() <= 0) return;
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundContainerClosePacket(menu.id()));
        pump(clientbound, serverbound, 1000, packet -> false);
    }

    private int crateStone(OpenMenu menu) {
        return menu.id() > 0 ? countOf(menu.items(), 0, menu.items().size() - 36, net.minecraft.world.item.Items.COBBLESTONE) : -1;
    }

    /**
     * Crate acceptance: a held crate item opens and keeps what is put in across close and reopen; a placed crate block
     * does the same and keeps its contents through being broken, picked up and placed again.
     */
    private void crateProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, String crateName) throws Exception {
        var result = new JsonObject();
        report.add("crate", result);
        var crateItem = BuiltInRegistries.ITEM.getValue(Identifier.parse(crateName));
        var stone = net.minecraft.world.item.Items.COBBLESTONE;
        var crateBlock = new net.minecraft.core.BlockPos(3, 101, 3);
        var hitSupport = new net.minecraft.world.phys.BlockHitResult(new net.minecraft.world.phys.Vec3(3.5, 101.0, 3.5), net.minecraft.core.Direction.UP, crateBlock.below(), false);
        var hitCrate = new net.minecraft.world.phys.BlockHitResult(new net.minecraft.world.phys.Vec3(3.5, 102.0, 3.5), net.minecraft.core.Direction.UP, crateBlock, false);
        java.util.function.Supplier<Packet<?>> useHeld = () -> new net.minecraft.network.protocol.game.ServerboundUseItemPacket(net.minecraft.world.InteractionHand.MAIN_HAND, ++useSequence, 0.0F, 0.0F);
        java.util.function.Supplier<Packet<?>> useCrate = () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, hitCrate, ++useSequence);
        socket.setSoTimeout(500);
        // HBM opens a crate item on use only when the client reports that setting.
        var openHeld = Class.forName("com.hbm.packet.toserver.CrateOpenHeldPayload").getDeclaredConstructor(boolean.class);
        openHeld.setAccessible(true);
        sendPacket(serverbound, new ServerboundCustomPayloadPacket((CustomPacketPayload) openHeld.newInstance(true)));
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerInputPacket(net.minecraft.world.entity.player.Input.EMPTY));
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(crateItem), new net.minecraft.world.item.ItemStack(stone, 32));

        var held = openMenu(clientbound, serverbound, useHeld);
        result.addProperty("heldOpened", held.id() > 0);
        if (held.id() <= 0) return;
        result.addProperty("heldInserted", fillCrateAndClose(clientbound, serverbound, held));
        var heldAgain = openMenu(clientbound, serverbound, useHeld);
        result.addProperty("heldReopenedStone", crateStone(heldAgain));
        closeMenu(clientbound, serverbound, heldAgain);

        // The placed crate was set from the console beside the player; right-click it with an empty hand.
        giveHotbar(clientbound, serverbound, net.minecraft.world.item.ItemStack.EMPTY, new net.minecraft.world.item.ItemStack(stone, 32));
        var block = openMenu(clientbound, serverbound, useCrate);
        result.addProperty("blockOpened", block.id() > 0);
        if (block.id() <= 0) return;
        result.addProperty("blockInserted", fillCrateAndClose(clientbound, serverbound, block));
        var blockAgain = openMenu(clientbound, serverbound, useCrate);
        result.addProperty("blockReopenedStone", crateStone(blockAgain));
        closeMenu(clientbound, serverbound, blockAgain);

        // Break it as a fast survival player with a pickaxe, walk into the drop, then place the item again.
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.NETHERITE_PICKAXE), net.minecraft.world.item.ItemStack.EMPTY);
        command(serverbound, "gamemode survival");
        command(serverbound, "effect give @s minecraft:haste 600 255 true");
        pump(clientbound, serverbound, 1500, packet -> false);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
            net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK, crateBlock, net.minecraft.core.Direction.UP, ++useSequence));
        boolean broken = false;
        for (int attempt = 0; attempt < 6 && !broken; attempt++) {
            sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundPlayerActionPacket(
                net.minecraft.network.protocol.game.ServerboundPlayerActionPacket.Action.STOP_DESTROY_BLOCK, crateBlock, net.minecraft.core.Direction.UP, ++useSequence));
            broken = pump(clientbound, serverbound, 1000, packet ->
                packet instanceof net.minecraft.network.protocol.game.ClientboundBlockUpdatePacket update && update.getPos().equals(crateBlock) && update.getBlockState().isAir());
        }
        result.addProperty("blockBroken", broken);
        if (!broken) return;
        command(serverbound, "tp @s 3.5 101 3.5");
        int[] pickedSlot = {-1};
        pump(clientbound, serverbound, 6000, packet -> {
            net.minecraft.world.item.ItemStack stack = null;
            int slot = -1;
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetPlayerInventoryPacket inventory) { stack = inventory.contents(); slot = inventory.slot(); }
            else if (packet instanceof net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket set && set.getContainerId() == 0) { stack = set.getItem(); slot = set.getSlot() >= 36 ? set.getSlot() - 36 : set.getSlot(); }
            if (stack != null && stack.is(crateItem)) {
                result.addProperty("droppedCrateHasStone", holdsStone(stack));
                pickedSlot[0] = slot;
                return true;
            }
            return false;
        });
        result.addProperty("droppedCratePickedUp", pickedSlot[0] >= 0);
        if (pickedSlot[0] < 0) return;
        // Stand clear of the block position before placing.
        command(serverbound, "tp @s 0.5 101 2.5");
        pump(clientbound, serverbound, 1000, packet -> false);
        if (pickedSlot[0] <= 8) sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(pickedSlot[0]));
        boolean placed = false;
        for (int attempt = 0; attempt < 6 && !placed; attempt++) {
            sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, hitSupport, ++useSequence));
            placed = pump(clientbound, serverbound, 2000, packet ->
                packet instanceof net.minecraft.network.protocol.game.ClientboundBlockUpdatePacket update && update.getPos().equals(crateBlock) && !update.getBlockState().isAir());
        }
        result.addProperty("blockPlacedAgain", placed);
        if (!placed) return;
        pump(clientbound, serverbound, 1500, packet -> false);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(8));
        var placedAgain = openMenu(clientbound, serverbound, useCrate);
        result.addProperty("placedAgainStone", crateStone(placedAgain));
        closeMenu(clientbound, serverbound, placedAgain);
    }

    private int chunksSeen;
    private long playStart;

    /** Keeps reading until the client holds the chunks its view distance asks for, timing how long terrain took. */
    private void terrainProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, int wanted) throws IOException {
        pump(clientbound, serverbound, 120000, packet -> chunksSeen >= wanted);
        report.addProperty("terrainChunks", chunksSeen);
        report.addProperty("terrainMillis", System.currentTimeMillis() - playStart);
        System.out.println("terrain: " + chunksSeen + " chunks after " + (System.currentTimeMillis() - playStart) + " ms");
    }

    private void play(String blockName, int x, int y, int z, String placeItem, String gunItem, String crateItem) throws Exception {
        playStart = System.currentTimeMillis();
        RegistryAccess access = gameRegistries != null ? gameRegistries : RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        ProtocolInfo<?> clientbound = GameProtocols.CLIENTBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(access, ConnectionType.NEOFORGE));
        ProtocolInfo<?> serverbound = GameProtocols.SERVERBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(access, ConnectionType.NEOFORGE), () -> true);
        var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(blockName));
        if (block == null || BuiltInRegistries.BLOCK.getKey(block).toString().equals("minecraft:air")) {
            throw new IllegalArgumentException("Unknown probe block " + blockName);
        }
        int expected = Block.getId(block.defaultBlockState());
        report.addProperty("expectedState", expected);
        boolean sentModPayload = false;
        while (true) {
            FriendlyByteBuf raw = read();
            Packet<?> packet = decode(clientbound, raw);
            if (packet == null) continue;
            if (packet instanceof ClientboundLevelChunkWithLightPacket chunk) {
                System.out.println("play <- chunk " + chunk.getX() + "," + chunk.getZ());
            } else {
                System.out.println("play <- " + packet.type().id());
            }
            if (packet instanceof ClientboundDisconnectPacket disconnect) {
                throw new IllegalStateException("Play disconnect: " + disconnect.reason().getString());
            } else if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
                sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            } else if (packet instanceof ClientboundPingPacket ping) {
                sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
            } else if (packet instanceof ClientboundPlayerPositionPacket position) {
                sendPacket(serverbound, new ServerboundAcceptTeleportationPacket(position.id()));
                if (!sentModPayload) {
                    // A mod-channel payload the server has no handler for must not disconnect the client.
                    sendPacket(serverbound, new ServerboundCustomPayloadPacket(new DiscardedPayload(Identifier.fromNamespaceAndPath("hbm", "probe"))));
                    sentModPayload = true;
                }
            } else if (packet instanceof ClientboundChunkBatchFinishedPacket) {
                sendPacket(serverbound, new ServerboundChunkBatchReceivedPacket(20.0F));
            } else if (packet instanceof ClientboundLevelChunkWithLightPacket chunk && chunk.getX() == (x >> 4) && chunk.getZ() == (z >> 4)) {
                int actual = stateAt(chunk.getChunkData().getReadBuffer(), x, y, z);
                report.addProperty("actualState", actual);
                report.add("decodeFailures", decodeFailures);
                if (actual != expected) {
                    throw new IllegalStateException("Chunk state at " + x + "," + y + "," + z + " is " + actual + ", expected " + expected
                        + " (" + Block.BLOCK_STATE_REGISTRY.byId(actual) + ")");
                }
                observeEntityAndChat(clientbound, serverbound);
                if (placeItem != null && !placeItem.isEmpty()) {
                    menuProbe("block", clientbound, serverbound, new net.minecraft.core.BlockPos(x, y, z), true);
                    placeAgainst(clientbound, serverbound, placeItem, x, y, z);
                }
                if (gunItem != null && !gunItem.isEmpty()) meleeProbe(clientbound, serverbound, "draconicnext:draconic_sword");
                if (gunItem != null && !gunItem.isEmpty()) gunProbe(clientbound, serverbound, gunItem);
                if (gunItem != null && !gunItem.isEmpty()) armorProbe(clientbound, serverbound, "draconicnext:draconic_chest");
                if (gunItem != null && !gunItem.isEmpty()) toolProbe(clientbound, serverbound, "draconicnext:draconic_staff_of_power");
                if (gunItem != null && !gunItem.isEmpty()) spearProbe(clientbound, serverbound, "draconicnext:draconic_spear");
                if (gunItem != null && !gunItem.isEmpty()) modMobProbe(clientbound, serverbound);
                if (gunItem != null && !gunItem.isEmpty()) craftProbe(clientbound, serverbound);
                if (gunItem != null && !gunItem.isEmpty()) enchantProbe(clientbound, serverbound);
                if (gunItem != null && !gunItem.isEmpty()) maceProbe(clientbound, serverbound);
                if (crateItem != null && !crateItem.isEmpty()) crateProbe(clientbound, serverbound, crateItem);
                if (System.getenv("FERRUM_PROBE_TERRAIN") != null) terrainProbe(clientbound, serverbound, Integer.parseInt(System.getenv("FERRUM_PROBE_TERRAIN")));
                if (System.getenv("FERRUM_PROBE_MACHINES") != null) machinesProbe(clientbound, serverbound);
                if (placeItem != null && !placeItem.isEmpty() && blockName.equals("hbm:machine_electric_furnace")) carryIntoDisconnect(clientbound, serverbound, new net.minecraft.core.BlockPos(x, y, z));
                return;
            }
        }
    }

    /** Opens the machine menu and picks the hotbar stack up onto the cursor, so the disconnect happens with it carried. */
    private void carryIntoDisconnect(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound, net.minecraft.core.BlockPos machine) throws IOException {
        command(serverbound, "gamemode creative @s");
        pump(clientbound, serverbound, 1000, packet -> false);
        giveHotbar(clientbound, serverbound, new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.COBBLESTONE, 64), net.minecraft.world.item.ItemStack.EMPTY);
        var hit = new net.minecraft.world.phys.BlockHitResult(
            new net.minecraft.world.phys.Vec3(machine.getX() + 0.5, machine.getY() + 1.0, machine.getZ() + 0.5), net.minecraft.core.Direction.UP, machine, false);
        var menu = openMenu(clientbound, serverbound, () -> new net.minecraft.network.protocol.game.ServerboundUseItemOnPacket(net.minecraft.world.InteractionHand.MAIN_HAND, hit, ++useSequence));
        if (menu.id() <= 0) throw new IllegalStateException("The machine menu did not open for the carry probe");
        int hotbar0 = menu.items().size() - 9;
        sendPacket(serverbound, clickPacket(menu.id(), menu.stateId(), hotbar0, 0, net.minecraft.world.inventory.ContainerInput.PICKUP));
        boolean carried = pump(clientbound, serverbound, 5000, packet -> packet instanceof net.minecraft.network.protocol.game.ClientboundSetCursorItemPacket cursor && cursor.contents().is(net.minecraft.world.item.Items.COBBLESTONE));
        report.addProperty("carriedAtDisconnect", carried);
        carrying = carried;
    }

    /** The test script says "probe-chat" on the server console once the client is in play. */
    private void chatSeen(String text) {
        System.out.println("play <- chat " + text);
        if (text.contains("probe-chat")) report.addProperty("chat", text);
    }

    /** Keeps reading until a dropped item's data and a chat message arrived, or the wait ends. */
    private void observeEntityAndChat(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound) throws IOException {
        long deadline = System.currentTimeMillis() + 30000;
        socket.setSoTimeout(1000);
        while (System.currentTimeMillis() < deadline && !(report.has("droppedItem") && report.has("chat"))) {
            Packet<?> packet;
            try {
                packet = decode(clientbound, read());
            } catch (java.net.SocketTimeoutException idle) {
                continue;
            }
            if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
                sendPacket(serverbound, new ServerboundKeepAlivePacket(keepAlive.getId()));
            } else if (packet instanceof ClientboundPingPacket ping) {
                sendPacket(serverbound, new ServerboundPongPacket(ping.getId()));
            } else if (packet instanceof ClientboundChunkBatchFinishedPacket) {
                sendPacket(serverbound, new ServerboundChunkBatchReceivedPacket(20.0F));
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket data) {
                for (var value : data.packedItems()) {
                    if (value.value() instanceof net.minecraft.world.item.ItemStack stack && !stack.isEmpty() && !report.has("droppedItem")) {
                        report.addProperty("droppedItem", BuiltInRegistries.ITEM.getKey(stack.getItem()).toString());
                    }
                }
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundDisguisedChatPacket chat) {
                chatSeen(chat.message().getString());
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundSystemChatPacket chat) {
                chatSeen(chat.content().getString());
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundPlayerChatPacket chat) {
                chatSeen(chat.body().content());
            }
        }
    }

    private int stateAt(FriendlyByteBuf buffer, int x, int y, int z) {
        int target = (y + 64) >> 4;
        int globalBits = Mth.ceillog2(Block.BLOCK_STATE_REGISTRY.size());
        report.addProperty("globalStateBits", globalBits);
        for (int section = 0; ; section++) {
            buffer.readShort();
            buffer.readShort();
            int bits = buffer.readUnsignedByte();
            int[] palette = null;
            int single = 0;
            if (bits == 0) {
                single = buffer.readVarInt();
            } else if (bits <= 8) {
                palette = new int[buffer.readVarInt()];
                for (int i = 0; i < palette.length; i++) palette[i] = buffer.readVarInt();
            } else if (bits != globalBits) {
                throw new IllegalStateException("Direct palette uses " + bits + " bits, client expects " + globalBits);
            }
            long[] data = new long[bits == 0 ? 0 : Mth.positiveCeilDiv(4096, 64 / bits)];
            for (int i = 0; i < data.length; i++) data[i] = buffer.readLong();
            if (section == target) {
                if (bits == 0) return single;
                int index = (((y & 15) << 8) | ((z & 15) << 4) | (x & 15));
                int perLong = 64 / bits;
                int value = (int) ((data[index / perLong] >>> ((index % perLong) * bits)) & ((1L << bits) - 1));
                return palette != null ? palette[value] : value;
            }
            int biomeBits = buffer.readUnsignedByte();
            if (biomeBits == 0) {
                buffer.readVarInt();
            } else {
                if (biomeBits <= 3) {
                    int count = buffer.readVarInt();
                    for (int i = 0; i < count; i++) buffer.readVarInt();
                }
                buffer.skipBytes(Mth.positiveCeilDiv(64, 64 / biomeBits) * 8);
            }
        }
    }

    private void checkSnapshot(FrozenRegistryPayload frozen) {
        frozenRegistries++;
        Registry<?> registry = BuiltInRegistries.REGISTRY.getValue(frozen.registryName());
        if (registry == null) {
            registryMismatches++;
            return;
        }
        Set<Identifier> seen = new HashSet<>();
        for (var entry : frozen.snapshot().getIds().int2ObjectEntrySet()) {
            seen.add(entry.getValue());
            var value = registry.getValue(entry.getValue());
            if (value == null || registryId(registry, value) != entry.getIntKey()) registryMismatches++;
        }
        if (seen.size() != registry.size()) registryMismatches++;
    }

    @SuppressWarnings("unchecked")
    private static <T> int registryId(Registry<T> registry, Object value) {
        return registry.getId((T) value);
    }

    /** The channels a client with the same mods would advertise, as NeoForge's client does. */
    @SuppressWarnings("unchecked")
    private static Map<ConnectionProtocol, Set<ModdedNetworkQueryComponent>> localChannels() {
        try {
            var field = NetworkRegistry.class.getDeclaredField("PAYLOAD_REGISTRATIONS");
            field.setAccessible(true);
            var registrations = (Map<ConnectionProtocol, Map<Identifier, PayloadRegistration<?>>>) field.get(null);
            Map<ConnectionProtocol, Set<ModdedNetworkQueryComponent>> channels = new HashMap<>();
            registrations.forEach((protocol, entries) -> {
                Set<ModdedNetworkQueryComponent> components = new HashSet<>();
                entries.forEach((id, registration) -> components.add(
                    new ModdedNetworkQueryComponent(id, registration.version(), registration.flow(), registration.optional())));
                channels.put(protocol, components);
            });
            return channels;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException(error);
        }
    }

    private int zombieId = -1;
    private int meleeZombieId = -1;
    private float meleeHealth = 20.0F;

    /** The summoned zombie's life as the client sees it: any hurt, damage or death signal for it counts as damaged. */
    private int modMobId = -1;
    private boolean modMobHurt;
    private float modMobHealth = -1;

    private void trackModMob(Packet<?> packet) {
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundAddEntityPacket add
            && BuiltInRegistries.ENTITY_TYPE.getKey(add.getType()).toString().equals("hbm:entity_test_dummy")) {
            modMobId = add.getId();
            report.addProperty("modMobId", modMobId);
        } else if (modMobId >= 0 && (packet instanceof net.minecraft.network.protocol.game.ClientboundDamageEventPacket damage && damage.entityId() == modMobId
            || packet instanceof net.minecraft.network.protocol.game.ClientboundHurtAnimationPacket hurt && hurt.id() == modMobId)) {
            modMobHurt = true;
        } else if (modMobId >= 0 && packet instanceof net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket data && data.id() == modMobId) {
            for (var value : data.packedItems()) {
                if (value.value() instanceof Float health) {
                    if (modMobHealth >= 0 && health < modMobHealth) modMobHurt = true;
                    modMobHealth = health;
                }
            }
        }
    }

    /** A punch on a Java-ticked mod mob must reach its Java hurtServer. */
    private void modMobProbe(ProtocolInfo<?> clientbound, ProtocolInfo<?> serverbound) throws IOException {
        command(serverbound, "gamemode survival NeoProbe");
        command(serverbound, "tp NeoProbe -0.5 101 4.5");
        command(serverbound, "setblock -1 100 6 minecraft:stone");
        command(serverbound, "ferrum summon -1 101 6 hbm:entity_test_dummy");
        for (int wait = 0; modMobId < 0 && wait < 10; wait++) pump(clientbound, serverbound, 500);
        if (modMobId < 0) return;
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket(8));
        pump(clientbound, serverbound, 1500);
        sendPacket(serverbound, new net.minecraft.network.protocol.game.ServerboundAttackPacket(modMobId));
        pump(clientbound, serverbound, 2000, packet -> modMobHurt);
        report.addProperty("modMobHurt", modMobHurt);
    }

    private void trackZombie(Packet<?> packet) {
        trackModMob(packet);
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundAddEntityPacket add) {
            if (meleeZombieId < 0 && add.getZ() < 5.0 && BuiltInRegistries.ENTITY_TYPE.getKey(add.getType()).toString().equals("minecraft:husk")) {
                meleeZombieId = add.getId();
                return;
            }
            if (zombieId < 0 && BuiltInRegistries.ENTITY_TYPE.getKey(add.getType()).toString().equals("minecraft:husk")) {
                zombieId = add.getId();
                report.addProperty("zombieId", zombieId);
            }
            return;
        }
        if (meleeZombieId >= 0) {
            if (packet instanceof net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket data && data.id() == meleeZombieId) {
                for (var value : data.packedItems()) if (value.value() instanceof Float health) meleeHealth = Math.min(meleeHealth, health);
            } else if (packet instanceof net.minecraft.network.protocol.game.ClientboundEntityEventPacket event) {
                try {
                    var field = net.minecraft.network.protocol.game.ClientboundEntityEventPacket.class.getDeclaredField("entityId");
                    field.setAccessible(true);
                    if (field.getInt(event) == meleeZombieId && event.getEventId() == 3) meleeHealth = 0.0F;
                } catch (ReflectiveOperationException error) { throw new IllegalStateException(error); }
            }
        }
        if (zombieId < 0 || report.has("zombieDamaged")) return;
        boolean hit = packet instanceof net.minecraft.network.protocol.game.ClientboundHurtAnimationPacket hurt && hurt.id() == zombieId
            || packet instanceof net.minecraft.network.protocol.game.ClientboundDamageEventPacket damage && damage.entityId() == zombieId;
        if (!hit && packet instanceof net.minecraft.network.protocol.game.ClientboundEntityEventPacket event) {
            try {
                var field = net.minecraft.network.protocol.game.ClientboundEntityEventPacket.class.getDeclaredField("entityId");
                field.setAccessible(true);
                hit = field.getInt(event) == zombieId && (event.getEventId() == 2 || event.getEventId() == 3);
            } catch (ReflectiveOperationException error) { throw new IllegalStateException(error); }
        }
        if (!hit && packet instanceof net.minecraft.network.protocol.game.ClientboundSetEntityDataPacket data && data.id() == zombieId) {
            for (var value : data.packedItems()) {
                if (value.value() instanceof Float health && health < 20.0F) hit = true;
            }
        }
        if (hit) {
            report.addProperty("zombieDamaged", true);
            System.out.println("play <- zombie #" + zombieId + " damaged: " + packet.type().id());
        }
    }

    private Packet<?> decode(ProtocolInfo<?> protocol, FriendlyByteBuf raw) {
        int start = raw.readerIndex();
        int id = raw.readVarInt();
        raw.readerIndex(start);
        try {
            Packet<?> packet = protocol.codec().decode(raw);
            if (raw.isReadable()) throw new IllegalStateException(raw.readableBytes() + " trailing bytes");
            trackZombie(packet);
            if (packet instanceof ClientboundLevelChunkWithLightPacket) chunksSeen++;
            return packet;
        } catch (RuntimeException error) {
            if (protocol.id() == ConnectionProtocol.CONFIGURATION) {
                throw new IllegalStateException("Undecodable configuration packet " + id, error);
            }
            if (decodeFailures.size() < 32) decodeFailures.add(id + ": " + error);
            System.out.println("play <- undecodable " + id + ": " + error + (error.getCause() != null ? " / " + error.getCause() : ""));
            return null;
        }
    }

    @SuppressWarnings("unchecked")
    private void sendPacket(ProtocolInfo<?> protocol, Packet<?> packet) throws IOException {
        ByteBuf buffer = Unpooled.buffer();
        ((StreamCodec<ByteBuf, Packet<?>>) (Object) protocol.codec()).encode(buffer, packet);
        writeFrame(ByteBufUtil.getBytes(buffer));
        buffer.release();
    }

    private interface Writer {
        void write(FriendlyByteBuf buffer);
    }

    private void send(Writer writer) throws IOException {
        var buffer = new FriendlyByteBuf(Unpooled.buffer());
        writer.write(buffer);
        writeFrame(ByteBufUtil.getBytes(buffer));
    }

    private void writeFrame(byte[] data) throws IOException {
        var frame = new FriendlyByteBuf(Unpooled.buffer());
        if (threshold >= 0) {
            var body = new FriendlyByteBuf(Unpooled.buffer());
            if (data.length >= threshold) {
                body.writeVarInt(data.length);
                var deflater = new Deflater();
                deflater.setInput(data);
                deflater.finish();
                byte[] chunk = new byte[8192];
                while (!deflater.finished()) body.writeBytes(chunk, 0, deflater.deflate(chunk));
                deflater.end();
            } else {
                body.writeVarInt(0);
                body.writeBytes(data);
            }
            data = ByteBufUtil.getBytes(body);
        }
        frame.writeVarInt(data.length);
        frame.writeBytes(data);
        out.write(ByteBufUtil.getBytes(frame));
        out.flush();
    }

    private FriendlyByteBuf read() throws IOException {
        int length = readVarInt();
        if (length <= 0 || length > 8 * 1024 * 1024) throw new IOException("Bad frame length " + length);
        byte[] frame = new byte[length];
        in.readFully(frame);
        var buffer = new FriendlyByteBuf(Unpooled.wrappedBuffer(frame));
        if (threshold < 0) return buffer;
        int size = buffer.readVarInt();
        if (size == 0) return new FriendlyByteBuf(buffer.slice());
        if (size > 8 * 1024 * 1024) throw new IOException("Bad packet size " + size);
        var inflater = new Inflater();
        inflater.setInput(ByteBufUtil.getBytes(buffer));
        byte[] data = new byte[size];
        try {
            int read = 0;
            while (read < size && !inflater.finished()) read += inflater.inflate(data, read, size - read);
        } catch (DataFormatException error) {
            throw new IOException(error);
        } finally {
            inflater.end();
        }
        return new FriendlyByteBuf(Unpooled.wrappedBuffer(data));
    }

    private int readVarInt() throws IOException {
        int value = 0;
        for (int i = 0; i < 5; i++) {
            int b = in.read();
            if (b < 0) throw new EOFException("Server closed the connection");
            value |= (b & 0x7F) << (i * 7);
            if ((b & 0x80) == 0) return value;
        }
        throw new IOException("VarInt too long");
    }
}

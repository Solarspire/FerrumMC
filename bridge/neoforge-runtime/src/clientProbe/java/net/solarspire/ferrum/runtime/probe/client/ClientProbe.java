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
    public static String run(String host, int port, String blockName, int x, int y, int z, int timeoutMillis, String placeItem, String gunItem) throws Exception {
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress(host, port), timeoutMillis);
            socket.setSoTimeout(timeoutMillis);
            var probe = new ClientProbe(socket);
            probe.login();
            probe.configuration();
            probe.play(blockName, x, y, z, placeItem, gunItem);
            return probe.report.toString();
        }
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
        report.addProperty("gunInitialData", String.valueOf(initial));
        report.addProperty("gunData", String.valueOf(gunData));
        report.addProperty("gunStateChanged", gunData != null && !gunData.equals(initial));
        report.addProperty("gunKeybindsSent", sent);
        report.addProperty("gunStates", String.join(" | ", gunStates));
    }

    private void play(String blockName, int x, int y, int z, String placeItem, String gunItem) throws Exception {
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
                if (gunItem != null && !gunItem.isEmpty()) gunProbe(clientbound, serverbound, gunItem);
                return;
            }
        }
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

    /** The summoned zombie's life as the client sees it: any hurt, damage or death signal for it counts as damaged. */
    private void trackZombie(Packet<?> packet) {
        if (packet instanceof net.minecraft.network.protocol.game.ClientboundAddEntityPacket add) {
            if (zombieId < 0 && BuiltInRegistries.ENTITY_TYPE.getKey(add.getType()).toString().equals("minecraft:zombie")) {
                zombieId = add.getId();
                report.addProperty("zombieId", zombieId);
            }
            return;
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

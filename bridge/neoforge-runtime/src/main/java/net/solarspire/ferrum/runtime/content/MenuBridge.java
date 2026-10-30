package net.solarspire.ferrum.runtime.content;

import io.netty.buffer.Unpooled;
import java.util.Base64;
import net.minecraft.network.ProtocolInfo;
import net.minecraft.network.protocol.game.GameProtocols;
import net.minecraft.network.protocol.game.ServerGamePacketListener;
import net.minecraft.network.protocol.game.ServerboundContainerButtonClickPacket;
import net.minecraft.network.protocol.game.ServerboundContainerClickPacket;
import net.minecraft.network.protocol.game.ServerboundContainerClosePacket;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.neoforged.neoforge.network.connection.ConnectionType;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Menus that Java code opens for a Rust player. Java owns the menu object and its slot sync; Rust only forwards the
 * client's container packets while one is open. The stack movement itself runs against the player's inventory view,
 * which Rust refreshes from its own authoritative inventory before every step and reads back after it.
 */
final class MenuBridge {
    private static final Logger LOGGER = LoggerFactory.getLogger(MenuBridge.class);
    private static ProtocolInfo<ServerGamePacketListener> serverbound;

    private MenuBridge() {}

    /** The open Java menu's container id, or 0 when the player only has their inventory open. */
    static int openId(RustPlayer player) {
        return player.containerMenu == player.inventoryMenu ? 0 : player.containerMenu.containerId;
    }

    @SuppressWarnings("unchecked")
    private static synchronized ProtocolInfo<ServerGamePacketListener> protocol() {
        if (serverbound == null) {
            serverbound = GameProtocols.SERVERBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(BridgeContent.worldRegistries, ConnectionType.NEOFORGE), () -> true);
        }
        return serverbound;
    }

    /** One serverbound packet exactly as the client wrote it (id, then body); only container packets reach here. */
    static void packet(RustPlayer player, String data) {
        var buffer = Unpooled.wrappedBuffer(Base64.getDecoder().decode(data));
        try {
            var packet = protocol().codec().decode(buffer);
            if (buffer.isReadable()) throw new IllegalStateException("Container packet left " + buffer.readableBytes() + " unread bytes");
            if (packet instanceof ServerboundContainerClickPacket click) click(player, click);
            else if (packet instanceof ServerboundContainerClosePacket) player.doCloseContainer();
            else if (packet instanceof ServerboundContainerButtonClickPacket button) button(player, button);
            else LOGGER.debug("Ignoring {} for a bridged menu", packet.type());
        } finally {
            buffer.release();
        }
    }

    /** Mirrors ServerGamePacketListenerImpl.handleContainerClick. */
    private static void click(RustPlayer player, ServerboundContainerClickPacket packet) {
        var menu = player.containerMenu;
        if (menu.containerId != packet.containerId()) return;
        if (player.isSpectator() || player.isDeadOrDying()) {
            menu.sendAllDataToRemote();
            return;
        }
        if (!menu.stillValid(player)) {
            LOGGER.debug("Player {} interacted with invalid menu {}", player.getGameProfile().name(), menu);
            return;
        }
        int slot = packet.slotNum();
        if (!menu.isValidSlotIndex(slot)) {
            LOGGER.debug("Player {} clicked invalid slot index {} of {} slots", player.getGameProfile().name(), slot, menu.slots.size());
            return;
        }
        boolean resync = packet.stateId() != menu.getStateId();
        menu.suppressRemoteUpdates();
        menu.clicked(slot, packet.buttonNum(), packet.containerInput(), player);
        for (var entry : it.unimi.dsi.fastutil.ints.Int2ObjectMaps.fastIterable(packet.changedSlots())) {
            menu.setRemoteSlotUnsafe(entry.getIntKey(), entry.getValue());
        }
        menu.setRemoteCarried(packet.carriedItem());
        menu.resumeRemoteUpdates();
        if (resync) menu.broadcastFullState();
        else menu.broadcastChanges();
    }

    /** Mirrors ServerGamePacketListenerImpl.handleContainerButtonClick. */
    private static void button(RustPlayer player, ServerboundContainerButtonClickPacket packet) {
        var menu = player.containerMenu;
        if (menu.containerId != packet.containerId() || player.isSpectator()) return;
        if (!menu.stillValid(player)) return;
        if (menu.clickMenuButton(player, packet.buttonId())) menu.broadcastChanges();
    }

    /** Mirrors the container part of ServerPlayer.tick: push slot changes, close a menu that stopped being valid. */
    static void tick(RustPlayer player) {
        if (player.containerMenu == player.inventoryMenu) return;
        player.containerMenu.broadcastChanges();
        if (!player.containerMenu.stillValid(player)) {
            player.closeContainer();
            player.containerMenu = player.inventoryMenu;
        }
    }
}

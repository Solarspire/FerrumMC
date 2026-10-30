package net.solarspire.ferrummc.hello;

import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.block.BlockBreakEvent;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.plugin.java.JavaPlugin;

public final class HelloPlugin extends JavaPlugin implements Listener {
    @Override
    public void onEnable() {
        getServer().getPluginManager().registerEvents(this, this);
        getLogger().info("FerrumHello enabled; player joins are logged and block breaks are cancelled");
    }

    @EventHandler
    public void onJoin(PlayerJoinEvent event) {
        getLogger().info("FerrumHello join: " + event.getPlayer().getName());
    }

    @EventHandler
    public void onBreak(BlockBreakEvent event) {
        event.setCancelled(true);
        getLogger().info("FerrumHello cancelled block break: " + event.getPlayer().getName() + " " + event.getBlock().getType());
    }

    @Override
    public void onDisable() {
        getLogger().info("FerrumHello disabled");
    }
}

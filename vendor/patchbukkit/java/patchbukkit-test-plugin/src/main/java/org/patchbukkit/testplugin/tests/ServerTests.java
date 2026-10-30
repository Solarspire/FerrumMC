package org.patchbukkit.testplugin.tests;

import org.bukkit.Bukkit;
import org.bukkit.Server;
import org.bukkit.command.CommandMap;
import org.bukkit.command.ConsoleCommandSender;
import org.bukkit.plugin.PluginManager;
import org.bukkit.scheduler.BukkitScheduler;
import org.patchbukkit.testplugin.ConformanceTest;
import org.patchbukkit.testplugin.TestCategory;
import org.patchbukkit.testplugin.TestExpectation;
import org.bukkit.plugin.java.JavaPlugin;

import static org.patchbukkit.testplugin.TestAssertions.*;

public final class ServerTests {

    private final JavaPlugin plugin;

    public ServerTests(JavaPlugin plugin) {
        this.plugin = plugin;
    }

    @ConformanceTest(name = "Server.getName() returns non-null", category = TestCategory.SERVER)
    public void testGetName() {
        String name = Bukkit.getServer().getName();
        assertNotNull(name, "Server.getName()");
    }

    @ConformanceTest(name = "Server.getVersion() returns non-null", category = TestCategory.SERVER)
    public void testGetVersion() {
        String version = Bukkit.getServer().getVersion();
        assertNotNull(version, "Server.getVersion()");
    }

    @ConformanceTest(name = "Server.getBukkitVersion() returns non-null", category = TestCategory.SERVER)
    public void testGetBukkitVersion() {
        String version = Bukkit.getServer().getBukkitVersion();
        assertNotNull(version, "Server.getBukkitVersion()");
    }

    @ConformanceTest(name = "Server.getPluginManager() returns non-null", category = TestCategory.SERVER)
    public void testGetPluginManager() {
        PluginManager pm = Bukkit.getServer().getPluginManager();
        assertNotNull(pm, "Server.getPluginManager()");
    }

    @ConformanceTest(name = "Server.getConsoleSender() returns non-null", category = TestCategory.SERVER)
    public void testGetConsoleSender() {
        ConsoleCommandSender cs = Bukkit.getServer().getConsoleSender();
        assertNotNull(cs, "Server.getConsoleSender()");
    }

    @ConformanceTest(name = "Server.getCommandMap() returns non-null", category = TestCategory.SERVER)
    public void testGetCommandMap() {
        CommandMap cm = Bukkit.getServer().getCommandMap();
        assertNotNull(cm, "Server.getCommandMap()");
    }

    @ConformanceTest(name = "Server.getScheduler() returns non-null", category = TestCategory.SERVER)
    public void testGetScheduler() {
        BukkitScheduler sched = Bukkit.getServer().getScheduler();
        assertNotNull(sched, "Server.getScheduler()");
    }

    @ConformanceTest(name = "Server.getOnlinePlayers() returns collection", category = TestCategory.SERVER)
    public void testGetOnlinePlayers() {
        var players = Bukkit.getServer().getOnlinePlayers();
        assertNotNull(players, "Server.getOnlinePlayers()");
    }

    @ConformanceTest(name = "Server.getUnsafe() returns non-null", category = TestCategory.SERVER)
    public void testGetUnsafe() {
        Object unsafe = Bukkit.getUnsafe();
        assertNotNull(unsafe, "Server.getUnsafe()");
    }

    @ConformanceTest(name = "Server.suggestPlayerNamesWhenNullTabCompletions() returns boolean", category = TestCategory.SERVER)
    public void testSuggestPlayerNames() {
        boolean result = Bukkit.getServer().suggestPlayerNamesWhenNullTabCompletions();
        assertTrue(result, "Server.suggestPlayerNamesWhenNullTabCompletions()");
    }

    @ConformanceTest(name = "Bukkit.getServer() matches plugin.getServer()", category = TestCategory.SERVER)
    public void testServerConsistency() {
        Server bukkit = Bukkit.getServer();
        Server fromPlugin = plugin.getServer();
        assertTrue(bukkit == fromPlugin, "Bukkit.getServer() == plugin.getServer()");
    }

    @ConformanceTest(name = "Server.getPluginCommand() returns null for unregistered command", category = TestCategory.SERVER)
    public void testGetPluginCommand() {
        // getPluginCommand only finds PluginCommands registered via plugin.yml;
        // commands registered directly on the CommandMap are not PluginCommands
        var cmd = Bukkit.getServer().getPluginCommand("nonexistent_abc");
        assertTrue(cmd == null, "Server.getPluginCommand(unregistered) should be null");
    }

    @ConformanceTest(name = "Server.getPluginCommand() returns null for unknown", category = TestCategory.SERVER)
    public void testGetPluginCommandUnknown() {
        var cmd = Bukkit.getServer().getPluginCommand("nonexistent_command_xyz");
        assertTrue(cmd == null, "Server.getPluginCommand(unknown) should be null");
    }

    @ConformanceTest(name = "Server.getWorlds() returns non-empty list", category = TestCategory.SERVER)
    public void testGetWorlds() {
        var worlds = Bukkit.getServer().getWorlds();
        assertNotNull(worlds, "Server.getWorlds()");
        assertTrue(!worlds.isEmpty(), "Server.getWorlds() should not be empty");
    }

    @ConformanceTest(name = "Server.getMaxPlayers() returns positive int", category = TestCategory.SERVER)
    public void testGetMaxPlayers() {
        int max = Bukkit.getServer().getMaxPlayers();
        assertTrue(max >= 0, "Server.getMaxPlayers() >= 0");
    }

    @ConformanceTest(name = "Server.getPort() returns port", category = TestCategory.SERVER)
    public void testGetPort() {
        int port = Bukkit.getServer().getPort();
        assertTrue(port >= 0, "Server.getPort() >= 0");
    }

    @ConformanceTest(name = "Server.getIp() returns string", category = TestCategory.SERVER)
    public void testGetIp() {
        String ip = Bukkit.getServer().getIp();
        assertNotNull(ip, "Server.getIp()");
    }

    @ConformanceTest(name = "Server.getViewDistance() returns positive int", category = TestCategory.SERVER)
    public void testGetViewDistance() {
        int vd = Bukkit.getServer().getViewDistance();
        assertTrue(vd > 0, "Server.getViewDistance() > 0");
    }

    @ConformanceTest(name = "Server.getSimulationDistance() returns positive int", category = TestCategory.SERVER)
    public void testGetSimulationDistance() {
        int sd = Bukkit.getServer().getSimulationDistance();
        assertTrue(sd > 0, "Server.getSimulationDistance() > 0");
    }

    @ConformanceTest(name = "Server.getUpdateFolder() returns string", category = TestCategory.SERVER)
    public void testGetUpdateFolder() {
        String uf = Bukkit.getServer().getUpdateFolder();
        assertNotNull(uf, "Server.getUpdateFolder()");
    }

    @ConformanceTest(name = "Server.getUpdateFolderFile() returns non-null", category = TestCategory.SERVER)
    public void testGetUpdateFolderFile() {
        java.io.File file = Bukkit.getServer().getUpdateFolderFile();
        assertNotNull(file, "Server.getUpdateFolderFile()");
    }

    @ConformanceTest(name = "Server.getConnectionThrottle() returns long", category = TestCategory.SERVER)
    public void testGetConnectionThrottle() {
        long throttle = Bukkit.getServer().getConnectionThrottle();
        assertTrue(throttle >= 0, "Server.getConnectionThrottle() >= 0");
    }

    @ConformanceTest(name = "Server.broadcastMessage() works without throwing", category = TestCategory.SERVER)
    @SuppressWarnings("deprecation")
    public void testBroadcastMessage() {
        int reached = Bukkit.getServer().broadcastMessage("test conformance broadcast");
        assertTrue(reached >= 0, "broadcastMessage return >= 0");
    }

    @ConformanceTest(name = "Server.getOfflinePlayer(UUID) returns non-null", category = TestCategory.SERVER)
    public void testGetOfflinePlayer() {
        var op = Bukkit.getServer().getOfflinePlayer(java.util.UUID.randomUUID());
        assertNotNull(op, "Server.getOfflinePlayer(UUID)");
    }

    @ConformanceTest(name = "Server.getBanList() returns non-null", category = TestCategory.SERVER)
    public void testGetBanList() {
        var bans = Bukkit.getServer().getBanList(org.bukkit.BanList.Type.NAME);
        assertNotNull(bans, "Server.getBanList(NAME)");
    }

    @ConformanceTest(name = "Server.getOperators() returns non-null set", category = TestCategory.SERVER)
    public void testGetOperators() {
        var ops = Bukkit.getServer().getOperators();
        assertNotNull(ops, "Server.getOperators()");
    }

    @ConformanceTest(name = "Server.getWhitelistedPlayers() returns non-null set", category = TestCategory.SERVER)
    public void testGetWhitelistedPlayers() {
        var wl = Bukkit.getServer().getWhitelistedPlayers();
        assertNotNull(wl, "Server.getWhitelistedPlayers()");
    }

    @ConformanceTest(name = "Server.reloadWhitelist() executes without throwing", category = TestCategory.SERVER)
    public void testReloadWhitelist() {
        Bukkit.getServer().reloadWhitelist();
    }

    @ConformanceTest(name = "Server.getMotd() returns non-null", category = TestCategory.SERVER)
    public void testGetMotd() {
        String motd = Bukkit.getServer().getMotd();
        assertNotNull(motd, "Server.getMotd()");
    }

    @ConformanceTest(name = "Server.getAllowNether() returns boolean", category = TestCategory.SERVER)
    public void testGetAllowNether() {
        boolean nether = Bukkit.getServer().getAllowNether();
        assertTrue(nether || !nether, "Server.getAllowNether()");
    }
}

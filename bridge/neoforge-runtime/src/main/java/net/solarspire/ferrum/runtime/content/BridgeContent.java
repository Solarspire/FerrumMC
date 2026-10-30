package net.solarspire.ferrum.runtime.content;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.TreeMap;
import java.util.regex.Pattern;
import net.minecraft.SharedConstants;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.neoforged.fml.ModList;
import net.neoforged.fml.common.Mod;
import net.neoforged.fml.loading.FMLLoader;
import net.neoforged.neoforge.network.registration.NetworkRegistry;
import net.neoforged.neoforge.network.registration.PayloadRegistration;
import net.neoforged.neoforge.server.loading.ServerModLoader;
import net.solarspire.ferrum.runtime.BridgeMixinErrors;
import net.solarspire.ferrum.runtime.HeadlessGuard;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.spongepowered.asm.mixin.MixinEnvironment;

@Mod("ferrum_runtime")
public final class BridgeContent {
    private static final Logger LOGGER = LoggerFactory.getLogger(BridgeContent.class);
    private static final Gson JSON = new GsonBuilder().setPrettyPrinting().create();
    private static final JsonArray AUDIT_FAILURES = new JsonArray();
    private static final Map<String, String> MIXIN_PACKAGES = new TreeMap<>();
    private static final JsonArray SKIPPED_CLIENTS = new JsonArray();
    private static Path reportPath;
    static net.minecraft.core.RegistryAccess.Frozen worldRegistries;
    static net.minecraft.core.LayeredRegistryAccess<net.minecraft.server.RegistryLayer> layeredRegistries;
    static java.util.List<net.minecraft.server.packs.repository.KnownPack> serverKnownPacks = java.util.List.of();
    static net.minecraft.world.item.crafting.RecipeManager recipes;
    static net.minecraft.world.level.block.entity.FuelValues fuels;
    static net.minecraft.server.MinecraftServer.ReloadableResources serverResources;
    private static net.minecraft.server.packs.resources.CloseableResourceManager resources;

    public BridgeContent() {}

    public static void boot(String configJson) throws Exception {
        JsonObject config = JSON.fromJson(configJson, JsonObject.class);
        reportPath = Path.of(config.get("reportPath").getAsString()).toAbsolutePath();
        collectMixinConfigs();
        SharedConstants.tryDetectVersion();
        if (!SharedConstants.getCurrentVersion().id().equals("26.2")
            || SharedConstants.class.getClassLoader() != BridgeContent.class.getClassLoader()) {
            throw new IllegalStateException("Bridge requires the FML-transformed Minecraft 26.2 class universe");
        }
        Bootstrap.bootStrap();
        ServerModLoader.load(false);
        var packs = net.minecraft.server.packs.repository.ServerPacksSource.createVanillaTrustedRepository();
        var packConfig = new net.minecraft.server.WorldLoader.PackConfig(packs,net.minecraft.world.level.WorldDataConfiguration.DEFAULT,false,true);
        var init = new net.minecraft.server.WorldLoader.InitConfig(packConfig,net.minecraft.commands.Commands.CommandSelection.DEDICATED,net.minecraft.server.permissions.PermissionSet.ALL_PERMISSIONS);
        net.minecraft.server.WorldLoader.load(init,
            context -> new net.minecraft.server.WorldLoader.DataLoadOutput<>(Boolean.TRUE,context.datapackDimensions()),
            (manager,data,registries,cookie) -> {
                resources=manager;
                layeredRegistries=registries;
                worldRegistries=registries.compositeAccess();
                serverKnownPacks=manager.listPacks().flatMap(pack -> pack.knownPackInfo().stream()).toList();
                serverResources=new net.minecraft.server.MinecraftServer.ReloadableResources(manager,data);
                recipes=data.getRecipeManager();
                recipes.finalizeRecipeLoading(net.minecraft.world.level.WorldDataConfiguration.DEFAULT.enabledFeatures());
                fuels=net.minecraft.world.level.block.entity.FuelValues.vanillaBurnTimes(worldRegistries,net.minecraft.world.level.WorldDataConfiguration.DEFAULT.enabledFeatures());
                return cookie;
            },
            Runnable::run,Runnable::run).join();
        // NeoForge loads SERVER configs when a server starts; ConfigSync sends them to modded clients.
        var gameDirectory = net.neoforged.fml.loading.FMLPaths.GAMEDIR.get();
        net.neoforged.fml.config.ConfigTracker.INSTANCE.loadConfigs(net.neoforged.fml.config.ModConfig.Type.SERVER,
            gameDirectory.resolve("serverconfig"), gameDirectory.resolve("defaultconfigs"));
        try {
            MixinEnvironment.getCurrentEnvironment().audit();
        } catch (LinkageError | RuntimeException error) {
            AUDIT_FAILURES.add(error.toString());
            LOGGER.error("Mixin target audit failed; runtime remains available for diagnostics", error);
        }
        Files.writeString(reportPath, dumpRegistries());
        LOGGER.info("FerrumMC NeoForge bridge ready; no MinecraftServer or ServerLevel was created");
    }

    public static String dumpRegistries() throws Exception {
        JsonObject report = new JsonObject();
        report.addProperty("minecraft", SharedConstants.getCurrentVersion().id());
        report.addProperty("neoforge", FMLLoader.getCurrent().getVersionInfo().neoForgeVersion());
        report.addProperty("ready", true);
        JsonArray mods = new JsonArray();
        for (var mod : ModList.get().getMods()) {
            JsonObject entry = new JsonObject();
            entry.addProperty("id", mod.getModId());
            entry.addProperty("version", mod.getVersion().toString());
            entry.addProperty("constructed", ModList.get().getModContainerById(mod.getModId()).isPresent());
            mods.add(entry);
        }
        report.add("mods", mods);
        JsonObject registries = new JsonObject();
        var frozenField = MappedRegistry.class.getDeclaredField("frozen");
        frozenField.setAccessible(true);
        boolean frozen = frozenField.getBoolean(BuiltInRegistries.REGISTRY);
        for (var registry : BuiltInRegistries.REGISTRY) {
            frozen &= frozenField.getBoolean(registry);
            JsonArray entries = new JsonArray();
            registry.keySet().stream().sorted().forEach(id -> entries.add(id.toString()));
            registries.add(registry.key().identifier().toString(), entries);
        }
        report.add("registries", registries);
        report.addProperty("registriesFrozen", frozen);
        var payloadField = NetworkRegistry.class.getDeclaredField("PAYLOAD_REGISTRATIONS");
        payloadField.setAccessible(true);
        JsonObject payloads = new JsonObject();
        for (var entry : ((Map<?, ?>) payloadField.get(null)).entrySet()) {
            JsonArray registrations = new JsonArray();
            var values = ((Map<?, ?>) entry.getValue()).values().stream()
                .map(value -> (PayloadRegistration<?>) value)
                .sorted(java.util.Comparator.comparing(value -> value.id().toString())).toList();
            for (var value : values) {
                JsonObject registration = new JsonObject();
                registration.addProperty("id", value.id().toString());
                registration.addProperty("version", value.version());
                registration.addProperty("optional", value.optional());
                registration.addProperty("flow", value.flow().map(Enum::name).orElse("BIDIRECTIONAL"));
                registrations.add(registration);
            }
            payloads.add(entry.getKey().toString(), registrations);
        }
        report.add("payloads", payloads);
        var audit = FMLLoader.getCurrent().getClassTransformerAuditLog();
        var auditField = audit.getClass().getDeclaredField("audit");
        auditField.setAccessible(true);
        JsonArray transformations = new JsonArray();
        var classes = ((Map<?, ?>) auditField.get(audit)).keySet().stream().map(Object::toString).sorted().toList();
        var application = Pattern.compile("APP:([^:,\\]]+):([^,\\]]+?) from mod ([^,\\]]+)");
        for (String target : classes) {
            String activity = audit.getAuditString(target);
            var matcher = application.matcher(activity);
            while (matcher.find()) {
                JsonObject applied = new JsonObject();
                applied.addProperty("target", target);
                applied.addProperty("config", matcher.group(1));
                applied.addProperty("mixin", MIXIN_PACKAGES.get(matcher.group(1)) + "." + matcher.group(2));
                applied.addProperty("mod", matcher.group(3));
                transformations.add(applied);
            }
        }
        report.add("appliedMixins", transformations);
        report.add("failedMixins", JSON.toJsonTree(BridgeMixinErrors.failures()));
        report.add("mixinAuditFailures", AUDIT_FAILURES.deepCopy());
        report.add("skippedClientMixins", SKIPPED_CLIENTS.deepCopy());
        report.add("headlessGuards", JSON.toJsonTree(HeadlessGuard.installedGuards()));
        report.add("blockedWorldOperations", JSON.toJsonTree(HeadlessGuard.blockedOperations()));
        JsonArray serverThreads = new JsonArray();
        Thread.getAllStackTraces().keySet().stream().map(Thread::getName)
            .filter(name -> name.equals("Server thread")).sorted().forEach(serverThreads::add);
        report.add("serverThreads", serverThreads);
        report.addProperty("compatible", frozen && BridgeMixinErrors.failures().isEmpty()
            && AUDIT_FAILURES.isEmpty() && HeadlessGuard.blockedOperations().isEmpty() && serverThreads.isEmpty());
        return JSON.toJson(report);
    }

    private static void collectMixinConfigs() throws Exception {
        for (var fileInfo : FMLLoader.getCurrent().getLoadingModList().getModFiles()) {
            var file = fileInfo.getFile();
            for (var mixinConfig : file.getMixinConfigs()) {
                byte[] bytes = file.getContents().readFile(mixinConfig.config());
                if (bytes == null) continue;
                JsonObject config = JSON.fromJson(new String(bytes, StandardCharsets.UTF_8), JsonObject.class);
                MIXIN_PACKAGES.put(mixinConfig.config(), config.get("package").getAsString());
                if (!config.has("client")) continue;
                for (var mixin : config.getAsJsonArray("client")) {
                    JsonObject skipped = new JsonObject();
                    skipped.addProperty("config", mixinConfig.config());
                    skipped.addProperty("mixin", config.get("package").getAsString() + "." + mixin.getAsString());
                    skipped.addProperty("reason", "DEDICATED_SERVER");
                    SKIPPED_CLIENTS.add(skipped);
                }
            }
        }
    }

    /** Stable disk identity is the registry name and properties, never a raw Java ID. */
    public static void shutdown() throws Exception {
        RustLevel.stopServer();
        net.neoforged.fml.config.ConfigTracker.INSTANCE.unloadConfigs(net.neoforged.fml.config.ModConfig.Type.SERVER);
        if (resources != null) { resources.close(); resources=null; }
    }

    public static String tick(String batch) throws Exception { return RustLevel.tick(batch); }

    public static String play(String request) throws Exception { return PlayerBridge.play(request); }

    public static String configuration(String request) { return ConfigBridge.handle(request); }

    public static String registryManifest() {
        JsonObject manifest = new JsonObject();
        JsonArray states = new JsonArray();
        for (var block : BuiltInRegistries.BLOCK) {
            String name = BuiltInRegistries.BLOCK.getKey(block).toString();
            for (var state : block.getStateDefinition().getPossibleStates()) {
                JsonObject entry = new JsonObject();
                entry.addProperty("name", name);
                entry.addProperty("javaId", net.minecraft.world.level.block.Block.getId(state));
                entry.addProperty("default", state == block.defaultBlockState());
                float hardness = state.getDestroySpeed(net.minecraft.world.level.EmptyBlockGetter.INSTANCE, net.minecraft.core.BlockPos.ZERO);
                if (Float.isFinite(hardness)) entry.addProperty("hardness",hardness);
                else entry.addProperty("hardness",Float.toString(hardness));
                entry.addProperty("luminance", Math.clamp(state.getLightEmission(net.minecraft.world.level.EmptyBlockGetter.INSTANCE,net.minecraft.core.BlockPos.ZERO),0,15));
                entry.addProperty("solid", state.isSolidRender());
                if (block instanceof net.minecraft.world.level.block.EntityBlock entityBlock) {
                    var be = entityBlock.newBlockEntity(net.minecraft.core.BlockPos.ZERO,state);
                    if (be != null) entry.addProperty("blockEntity", BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(be.getType()).toString());
                }
                JsonObject properties = new JsonObject();
                state.getValues()
                    .sorted(java.util.Comparator.comparing(value -> value.property().getName()))
                    .forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                entry.add("properties", properties);
                states.add(entry);
            }
        }
        manifest.add("states", states);
        JsonArray items = new JsonArray();
        for (var item : BuiltInRegistries.ITEM) {
            JsonObject entry = new JsonObject();
            entry.addProperty("name", BuiltInRegistries.ITEM.getKey(item).toString());
            entry.addProperty("javaId", BuiltInRegistries.ITEM.getId(item));
            items.add(entry);
        }
        manifest.add("items", items);
        JsonArray types = new JsonArray();
        BuiltInRegistries.BLOCK_ENTITY_TYPE.keySet().stream().sorted().forEach(id -> types.add(id.toString()));
        manifest.add("blockEntityTypes", types);
        JsonObject typeIds = new JsonObject();
        for (var type : BuiltInRegistries.BLOCK_ENTITY_TYPE) {
            typeIds.addProperty(BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(type).toString(), BuiltInRegistries.BLOCK_ENTITY_TYPE.getId(type));
        }
        manifest.add("blockEntityTypeIds", typeIds);
        JsonObject blockIds = new JsonObject();
        for (var block : BuiltInRegistries.BLOCK) {
            blockIds.addProperty(BuiltInRegistries.BLOCK.getKey(block).toString(), BuiltInRegistries.BLOCK.getId(block));
        }
        manifest.add("blockIds", blockIds);
        // Mod entity types: the client id and the tracking parameters Rust needs to mirror a Java-ticked entity.
        JsonObject entityTypes = new JsonObject();
        for (var type : BuiltInRegistries.ENTITY_TYPE) {
            var key = BuiltInRegistries.ENTITY_TYPE.getKey(type);
            if (key.getNamespace().equals("minecraft")) continue;
            JsonObject entry = new JsonObject();
            entry.addProperty("javaId", BuiltInRegistries.ENTITY_TYPE.getId(type));
            entry.addProperty("width", type.getWidth());
            entry.addProperty("height", type.getHeight());
            entry.addProperty("eyeHeight", type.getDimensions().eyeHeight());
            entry.addProperty("trackingRange", type.clientTrackingRange());
            entry.addProperty("updateInterval", type.updateInterval());
            entry.addProperty("trackDeltas", type.trackDeltas());
            entityTypes.add(key.toString(), entry);
        }
        manifest.add("entityTypes", entityTypes);
        // Entry order of each synchronized registry as the client receives it; mod entries sort in among vanilla's.
        JsonObject synced = new JsonObject();
        new net.minecraft.server.network.config.SynchronizeRegistriesTask(java.util.List.of(), layeredRegistries)
            .handleResponse(java.util.List.of(), packet -> {
                if (packet instanceof net.minecraft.network.protocol.configuration.ClientboundRegistryDataPacket data) {
                    JsonArray ids = new JsonArray();
                    data.entries().forEach(entry -> ids.add(entry.id().toString()));
                    synced.add(data.registry().identifier().toString(), ids);
                }
            });
        manifest.add("syncedRegistries", synced);
        return JSON.toJson(manifest);
    }

}

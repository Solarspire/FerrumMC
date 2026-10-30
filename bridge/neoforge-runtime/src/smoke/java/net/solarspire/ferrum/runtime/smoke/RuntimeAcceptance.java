package net.solarspire.ferrum.runtime.smoke;

import com.google.gson.JsonParser;
import java.lang.reflect.InvocationTargetException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.function.Function;
import net.neoforged.fml.loading.FMLLoader;
import net.solarspire.ferrum.runtime.BridgeRuntime;
import org.slf4j.LoggerFactory;

/** Exercises the public JNI API and the transformed world-start fence in a shared JVM. */
public final class RuntimeAcceptance {
    private RuntimeAcceptance() {}

    public static void main(String[] args) throws Exception {
        try {
            BridgeRuntime.boot(args[0]);
            String first = BridgeRuntime.dumpRegistries();
            var report = JsonParser.parseString(first).getAsJsonObject();
            if (!report.get("registriesFrozen").getAsBoolean()) throw new AssertionError("Registries are mutable");
            // The host remains resident between calls without advancing a Java game loop.
            Thread.sleep(1000);
            if (!first.equals(BridgeRuntime.dumpRegistries())) throw new AssertionError("Headless registry snapshot changed");
            var config = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
            Files.writeString(Path.of(config.get("reportPath").getAsString()), first);
            var game = FMLLoader.getCurrent().getCurrentClassLoader();
            var server = Class.forName("net.minecraft.server.MinecraftServer", false, game);
            try {
                server.getMethod("spin", Function.class).invoke(null, (Function<Thread, Object>) thread -> {
                    throw new AssertionError("Vanilla server factory was reached");
                });
                throw new AssertionError("Vanilla server start was allowed");
            } catch (InvocationTargetException error) {
                if (!(error.getCause() instanceof IllegalStateException)
                    || !error.getCause().getMessage().startsWith("Unavailable in FerrumMC headless bridge mode:")) throw error;
            }
            var fenced = JsonParser.parseString(BridgeRuntime.dumpRegistries()).getAsJsonObject();
            if (fenced.getAsJsonArray("blockedWorldOperations").size() != 1) throw new AssertionError("World fence was not reported");
        } finally {
            BridgeRuntime.shutdown();
        }
        BridgeRuntime.shutdown();
        if (FMLLoader.getCurrentOrNull() != null) throw new AssertionError("FML remained active after shutdown");
        try {
            BridgeRuntime.dumpRegistries();
            throw new AssertionError("Dump succeeded after shutdown");
        } catch (IllegalStateException expected) {
            // The shared JVM is still usable after the bridge releases its loader and thread.
        }
        LoggerFactory.getLogger(RuntimeAcceptance.class).info("JNI API smoke passed; world fence enforced; shared JVM survived shutdown");
    }
}

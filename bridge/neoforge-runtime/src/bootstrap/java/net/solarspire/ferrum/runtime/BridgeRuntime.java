package net.solarspire.ferrum.runtime;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonObject;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.lang.reflect.InvocationTargetException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import net.neoforged.api.distmarker.Dist;
import net.neoforged.fml.startup.Entrypoint;
import org.slf4j.LoggerFactory;
import org.spongepowered.asm.mixin.Mixins;

/** JNI entry point; all game classes stay behind FML's transforming classloader. */
public final class BridgeRuntime extends Entrypoint {
    private static ExecutorService executor;
    private static StartupResult startupResult;
    private static Class<?> content;
    private static boolean started;

    private BridgeRuntime() {}

    /** Boots once per JVM; callers must gate gameplay on the report's compatible flag. */
    public static synchronized void boot(String configPath) throws Exception {
        if (started) throw new IllegalStateException("The NeoForge runtime can only boot once per JVM");
        started = true;
        Path path = Path.of(configPath).toAbsolutePath();
        if (Files.size(path) > 1024 * 1024) throw new IllegalArgumentException("Runtime config exceeds 1 MiB");
        JsonObject config = new Gson().fromJson(Files.readString(path), JsonObject.class);
        for (String key : new String[] {"gameDirectory", "reportPath"}) {
            if (config == null || !config.has(key) || !config.get(key).isJsonPrimitive()
                || !config.getAsJsonPrimitive(key).isString() || config.get(key).getAsString().isBlank()) {
                throw new IllegalArgumentException("Runtime config requires a string: " + key);
            }
        }
        Path directory = Path.of(config.get("gameDirectory").getAsString()).toAbsolutePath();
        if (!Files.isDirectory(directory)) throw new IllegalArgumentException("Missing runtime directory: " + directory);
        System.setProperty("libraryDirectory", directory.resolve("libraries").toString());
        executor = Executors.newSingleThreadExecutor(runnable -> new Thread(runnable, "Ferrum compatibility thread"));
        try {
            call(() -> {
                startupResult = startup(new String[] {
                    "--gameDir", directory.toString(),
                    "--fml.neoForgeVersion", "26.2.0.93",
                    "--fml.mcVersion", "26.2", "--fml.neoFormVersion", "2"
                }, true, Dist.DEDICATED_SERVER, true);
                Mixins.registerErrorHandlerClass(BridgeMixinErrors.class.getName());
                content = Class.forName("net.solarspire.ferrum.runtime.content.BridgeContent", true,
                    startupResult.loader().getCurrentClassLoader());
                if (content.getClassLoader() != startupResult.loader().getCurrentClassLoader()) {
                    throw new IllegalStateException("Bridge content must be loaded through FML");
                }
                invoke("boot", new Class<?>[] {String.class}, config.toString());
                return null;
            });
        } catch (Exception | Error error) {
            try {
                var failure = new JsonObject();
                failure.addProperty("ready", false);
                var stack = new StringWriter();
                error.printStackTrace(new PrintWriter(stack));
                failure.addProperty("error", stack.toString());
                var json = new GsonBuilder().setPrettyPrinting().create();
                failure.add("failedMixins", json.toJsonTree(BridgeMixinErrors.failures()));
                failure.add("blockedWorldOperations", json.toJsonTree(HeadlessGuard.blockedOperations()));
                Files.writeString(Path.of(config.get("reportPath").getAsString()), json.toJson(failure));
            } catch (Exception reportError) {
                error.addSuppressed(reportError);
            }
            try {
                shutdown();
            } catch (Exception shutdownError) {
                error.addSuppressed(shutdownError);
            }
            throw error;
        }
    }

    /** Returns a JSON snapshot of the frozen registries and loader diagnostics. */
    public static synchronized String dumpRegistries() throws Exception {
        if (content == null) throw new IllegalStateException("Runtime is not ready");
        return call(() -> (String) invoke("dumpRegistries", new Class<?>[0]));
    }

    public static synchronized String registryManifest() throws Exception {
        if (content == null) throw new IllegalStateException("Runtime is not ready");
        return call(() -> (String) invoke("registryManifest", new Class<?>[0]));
    }

    public static native String world(String request);
    // Hot world operations without JSON; `world` is the handle Rust names the level by. See ferrum-neoforge world.rs.
    public static native int getState(int world, int x, int y, int z);
    public static native boolean setState(int world, int x, int y, int z, int state, int flags);
    public static native boolean isLoaded(int world, int chunkX, int chunkZ);
    public static native int getHeight(int world, int heightmap, int x, int z);
    public static native int biome(int world, int x, int y, int z);
    public static native int light(int world, int layer, int x, int y, int z);
    public static native int skyDarken(int world);
    public static native int random();

    public static ClassLoader gameClassLoader() { return content.getClassLoader(); }

    /** Runs one NeoForge configuration step for a Rust-owned connection on the compatibility thread. */
    public static String configuration(String request) throws Exception {
        if (content == null) throw new IllegalStateException("Runtime is not ready");
        return call(() -> {
            var thread = Thread.currentThread();
            var previous = thread.getContextClassLoader();
            thread.setContextClassLoader(content.getClassLoader());
            try { return (String) invoke("configuration", new Class<?>[] {String.class}, request); }
            finally { thread.setContextClassLoader(previous); }
        });
    }

    public static String tick(String batch) throws Exception {
        var thread = Thread.currentThread();
        var previous = thread.getContextClassLoader();
        thread.setContextClassLoader(content.getClassLoader());
        try { return (String) invoke("tick", new Class<?>[] {String.class}, batch); }
        finally { thread.setContextClassLoader(previous); }
    }

    /** Runs mod player events on the calling tick thread, like {@link #tick}. */
    public static String play(String request) throws Exception {
        var thread = Thread.currentThread();
        var previous = thread.getContextClassLoader();
        thread.setContextClassLoader(content.getClassLoader());
        try { return (String) invoke("play", new Class<?>[] {String.class}, request); }
        finally { thread.setContextClassLoader(previous); }
    }

    /** Releases FML resources without terminating the shared JVM. */
    public static synchronized void shutdown() throws Exception {
        if (executor == null) return;
        try {
            call(() -> {
                if (content != null) invoke("shutdown",new Class<?>[0]);
                if (startupResult != null) startupResult.close();
                content = null;
                startupResult = null;
                return null;
            });
        } finally {
            var closingExecutor = executor;
            executor = null;
            closingExecutor.close();
        }
    }

    private static Object invoke(String name, Class<?>[] types, Object... arguments) throws Exception {
        try {
            return content.getMethod(name, types).invoke(null, arguments);
        } catch (InvocationTargetException error) {
            if (error.getCause() instanceof Exception exception) throw exception;
            if (error.getCause() instanceof Error cause) throw cause;
            throw error;
        }
    }

    private static <T> T call(Callable<T> operation) throws Exception {
        var future = executor.submit(operation);
        boolean interrupted = false;
        try {
            while (true) {
                try {
                    return future.get();
                } catch (InterruptedException error) {
                    // A caller cannot abandon mod construction while it still owns this JVM.
                    interrupted = true;
                }
            }
        } catch (ExecutionException error) {
            if (error.getCause() instanceof Exception exception) throw exception;
            if (error.getCause() instanceof Error cause) throw cause;
            throw error;
        } finally {
            if (interrupted) Thread.currentThread().interrupt();
        }
    }

    public static void main(String[] args) {
        int status = 0;
        try {
            if (args.length == 0) throw new IllegalArgumentException("Usage: BridgeRuntime <config.json> [--once]");
            boot(args[0]);
            if (args.length < 2 || !args[1].equals("--once")) {
                while (System.in.read() != -1) { /* EOF shuts down the standalone host. */ }
            }
        } catch (Throwable error) {
            LoggerFactory.getLogger(BridgeRuntime.class).error("NeoForge bridge boot failed", error);
            status = 1;
        } finally {
            try {
                shutdown();
            } catch (Exception error) {
                LoggerFactory.getLogger(BridgeRuntime.class).error("NeoForge bridge shutdown failed", error);
                status = 1;
            }
        }
        // A successful CLI boot must terminate naturally, exercising the same shutdown as JNI.
        if (status != 0) System.exit(status);
    }
}

package net.solarspire.ferrum.runtime.smoke;

import java.net.URL;
import java.net.URLClassLoader;
import java.nio.file.Files;
import java.nio.file.Path;
import net.neoforged.fml.loading.FMLLoader;
import net.solarspire.ferrum.runtime.BridgeRuntime;

/** Boots NeoForge with the same mods and runs the headless client probe against a server. */
public final class ClientAcceptance {
    private ClientAcceptance() {}

    public static void main(String[] args) throws Exception {
        // args: runtime config, probe jar, host, port, block, x, y, z, report path
        BridgeRuntime.boot(args[0]);
        try {
            var game = FMLLoader.getCurrent().getCurrentClassLoader();
            try (var loader = new URLClassLoader(new URL[] {Path.of(args[1]).toUri().toURL()}, game)) {
                var thread = Thread.currentThread();
                thread.setContextClassLoader(loader);
                var probe = Class.forName("net.solarspire.ferrum.runtime.probe.client.ClientProbe", true, loader);
                String report = (String) probe.getMethod("run", String.class, int.class, String.class, int.class, int.class, int.class, int.class, String.class, String.class)
                    .invoke(null, args[2], Integer.parseInt(args[3]), args[4], Integer.parseInt(args[5]), Integer.parseInt(args[6]),
                        Integer.parseInt(args[7]), 60000, args.length > 9 ? args[9] : "", args.length > 10 ? args[10] : "");
                Files.writeString(Path.of(args[8]), report);
                System.out.println("CLIENT PROBE PASS " + report);
            }
        } finally {
            BridgeRuntime.shutdown();
        }
    }
}

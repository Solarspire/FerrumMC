package net.solarspire.ferrum.runtime;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;

/** Keeps generation on the calling Rayon worker, inside FML's class universe. */
public final class WorldgenRuntime {
    private static Class<?> bridge;
    private static Method decorate;

    public static int initialize(String biomes, int[] states) throws Exception {
        bridge = Class.forName("net.solarspire.ferrum.runtime.content.WorldgenBridge", true, BridgeRuntime.gameClassLoader());
        decorate = bridge.getMethod("decorate", long.class, String.class, int[].class, short[].class, byte[].class);
        return (int) bridge.getMethod("initialize", String.class, int[].class).invoke(null, biomes, states);
    }

    public static byte[] decorate(long seed, String dimension, int[] metadata, short[] blocks, byte[] biomes) throws Exception {
        var thread = Thread.currentThread();
        var previous = thread.getContextClassLoader();
        thread.setContextClassLoader(bridge.getClassLoader());
        try {
            return (byte[]) decorate.invoke(null, seed, dimension, metadata, blocks, biomes);
        } catch (InvocationTargetException error) {
            if (error.getCause() instanceof Exception cause) throw cause;
            if (error.getCause() instanceof Error cause) throw cause;
            throw error;
        } finally { thread.setContextClassLoader(previous); }
    }
}

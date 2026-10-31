package net.solarspire.ferrum.runtime.content;

import java.lang.reflect.Method;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import net.neoforged.bus.api.Event;
import net.neoforged.neoforge.common.NeoForge;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Posts events to the game bus one listener at a time, so a mod listener that throws against the bridge's partial
 * world cannot stop the listeners after it (HBM's tick listeners share one event with boss spawning, for example).
 */
final class SafeEvents {
    private static final Logger LOGGER = LoggerFactory.getLogger(SafeEvents.class);
    private static final Set<String> FAILURES = ConcurrentHashMap.newKeySet();
    private static final Method LISTENER_LIST = listenerList();

    private SafeEvents() {}

    private static Method listenerList() {
        try {
            var method = net.neoforged.bus.EventBus.class.getDeclaredMethod("getListenerList", Class.class);
            method.setAccessible(true);
            return method;
        } catch (ReflectiveOperationException | RuntimeException error) {
            LOGGER.warn("Cannot isolate event listeners; a failing listener will stop the ones after it", error);
            return null;
        }
    }

    static <T extends Event> T post(T event) {
        if (LISTENER_LIST != null && NeoForge.EVENT_BUS instanceof net.neoforged.bus.EventBus bus) {
            try {
                var list = (net.neoforged.bus.ListenerList) LISTENER_LIST.invoke(bus, event.getClass());
                for (var listener : list.getListeners()) {
                    try { listener.invoke(event); }
                    catch (Throwable error) { failed(event, error); }
                }
                return event;
            } catch (ReflectiveOperationException error) {
                throw new IllegalStateException(error);
            }
        }
        try { return NeoForge.EVENT_BUS.post(event); }
        catch (RuntimeException | LinkageError error) { failed(event, error); return event; }
    }

    /** The first failure of each kind is logged with its stack; repeats (these run every tick) only at debug level. */
    private static void failed(Event event, Throwable error) {
        if (error instanceof VirtualMachineError fatal) throw fatal;
        var frame = error.getStackTrace().length > 0 ? error.getStackTrace()[0].toString() : "";
        if (FAILURES.add(event.getClass().getName() + ":" + error + "@" + frame)) {
            LOGGER.error("A mod listener failed on {}", event.getClass().getSimpleName(), error);
        } else LOGGER.debug("A mod listener failed again on {}: {}", event.getClass().getSimpleName(), error.toString());
    }
}

package net.solarspire.ferrum.runtime;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import org.spongepowered.asm.mixin.extensibility.IMixinConfig;
import org.spongepowered.asm.mixin.extensibility.IMixinErrorHandler;
import org.spongepowered.asm.mixin.extensibility.IMixinInfo;

/** Keeps transformation failures visible even when bridge mode can continue loading. */
public final class BridgeMixinErrors implements IMixinErrorHandler {
    public record Failure(String phase, String config, String mixin, String target, String error) {}
    private static final List<Failure> FAILURES = new CopyOnWriteArrayList<>();

    public static List<Failure> failures() {
        return List.copyOf(FAILURES);
    }

    @Override
    public ErrorAction onPrepareError(IMixinConfig config, Throwable error, IMixinInfo mixin, ErrorAction action) {
        FAILURES.add(new Failure("prepare", config.getName(), mixin == null ? "" : mixin.getClassName(), "", error.toString()));
        return ErrorAction.WARN;
    }

    @Override
    public ErrorAction onApplyError(String target, Throwable error, IMixinInfo mixin, ErrorAction action) {
        FAILURES.add(new Failure("apply", mixin.getConfig().getName(), mixin.getClassName(), target, error.toString()));
        // Keep diagnostics available; a failed mixin still fails the acceptance gate.
        return ErrorAction.WARN;
    }
}

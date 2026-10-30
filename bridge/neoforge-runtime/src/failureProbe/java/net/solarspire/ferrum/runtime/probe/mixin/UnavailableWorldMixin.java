package net.solarspire.ferrum.runtime.probe.mixin;

import net.minecraft.server.level.ServerLevel;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(ServerLevel.class)
public abstract class UnavailableWorldMixin {
    @Inject(method = "ferrum$unavailableWorldMethod", at = @At("HEAD"))
    private void missingWorldMethod(CallbackInfo callback) {}
}

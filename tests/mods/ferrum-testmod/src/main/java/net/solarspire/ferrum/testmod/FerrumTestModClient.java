package net.solarspire.ferrum.testmod;

import net.minecraft.network.chat.Component;
import net.neoforged.api.distmarker.Dist;
import net.neoforged.bus.api.IEventBus;
import net.neoforged.fml.common.Mod;
import net.neoforged.neoforge.client.network.event.RegisterClientPayloadHandlersEvent;

@Mod(value = FerrumTestMod.ID, dist = Dist.CLIENT)
public final class FerrumTestModClient {
    public FerrumTestModClient(IEventBus bus) {
        bus.addListener(FerrumTestModClient::payloads);
    }

    private static void payloads(RegisterClientPayloadHandlersEvent event) {
        event.register(CounterPayload.TYPE, (payload, context) -> {
            var entity = context.player().level().getBlockEntity(payload.pos());
            String snapshot = entity instanceof CounterBlockEntity counter ? Long.toString(counter.counter()) : "unloaded";
            context.player().sendSystemMessage(Component.literal(
                    "Counter at " + payload.pos().toShortString() + ": server=" + payload.counter() + ", client=" + snapshot));
        });
    }
}

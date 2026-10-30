package net.solarspire.ferrum.testmod;

import net.minecraft.core.BlockPos;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.ByteBufCodecs;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;

public record CounterPayload(BlockPos pos, long counter) implements CustomPacketPayload {
    public static final Type<CounterPayload> TYPE = new Type<>(FerrumTestMod.id("counter"));
    public static final StreamCodec<RegistryFriendlyByteBuf, CounterPayload> CODEC = StreamCodec.composite(
            BlockPos.STREAM_CODEC, CounterPayload::pos,
            ByteBufCodecs.VAR_LONG, CounterPayload::counter, CounterPayload::new);

    @Override
    public Type<? extends CustomPacketPayload> type() {
        return TYPE;
    }
}

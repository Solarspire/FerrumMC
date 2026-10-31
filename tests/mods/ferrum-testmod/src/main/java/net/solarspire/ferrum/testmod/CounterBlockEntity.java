package net.solarspire.ferrum.testmod;

import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.item.Items;
import net.neoforged.neoforge.capabilities.Capabilities;
import net.neoforged.neoforge.transfer.transaction.Transaction;
import net.minecraft.core.HolderLookup;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.network.protocol.game.ClientboundBlockEntityDataPacket;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.Level;
import net.minecraft.world.Containers;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.world.level.storage.ValueOutput;
import net.neoforged.neoforge.transfer.item.ItemStacksResourceHandler;
import net.neoforged.neoforge.transfer.item.ItemResource;

public final class CounterBlockEntity extends BlockEntity {
    private static final int SYNC_INTERVAL = 20;
    private long counter;
    private final ItemStacksResourceHandler inventory = new ItemStacksResourceHandler(1) {
        @Override
        protected void onContentsChanged(int index, ItemStack previousContents) {
            setChanged();
        }
    };

    public CounterBlockEntity(BlockPos pos, BlockState state) {
        super(FerrumTestMod.COUNTER_ENTITY.get(), pos, state);
    }

    public long counter() {
        return counter;
    }

    public ItemStacksResourceHandler inventory() {
        return inventory;
    }

    public static void tick(Level level, BlockPos pos, BlockState state, CounterBlockEntity entity) {
        entity.counter++;
        entity.setChanged();
        if (entity.counter % SYNC_INTERVAL == 0) {
            level.sendBlockUpdated(pos, state, state, Block.UPDATE_CLIENTS);
            // Feeds a container above through its capability, as item pipes and machine outputs do.
            var above = level.getCapability(Capabilities.Item.BLOCK, pos.above(), Direction.DOWN);
            if (above != null) {
                try (var transaction = Transaction.openRoot()) {
                    if (above.insert(ItemResource.of(Items.STICK), 1, transaction) == 1) transaction.commit();
                }
            }
        }
    }

    @Override
    protected void saveAdditional(ValueOutput output) {
        super.saveAdditional(output);
        output.putLong("Counter", counter);
        inventory.serialize(output.child("Inventory"));
    }

    @Override
    protected void loadAdditional(ValueInput input) {
        super.loadAdditional(input);
        counter = input.getLongOr("Counter", 0L);
        inventory.deserialize(input.childOrEmpty("Inventory"));
    }

    @Override
    public void preRemoveSideEffects(BlockPos pos, BlockState state) {
        super.preRemoveSideEffects(pos, state);
        if (level instanceof ServerLevel && !inventory.getResource(0).isEmpty()) {
            Containers.dropItemStack(level, pos.getX(), pos.getY(), pos.getZ(),
                    inventory.getResource(0).toStack(inventory.getAmountAsInt(0)));
            inventory.set(0, ItemResource.EMPTY, 0);
        }
    }

    @Override
    public CompoundTag getUpdateTag(HolderLookup.Provider registries) {
        // Keep the private inventory off the wire; disk saves include it.
        CompoundTag tag = new CompoundTag();
        tag.putLong("Counter", counter);
        return tag;
    }

    @Override
    public ClientboundBlockEntityDataPacket getUpdatePacket() {
        return ClientboundBlockEntityDataPacket.create(this);
    }
}

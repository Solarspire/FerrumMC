package net.solarspire.ferrum.testmod;

import java.util.function.Consumer;
import net.minecraft.core.BlockPos;
import net.minecraft.gametest.framework.FunctionGameTestInstance;
import net.minecraft.gametest.framework.GameTestHelper;
import net.minecraft.gametest.framework.TestData;
import net.minecraft.resources.ResourceKey;
import net.minecraft.core.registries.Registries;
import net.minecraft.network.chat.Component;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.storage.TagValueInput;
import net.neoforged.bus.api.IEventBus;
import net.neoforged.neoforge.capabilities.Capabilities;
import net.neoforged.neoforge.event.RegisterGameTestsEvent;
import net.neoforged.neoforge.registries.DeferredRegister;
import net.neoforged.neoforge.transfer.item.ItemResource;
import net.neoforged.neoforge.transfer.transaction.Transaction;

final class CounterGameTests {
    private static final DeferredRegister<Consumer<GameTestHelper>> FUNCTIONS =
            DeferredRegister.create(Registries.TEST_FUNCTION, FerrumTestMod.ID);

    static {
        FUNCTIONS.register("ticks_once", () -> CounterGameTests::ticksOnce);
        FUNCTIONS.register("persists_inventory", () -> CounterGameTests::persistsInventory);
    }

    static void registerFunctions(IEventBus bus) {
        FUNCTIONS.register(bus);
    }

    static void register(RegisterGameTestsEvent event) {
        var environment = event.registerEnvironment(FerrumTestMod.id("empty"));
        for (String name : new String[]{"ticks_once", "persists_inventory"}) {
            event.registerTest(FerrumTestMod.id(name), new FunctionGameTestInstance(
                    ResourceKey.create(Registries.TEST_FUNCTION, FerrumTestMod.id(name)),
                    new TestData<>(environment, FerrumTestMod.id("empty"), 40, 0, true)));
        }
    }

    private static void ticksOnce(GameTestHelper helper) {
        BlockPos pos = new BlockPos(1, 1, 1);
        helper.setBlock(pos, FerrumTestMod.COUNTER_BLOCK.get());
        var entity = helper.getBlockEntity(pos, CounterBlockEntity.class);
        long initial = entity.counter();
        helper.runAfterDelay(5, () -> {
            helper.assertTrue(entity.counter() == initial + 5, Component.literal("Counter must advance once per server tick"));
            helper.succeed();
        });
    }

    private static void persistsInventory(GameTestHelper helper) {
        BlockPos pos = new BlockPos(1, 1, 1);
        helper.setBlock(pos, FerrumTestMod.COUNTER_BLOCK.get());
        var entity = helper.getBlockEntity(pos, CounterBlockEntity.class);
        var handler = helper.getLevel().getCapability(Capabilities.Item.BLOCK, helper.absolutePos(pos), null);
        helper.assertTrue(handler != null && handler.size() == 1, Component.literal("One-slot item capability is missing"));
        var iron = ItemResource.of(Items.IRON_INGOT);
        try (var transaction = Transaction.openRoot()) {
            handler.insert(0, iron, 7, transaction);
        }
        helper.assertTrue(handler.getAmountAsInt(0) == 0, Component.literal("Aborted insertion changed inventory"));
        try (var transaction = Transaction.openRoot()) {
            helper.assertTrue(handler.insert(0, iron, 7, transaction) == 7, Component.literal("Insertion failed"));
            transaction.commit();
        }
        helper.runAfterDelay(3, () -> {
            var registries = helper.getLevel().registryAccess();
            var saved = entity.saveWithFullMetadata(registries);
            var loaded = new CounterBlockEntity(entity.getBlockPos(), entity.getBlockState());
            loaded.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING, registries, saved));
            helper.assertTrue(loaded.counter() == entity.counter() && loaded.counter() > 0,
                    Component.literal("Counter did not survive NBT save/load"));
            helper.assertTrue(loaded.inventory().getAmountAsInt(0) == 7 && loaded.inventory().getResource(0).equals(iron),
                    Component.literal("Inventory did not survive NBT save/load"));
            helper.assertTrue(entity.getUpdateTag(registries).getLongOr("Counter", -1) == entity.counter(),
                    Component.literal("Update tag counter is missing"));
            helper.succeed();
        });
    }
}

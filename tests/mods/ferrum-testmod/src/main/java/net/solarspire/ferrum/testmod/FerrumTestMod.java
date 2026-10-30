package net.solarspire.ferrum.testmod;

import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.CreativeModeTabs;
import net.minecraft.world.level.block.entity.BlockEntityType;
import net.neoforged.bus.api.IEventBus;
import net.neoforged.fml.common.Mod;
import net.neoforged.neoforge.capabilities.Capabilities;
import net.neoforged.neoforge.capabilities.RegisterCapabilitiesEvent;
import net.neoforged.neoforge.event.BuildCreativeModeTabContentsEvent;
import net.neoforged.neoforge.network.event.RegisterPayloadHandlersEvent;
import net.neoforged.neoforge.registries.DeferredBlock;
import net.neoforged.neoforge.registries.DeferredHolder;
import net.neoforged.neoforge.registries.DeferredItem;
import net.neoforged.neoforge.registries.DeferredRegister;

@Mod(FerrumTestMod.ID)
public final class FerrumTestMod {
    public static final String ID = "ferrum_testmod";
    private static final DeferredRegister.Blocks BLOCKS = DeferredRegister.createBlocks(ID);
    private static final DeferredRegister.Items ITEMS = DeferredRegister.createItems(ID);
    private static final DeferredRegister<BlockEntityType<?>> BLOCK_ENTITIES =
            DeferredRegister.create(Registries.BLOCK_ENTITY_TYPE, ID);

    public static final DeferredBlock<CounterBlock> COUNTER_BLOCK =
            BLOCKS.registerBlock("counter_block", CounterBlock::new, properties -> properties.strength(1.5F));
    public static final DeferredItem<BlockItem> COUNTER_ITEM =
            ITEMS.registerSimpleBlockItem("counter_block", COUNTER_BLOCK);
    public static final DeferredHolder<BlockEntityType<?>, BlockEntityType<CounterBlockEntity>> COUNTER_ENTITY =
            BLOCK_ENTITIES.register("counter_block", () ->
                    new BlockEntityType<>(CounterBlockEntity::new, COUNTER_BLOCK.get()));

    public FerrumTestMod(IEventBus bus) {
        BLOCKS.register(bus);
        ITEMS.register(bus);
        BLOCK_ENTITIES.register(bus);
        CounterGameTests.registerFunctions(bus);
        bus.addListener(FerrumTestMod::capabilities);
        bus.addListener(FerrumTestMod::payloads);
        bus.addListener(FerrumTestMod::creativeTab);
        bus.addListener(CounterGameTests::register);
    }

    public static Identifier id(String path) {
        return Identifier.fromNamespaceAndPath(ID, path);
    }

    private static void capabilities(RegisterCapabilitiesEvent event) {
        event.registerBlockEntity(Capabilities.Item.BLOCK, COUNTER_ENTITY.get(),
                (entity, side) -> entity.inventory());
    }

    private static void payloads(RegisterPayloadHandlersEvent event) {
        event.registrar("1").playToClient(CounterPayload.TYPE, CounterPayload.CODEC);
    }

    private static void creativeTab(BuildCreativeModeTabContentsEvent event) {
        if (event.getTabKey() == CreativeModeTabs.FUNCTIONAL_BLOCKS) {
            event.accept(COUNTER_ITEM);
        }
    }
}

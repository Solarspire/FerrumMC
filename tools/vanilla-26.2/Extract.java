import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import java.lang.reflect.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.component.*;
import net.minecraft.core.registries.*;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.TagKey;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.item.*;
import net.minecraft.world.item.alchemy.*;
import net.minecraft.world.item.crafting.Ingredient;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.*;

/** Additional 26.2 extractor inputs. Run against the official server and its libraries. */
public final class Extract {
    static final Gson JSON = new GsonBuilder().setPrettyPrinting().create();
    static Path assets;
    static void write(String file, Object data) throws Exception { Files.writeString(assets.resolve(file+".json"),JSON.toJson(data)); }
    static Object field(Object owner, Class<?> type, String name) throws Exception {
        var field=type.getDeclaredField(name);field.setAccessible(true);return field.get(owner);
    }
    static List<Holder<Item>> tag(String name) throws Exception {
        var data=JsonParser.parseString(Files.readString(assets.resolve("datapack/data/minecraft/tags/item/"+name+".json"))).getAsJsonObject();
        var result=new LinkedHashSet<Holder<Item>>();
        for(var entry:data.getAsJsonArray("values")) {
            String id=entry.isJsonPrimitive()?entry.getAsString():entry.getAsJsonObject().get("id").getAsString();
            if(id.startsWith("#"))result.addAll(tag(id.substring(1).replace("minecraft:","")));
            else result.add(BuiltInRegistries.ITEM.get(Identifier.parse(id)).orElseThrow());
        }
        return new ArrayList<>(result);
    }
    public static void main(String[] args) throws Exception {
        assets=Path.of(args[0]);SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        if(SharedConstants.getCurrentVersion().protocolVersion()!=776)throw new IllegalStateException("Requires 26.2");
        var tags=new HashMap<TagKey<Item>,List<Holder<Item>>>();
        var tagDir=assets.resolve("datapack/data/minecraft/tags/item");
        try(var paths=Files.walk(tagDir)) {
            for(var path:paths.filter(p->p.toString().endsWith(".json")).toList()) {
                String name=tagDir.relativize(path).toString().replace('\\','/').replace(".json","");
                tags.put(TagKey.create(Registries.ITEM,Identifier.withDefaultNamespace(name)),tag(name));
            }
        }
        BuiltInRegistries.ITEM.prepareTagReload(new net.minecraft.tags.TagLoader.LoadResult<>(Registries.ITEM,tags)).apply();
        var lookup=RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(net.minecraft.data.registries.VanillaRegistries.createLookup()).forEach(c->c.apply());
        var fuelValues=FuelValues.vanillaBurnTimes(lookup,FeatureFlags.VANILLA_SET);
        var fuels=new TreeMap<Integer,Integer>();
        for(var item:fuelValues.fuelItems())fuels.put(BuiltInRegistries.ITEM.getId(item),fuelValues.burnDuration(new ItemStack(item)));
        write("fuels",fuels);
        var compost=new TreeMap<Integer,Float>();
        ComposterBlock.COMPOSTABLES.forEach((item,chance)->compost.put(BuiltInRegistries.ITEM.getId(item.asItem()),chance));
        write("composter_increase_chance",compost);
        var components=new TreeMap<String,Object>();
        for(var type:BuiltInRegistries.DATA_COMPONENT_TYPE) components.put(BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(type).toString(),Map.of("id",BuiltInRegistries.DATA_COMPONENT_TYPE.getId(type),"persistent",!type.isTransient(),"ignore_swap_animation",type.ignoreSwapAnimation()));
        write("data_component",components);
        var brewing=PotionBrewing.bootstrap(FeatureFlags.VANILLA_SET);
        var recipes=new ArrayList<Object>();
        // Evaluate the public mix operation, preserving vanilla's container/potion mix precedence.
        for(var potion:BuiltInRegistries.POTION.listElements().toList())for(var container:List.of(Items.POTION,Items.SPLASH_POTION,Items.LINGERING_POTION))for(var ingredient:BuiltInRegistries.ITEM) {
            var input=PotionContents.createItemStack(container,potion);
            var reagent=new ItemStack(ingredient);
            if(!brewing.hasMix(input,reagent))continue;
            var output=brewing.mix(reagent,input);
            var result=output.get(DataComponents.POTION_CONTENTS).potion().orElseThrow();
            recipes.add(Map.of("input",Map.of("item",BuiltInRegistries.ITEM.getKey(container).toString(),"potion_contents",Map.of("potion",potion.unwrapKey().orElseThrow().identifier().toString())),"reagent",Map.of("item",BuiltInRegistries.ITEM.getKey(ingredient).toString()),"output",Map.of("id",BuiltInRegistries.ITEM.getKey(output.getItem()).toString(),"components",Map.of("minecraft:potion_contents",Map.of("potion",result.unwrapKey().orElseThrow().identifier().toString())))));
        }
        write("brewing",recipes);
        var pots=new TreeMap<String,Object>();
        var registry=new MappedRegistry<DecoratedPotPattern>(Registries.DECORATED_POT_PATTERN,com.mojang.serialization.Lifecycle.stable());
        DecoratedPotPatterns.bootstrap(registry); registry.freeze();
        for(var pattern:registry)pots.put(registry.getKey(pattern).getPath(),Map.of("asset_id",pattern.assetId().toString()));
        write("decorated_pot_patterns",pots);
        var stripping=new TreeMap<String,String>();
        ((Map<Block,Block>)field(null,AxeItem.class,"STRIPPABLES")).forEach((a,b)->stripping.put(BuiltInRegistries.BLOCK.getKey(a).toString(),BuiltInRegistries.BLOCK.getKey(b).toString()));
        write("strippables",stripping);
        var flattening=new TreeMap<String,String>();
        ((Map<Block,net.minecraft.world.level.block.state.BlockState>)field(null,ShovelItem.class,"FLATTENABLES")).forEach((a,b)->flattening.put(BuiltInRegistries.BLOCK.getKey(a).toString(),BuiltInRegistries.BLOCK.getKey(b.getBlock()).toString()));
        write("flattenables",flattening);
        for(var entry:Map.of("scrapables",WeatheringCopper.PREVIOUS_BY_BLOCK.get(),"unwaxables",HoneycombItem.WAX_OFF_BY_BLOCK.get()).entrySet()) {
            var values=new TreeMap<String,String>();
            entry.getValue().forEach((a,b)->values.put(BuiltInRegistries.BLOCK.getKey(a).toString(),BuiltInRegistries.BLOCK.getKey(b).toString()));
            write(entry.getKey(),values);
        }
    }
}




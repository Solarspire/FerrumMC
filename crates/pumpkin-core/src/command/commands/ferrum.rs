use crate::command::{
    argument_builder::{ArgumentBuilder, argument, command, literal},
    argument_types::{
        coordinates::block_pos::BlockPosArgumentType, core::integer::IntegerArgumentType,
        core::string::StringArgumentType, entity::EntityArgumentType,
    },
    context::command_context::CommandContext,
    node::{CommandExecutor, CommandExecutorResult, dispatcher::CommandDispatcher},
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{
    PermissionLvl,
    permission::{Permission, PermissionDefault, PermissionRegistry},
    text::TextComponent,
};

const PERMISSION: &str = "ferrum:command.be";
struct DumpBlockEntity;
impl CommandExecutor for DumpBlockEntity {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;
        let world = context.source.world();
        world.get_block_entity(&pos).map_or_else(
            || {
                context.source.send_feedback(
                    TextComponent::text("No block entity at this position"),
                    false,
                );
                Ok(0)
            },
            |be| {
                let mut nbt = NbtCompound::new();
                be.write_internal(&mut nbt);
                context
                    .source
                    .send_feedback(TextComponent::text(nbt.to_string()), false);
                Ok(1)
            },
        )
    }
}
struct SummonModEntity;
impl CommandExecutor for SummonModEntity {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;
        let entity_type = StringArgumentType::get(context, "type")?;
        let world = context.source.world();
        let position = pumpkin_util::math::vector3::Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y),
            f64::from(pos.0.z) + 0.5,
        );
        match crate::neoforge::summon(world, entity_type, position) {
            Ok(Some(id)) => {
                context.source.send_feedback(
                    TextComponent::text(format!("Summoned {entity_type} #{id}")),
                    false,
                );
                Ok(1)
            }
            Ok(None) => {
                context.source.send_feedback(
                    TextComponent::text(format!("Cannot summon {entity_type}")),
                    false,
                );
                Ok(0)
            }
            Err(error) => {
                context
                    .source
                    .send_feedback(TextComponent::text(error), false);
                Ok(0)
            }
        }
    }
}
struct CountEntities;
impl CommandExecutor for CountEntities {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let entity_type = StringArgumentType::get(context, "type")?;
        let mut found = Vec::new();
        context
            .source
            .world()
            .extend_entities_where(&mut found, usize::MAX - 1, |entity| {
                entity.get_entity().entity_type.resource_name == entity_type
            });
        context.source.send_feedback(
            TextComponent::text(format!("Entities {entity_type}: {}", found.len())),
            false,
        );
        Ok(i32::try_from(found.len()).unwrap_or(i32::MAX))
    }
}
struct CountBlocks;
impl CommandExecutor for CountBlocks {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;
        let radius = IntegerArgumentType::get(context, "radius")?;
        let prefix = StringArgumentType::get(context, "prefix")?;
        let world = context.source.world();
        let mut count = 0;
        for x in -radius..=radius {
            for y in -radius..=radius {
                for z in -radius..=radius {
                    let at = pos.offset(pumpkin_util::math::vector3::Vector3::new(x, y, z));
                    if world.get_block(&at).name.starts_with(prefix) {
                        count += 1;
                    }
                }
            }
        }
        context.source.send_feedback(
            TextComponent::text(format!("Blocks {prefix}: {count}")),
            false,
        );
        Ok(count)
    }
}
struct DumpItem;
impl CommandExecutor for DumpItem {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let slot = IntegerArgumentType::get(context, "slot")?;
        let mut shown = 0;
        for player in EntityArgumentType::get_players(context, "target")? {
            let stack = player
                .inventory()
                .get_slot(usize::try_from(slot).unwrap_or(usize::MAX));
            let mut nbt = NbtCompound::new();
            stack.write_item_stack(&mut nbt);
            context
                .source
                .send_feedback(TextComponent::text(format!("slot {slot}: {nbt}")), false);
            shown += 1;
        }
        Ok(shown)
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        "Inspect block entity NBT",
        PermissionDefault::Op(PermissionLvl::Two),
    ));
    dispatcher.register(
        command("ferrum", "Inspect the FerrumMC bridge")
            .requires(PERMISSION)
            .then(
                literal("be").then(argument("pos", BlockPosArgumentType).executes(DumpBlockEntity)),
            )
            .then(
                literal("summon").then(argument("pos", BlockPosArgumentType).then(
                    argument("type", StringArgumentType::GreedyPhrase).executes(SummonModEntity),
                )),
            )
            .then(
                literal("blocks").then(argument("pos", BlockPosArgumentType).then(
                    argument("radius", IntegerArgumentType::with_min(0)).then(
                        argument("prefix", StringArgumentType::GreedyPhrase).executes(CountBlocks),
                    ),
                )),
            )
            .then(
                literal("count").then(
                    argument("type", StringArgumentType::GreedyPhrase).executes(CountEntities),
                ),
            )
            .then(
                literal("item").then(
                    argument("target", EntityArgumentType::Players).then(
                        argument("slot", IntegerArgumentType::with_min(0)).executes(DumpItem),
                    ),
                ),
            ),
    );
}

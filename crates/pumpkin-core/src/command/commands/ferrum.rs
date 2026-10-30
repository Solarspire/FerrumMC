use crate::command::{
    argument_builder::{ArgumentBuilder, argument, command, literal},
    argument_types::coordinates::block_pos::BlockPosArgumentType,
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
            ),
    );
}

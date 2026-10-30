use super::{AccessorExt, in_active_context, run_blocking};
use crate::LockIgnorePoison;
use crate::RequireServer;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use wasmtime::component::{Accessor, HasSelf, Resource};

use crate::pumpkin::plugin::java_dialogs::Dialog;
use crate::{
    events::{from_wasm_game_mode, from_wasm_position, to_wasm_game_mode, to_wasm_position},
    living_entity::from_wit_damage_type,
    pumpkin::{
        self,
        plugin::{
            damage_types::DamageType as WitDamageType,
            player::{Player, PlayerSkin, SkinParts},
            statistics::{
                CustomStatistic as WitCustomStatistic, StatisticCategory as WitStatisticCategory,
            },
            uuid::Uuid,
            world::World,
        },
    },
    uuid::UuidExt,
};
use pumpkin_core::entity::{
    EntityBase,
    player::{
        TitleMode,
        advancement::{AdvancementAward, PlayerAdvancement},
    },
};
use pumpkin_core::plugin::api::gui::PluginScreenHandler;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_protocol::Property;

use pumpkin_protocol::java::client::dialog::DialogNBT;
use pumpkin_util::permission::PermissionLvl;
use pumpkin_wasm_host_common::state::PluginHostState;

use pumpkin_util::version::JavaMinecraftVersion;

const fn from_wit_client_game_event(
    event: pumpkin::plugin::player::ClientGameEvent,
) -> pumpkin_protocol::java::client::play::GameEvent {
    use pumpkin::plugin::player::ClientGameEvent as W;
    use pumpkin_protocol::java::client::play::GameEvent as G;
    match event {
        W::NoRespawnBlockAvailable => G::NoRespawnBlockAvailable,
        W::BeginRaining => G::BeginRaining,
        W::EndRaining => G::EndRaining,
        W::ChangeGameMode => G::ChangeGameMode,
        W::WinGame => G::WinGame,
        W::DemoEvent => G::DemoEvent,
        W::ArrowHitPlayer => G::ArrowHitPlayer,
        W::RainLevelChange => G::RainLevelChange,
        W::ThunderLevelChange => G::ThunderLevelChange,
        W::PlayPufferfishStringSound => G::PlayPufferfishStringSound,
        W::PlayElderGuardianMobAppearance => G::PlayElderGuardianMobAppearance,
        W::EnabledRespawnScreen => G::EnabledRespawnScreen,
        W::LimitedCrafting => G::LimitedCrafting,
        W::StartWaitingChunks => G::StartWaitingChunks,
    }
}

pub(crate) fn from_wit_server_link(
    state: &mut PluginHostState,
    link: pumpkin::plugin::player::ServerLink,
) -> wasmtime::Result<(pumpkin_protocol::Label, String)> {
    use pumpkin::plugin::player::{KnownServerLink, ServerLinkLabel};
    let label = match link.label {
        ServerLinkLabel::Known(known) => {
            let link_type = match known {
                KnownServerLink::BugReport => pumpkin_protocol::LinkType::BugReport,
                KnownServerLink::CommunityGuidelines => {
                    pumpkin_protocol::LinkType::CommunityGuidelines
                }
                KnownServerLink::Support => pumpkin_protocol::LinkType::Support,
                KnownServerLink::Status => pumpkin_protocol::LinkType::Status,
                KnownServerLink::Feedback => pumpkin_protocol::LinkType::Feedback,
                KnownServerLink::Community => pumpkin_protocol::LinkType::Community,
                KnownServerLink::Website => pumpkin_protocol::LinkType::Website,
                KnownServerLink::Forums => pumpkin_protocol::LinkType::Forums,
                KnownServerLink::News => pumpkin_protocol::LinkType::News,
                KnownServerLink::Announcements => pumpkin_protocol::LinkType::Announcements,
            };
            pumpkin_protocol::Label::BuiltIn(link_type)
        }
        ServerLinkLabel::Custom(text) => {
            let text_res = state.take(text)?;
            pumpkin_protocol::Label::TextComponent(Box::new(text_res))
        }
    };
    Ok((label, link.url))
}

const fn to_wasm_java_version(
    version: JavaMinecraftVersion,
) -> pumpkin::plugin::player::JavaMinecraftVersion {
    match version {
        JavaMinecraftVersion::V_1_7_2 => pumpkin::plugin::player::JavaMinecraftVersion::V172,
        JavaMinecraftVersion::V_1_7_6 => pumpkin::plugin::player::JavaMinecraftVersion::V176,
        JavaMinecraftVersion::V_1_8 => pumpkin::plugin::player::JavaMinecraftVersion::V18,
        JavaMinecraftVersion::V_1_9 => pumpkin::plugin::player::JavaMinecraftVersion::V19,
        JavaMinecraftVersion::V_1_9_1 => pumpkin::plugin::player::JavaMinecraftVersion::V191,
        JavaMinecraftVersion::V_1_9_2 => pumpkin::plugin::player::JavaMinecraftVersion::V192,
        JavaMinecraftVersion::V_1_9_3 => pumpkin::plugin::player::JavaMinecraftVersion::V193,
        JavaMinecraftVersion::V_1_10 => pumpkin::plugin::player::JavaMinecraftVersion::V110,
        JavaMinecraftVersion::V_1_11 => pumpkin::plugin::player::JavaMinecraftVersion::V111,
        JavaMinecraftVersion::V_1_11_1 => pumpkin::plugin::player::JavaMinecraftVersion::V1111,
        JavaMinecraftVersion::V_1_12 => pumpkin::plugin::player::JavaMinecraftVersion::V112,
        JavaMinecraftVersion::V_1_12_1 => pumpkin::plugin::player::JavaMinecraftVersion::V1121,
        JavaMinecraftVersion::V_1_12_2 => pumpkin::plugin::player::JavaMinecraftVersion::V1122,
        JavaMinecraftVersion::V_1_13 => pumpkin::plugin::player::JavaMinecraftVersion::V113,
        JavaMinecraftVersion::V_1_13_1 => pumpkin::plugin::player::JavaMinecraftVersion::V1131,
        JavaMinecraftVersion::V_1_13_2 => pumpkin::plugin::player::JavaMinecraftVersion::V1132,
        JavaMinecraftVersion::V_1_14 => pumpkin::plugin::player::JavaMinecraftVersion::V114,
        JavaMinecraftVersion::V_1_14_1 => pumpkin::plugin::player::JavaMinecraftVersion::V1141,
        JavaMinecraftVersion::V_1_14_2 => pumpkin::plugin::player::JavaMinecraftVersion::V1142,
        JavaMinecraftVersion::V_1_14_3 => pumpkin::plugin::player::JavaMinecraftVersion::V1143,
        JavaMinecraftVersion::V_1_14_4 => pumpkin::plugin::player::JavaMinecraftVersion::V1144,
        JavaMinecraftVersion::V_1_15 => pumpkin::plugin::player::JavaMinecraftVersion::V115,
        JavaMinecraftVersion::V_1_15_1 => pumpkin::plugin::player::JavaMinecraftVersion::V1151,
        JavaMinecraftVersion::V_1_15_2 => pumpkin::plugin::player::JavaMinecraftVersion::V1152,
        JavaMinecraftVersion::V_1_16 => pumpkin::plugin::player::JavaMinecraftVersion::V116,
        JavaMinecraftVersion::V_1_16_1 => pumpkin::plugin::player::JavaMinecraftVersion::V1161,
        JavaMinecraftVersion::V_1_16_2 => pumpkin::plugin::player::JavaMinecraftVersion::V1162,
        JavaMinecraftVersion::V_1_16_3 => pumpkin::plugin::player::JavaMinecraftVersion::V1163,
        JavaMinecraftVersion::V_1_16_4 => pumpkin::plugin::player::JavaMinecraftVersion::V1164,
        JavaMinecraftVersion::V_1_17 => pumpkin::plugin::player::JavaMinecraftVersion::V117,
        JavaMinecraftVersion::V_1_17_1 => pumpkin::plugin::player::JavaMinecraftVersion::V1171,
        JavaMinecraftVersion::V_1_18 => pumpkin::plugin::player::JavaMinecraftVersion::V118,
        JavaMinecraftVersion::V_1_18_2 => pumpkin::plugin::player::JavaMinecraftVersion::V1182,
        JavaMinecraftVersion::V_1_19 => pumpkin::plugin::player::JavaMinecraftVersion::V119,
        JavaMinecraftVersion::V_1_19_1 => pumpkin::plugin::player::JavaMinecraftVersion::V1191,
        JavaMinecraftVersion::V_1_19_3 => pumpkin::plugin::player::JavaMinecraftVersion::V1193,
        JavaMinecraftVersion::V_1_19_4 => pumpkin::plugin::player::JavaMinecraftVersion::V1194,
        JavaMinecraftVersion::V_1_20 => pumpkin::plugin::player::JavaMinecraftVersion::V120,
        JavaMinecraftVersion::V_1_20_2 => pumpkin::plugin::player::JavaMinecraftVersion::V1202,
        JavaMinecraftVersion::V_1_20_3 => pumpkin::plugin::player::JavaMinecraftVersion::V1203,
        JavaMinecraftVersion::V_1_20_5 => pumpkin::plugin::player::JavaMinecraftVersion::V1205,
        JavaMinecraftVersion::V_1_21 => pumpkin::plugin::player::JavaMinecraftVersion::V121,
        JavaMinecraftVersion::V_1_21_2 => pumpkin::plugin::player::JavaMinecraftVersion::V1212,
        JavaMinecraftVersion::V_1_21_4 => pumpkin::plugin::player::JavaMinecraftVersion::V1214,
        JavaMinecraftVersion::V_1_21_5 => pumpkin::plugin::player::JavaMinecraftVersion::V1215,
        JavaMinecraftVersion::V_1_21_6 => pumpkin::plugin::player::JavaMinecraftVersion::V1216,
        JavaMinecraftVersion::V_1_21_7 => pumpkin::plugin::player::JavaMinecraftVersion::V1217,
        JavaMinecraftVersion::V_1_21_9 => pumpkin::plugin::player::JavaMinecraftVersion::V1219,
        JavaMinecraftVersion::V_1_21_11 => pumpkin::plugin::player::JavaMinecraftVersion::V12111,
        JavaMinecraftVersion::V_26_1 => pumpkin::plugin::player::JavaMinecraftVersion::V261,
        JavaMinecraftVersion::V_26_2 => pumpkin::plugin::player::JavaMinecraftVersion::V262,
        JavaMinecraftVersion::V_26_3 => pumpkin::plugin::player::JavaMinecraftVersion::V263,
        JavaMinecraftVersion::Unknown => pumpkin::plugin::player::JavaMinecraftVersion::Unknown,
    }
}

const fn to_wasm_chat_mode(
    mode: &pumpkin_core::entity::player::ChatMode,
) -> pumpkin::plugin::player::ChatMode {
    match mode {
        pumpkin_core::entity::player::ChatMode::Enabled => {
            pumpkin::plugin::player::ChatMode::Enabled
        }
        pumpkin_core::entity::player::ChatMode::CommandsOnly => {
            pumpkin::plugin::player::ChatMode::CommandsOnly
        }
        pumpkin_core::entity::player::ChatMode::Hidden => pumpkin::plugin::player::ChatMode::Hidden,
    }
}

#[must_use]
pub const fn to_wit_statistic_category(
    category: pumpkin_data::statistic::StatisticCategory,
) -> WitStatisticCategory {
    match category {
        pumpkin_data::statistic::StatisticCategory::Mined => WitStatisticCategory::Mined,
        pumpkin_data::statistic::StatisticCategory::Crafted => WitStatisticCategory::Crafted,
        pumpkin_data::statistic::StatisticCategory::Used => WitStatisticCategory::Used,
        pumpkin_data::statistic::StatisticCategory::Broken => WitStatisticCategory::Broken,
        pumpkin_data::statistic::StatisticCategory::PickedUp => WitStatisticCategory::PickedUp,
        pumpkin_data::statistic::StatisticCategory::Dropped => WitStatisticCategory::Dropped,
        pumpkin_data::statistic::StatisticCategory::Killed => WitStatisticCategory::Killed,
        pumpkin_data::statistic::StatisticCategory::KilledBy => WitStatisticCategory::KilledBy,
        pumpkin_data::statistic::StatisticCategory::Custom => WitStatisticCategory::Custom,
    }
}

#[must_use]
pub const fn from_wit_statistic_category(
    wit: WitStatisticCategory,
) -> pumpkin_data::statistic::StatisticCategory {
    match wit {
        WitStatisticCategory::Mined => pumpkin_data::statistic::StatisticCategory::Mined,
        WitStatisticCategory::Crafted => pumpkin_data::statistic::StatisticCategory::Crafted,
        WitStatisticCategory::Used => pumpkin_data::statistic::StatisticCategory::Used,
        WitStatisticCategory::Broken => pumpkin_data::statistic::StatisticCategory::Broken,
        WitStatisticCategory::PickedUp => pumpkin_data::statistic::StatisticCategory::PickedUp,
        WitStatisticCategory::Dropped => pumpkin_data::statistic::StatisticCategory::Dropped,
        WitStatisticCategory::Killed => pumpkin_data::statistic::StatisticCategory::Killed,
        WitStatisticCategory::KilledBy => pumpkin_data::statistic::StatisticCategory::KilledBy,
        WitStatisticCategory::Custom => pumpkin_data::statistic::StatisticCategory::Custom,
    }
}

#[must_use]
pub const fn to_wit_custom_statistic(
    stat: pumpkin_data::statistic::CustomStatistic,
) -> WitCustomStatistic {
    // SAFETY: WitCustomStatistic is generated in the same numerical order as CustomStatistic
    unsafe { std::mem::transmute(stat as u8) }
}

#[must_use]
pub fn from_wit_custom_statistic(
    wit: WitCustomStatistic,
) -> pumpkin_data::statistic::CustomStatistic {
    pumpkin_data::statistic::CustomStatistic::from_i32(wit as i32)
        .unwrap_or(pumpkin_data::statistic::CustomStatistic::PlayTime)
}

pub(crate) fn text_component_from_resource(
    state: &PluginHostState,
    text: &Resource<pumpkin::plugin::text::TextComponent>,
) -> pumpkin_util::text::TextComponent {
    state
        .get(text)
        .expect("invalid text-component resource handle")
        .clone()
}

pub(crate) const fn to_wit_permission_level(
    level: PermissionLvl,
) -> pumpkin::plugin::permission::PermissionLevel {
    match level {
        PermissionLvl::Zero => pumpkin::plugin::permission::PermissionLevel::Zero,
        PermissionLvl::One => pumpkin::plugin::permission::PermissionLevel::One,
        PermissionLvl::Two => pumpkin::plugin::permission::PermissionLevel::Two,
        PermissionLvl::Three => pumpkin::plugin::permission::PermissionLevel::Three,
        PermissionLvl::Four => pumpkin::plugin::permission::PermissionLevel::Four,
    }
}

pub(crate) const fn from_wit_permission_level(
    level: pumpkin::plugin::permission::PermissionLevel,
) -> PermissionLvl {
    match level {
        pumpkin::plugin::permission::PermissionLevel::Zero => PermissionLvl::Zero,
        pumpkin::plugin::permission::PermissionLevel::One => PermissionLvl::One,
        pumpkin::plugin::permission::PermissionLevel::Two => PermissionLvl::Two,
        pumpkin::plugin::permission::PermissionLevel::Three => PermissionLvl::Three,
        pumpkin::plugin::permission::PermissionLevel::Four => PermissionLvl::Four,
    }
}

pub(crate) fn parse_ban_expiry(
    expires_at_utc: Option<String>,
    duration_seconds: Option<u64>,
) -> Option<time::OffsetDateTime> {
    if let Some(dur) = duration_seconds {
        let seconds = i64::try_from(dur).unwrap_or(i64::MAX);
        return Some(time::OffsetDateTime::now_utc() + time::Duration::seconds(seconds));
    }
    if let Some(s) = expires_at_utc {
        if s.eq_ignore_ascii_case("forever") || s.is_empty() {
            return None;
        }
        if let Ok(parsed) =
            time::OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339)
        {
            return Some(parsed);
        }
        if let Ok(parsed) = time::OffsetDateTime::parse(
            &s,
            time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute]:[second][offset_hour sign:mandatory]:[offset_minute]"
            ),
        ) {
            return Some(parsed);
        }
    }
    None
}

impl pumpkin::plugin::player::Host for PluginHostState {
    fn get_world_players(
        &mut self,
        world: Resource<pumpkin::plugin::world::World>,
    ) -> wasmtime::Result<Vec<Resource<pumpkin::plugin::player::Player>>> {
        let world = self.take(world)?;

        let mut players = Vec::new();
        for player in world.players.load().iter() {
            players.push(self.add(player.clone())?);
        }

        Ok(players)
    }
}
use crate::events::from_wasm_hand;
use pumpkin_inventory::Clearable;
use pumpkin_inventory::Inventory;
use pumpkin_inventory::generic_container_screen_handler::GenericContainerScreenHandler;
use pumpkin_inventory::player::ender_chest_inventory::EnderChestInventory;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::java::client::play::CSetContainerSlot;

use crate::pumpkin::plugin::item_stack::ItemStack as WitHostItemStack;

impl pumpkin::plugin::player::HostPlayer for PluginHostState {
    fn get_inventory(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Resource<crate::pumpkin::plugin::inventory::PlayerInventory>> {
        self.add(self.get(&player)?.clone())
    }

    fn get_ender_chest(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Resource<crate::pumpkin::plugin::inventory::Inventory>> {
        self.add(
            pumpkin_wasm_host_common::state::InventoryProvider::PlayerEnderChest(
                self.get(&player)?.clone(),
            ),
        )
    }

    fn get_inventory_item(
        &mut self,
        player: Resource<Player>,
        slot: u8,
    ) -> wasmtime::Result<Option<Resource<WitHostItemStack>>> {
        let player = self.get(&player)?;
        let stack = player.inventory().get_stack(slot as usize);
        if stack.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.add(Arc::new(tokio::sync::Mutex::new(stack)))?))
        }
    }

    fn get_ender_chest_item(
        &mut self,
        player: Resource<Player>,
        slot: u8,
    ) -> wasmtime::Result<Option<Resource<WitHostItemStack>>> {
        let player = self.get(&player)?;
        let ec = player.ender_chest_inventory();
        let stack = ec.get_stack(slot as usize);
        if stack.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.add(Arc::new(tokio::sync::Mutex::new(stack)))?))
        }
    }

    fn get_item_in_hand(
        &mut self,
        player: Resource<Player>,
        hand: pumpkin::plugin::common::Hand,
    ) -> wasmtime::Result<Option<Resource<WitHostItemStack>>> {
        let player = self.get(&player)?;
        let hand = from_wasm_hand(hand);
        let stack = player.inventory().get_stack_in_hand(hand);
        if stack.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.add(Arc::new(tokio::sync::Mutex::new(stack)))?))
        }
    }

    fn as_entity(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Resource<pumpkin::plugin::world::Entity>> {
        self.add(self.get(&player)?.clone() as _)
    }

    fn get_id(&mut self, player: Resource<Player>) -> wasmtime::Result<Uuid> {
        let player = self.get(&player)?;
        Ok(Uuid::to_wit(&player.gameprofile.id))
    }

    fn get_name(&mut self, player: Resource<Player>) -> wasmtime::Result<String> {
        let player = self.get(&player)?;
        Ok(player.gameprofile.name.clone())
    }

    fn get_position(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<pumpkin::plugin::common::Position> {
        let player = self.get(&player)?;
        Ok(to_wasm_position(player.position()))
    }

    fn get_yaw(&mut self, player: Resource<Player>) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.get_entity().yaw.load())
    }

    fn get_pitch(&mut self, player: Resource<Player>) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.get_entity().pitch.load())
    }

    fn get_world(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Resource<pumpkin::plugin::world::World>> {
        let player = self.get(&player)?;
        let world = player.world();
        self.add(world)
            .map_err(|_| wasmtime::Error::msg("failed to add world resource"))
    }

    fn get_gamemode(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<pumpkin::plugin::common::GameMode> {
        let player = self.get(&player)?;
        Ok(to_wasm_game_mode(player.gamemode.load()))
    }

    fn get_locale(&mut self, player: Resource<Player>) -> wasmtime::Result<String> {
        let player = self.get(&player)?;
        Ok(player.config.load().locale.clone())
    }

    fn get_ping(&mut self, player: Resource<Player>) -> wasmtime::Result<u32> {
        let player = self.get(&player)?;
        Ok(player.ping.load(Ordering::Relaxed))
    }

    fn get_permission_level(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<pumpkin::plugin::permission::PermissionLevel> {
        let player = self.get(&player)?;
        Ok(to_wit_permission_level(player.permission_lvl.load()))
    }

    fn set_permission(
        &mut self,
        player: Resource<Player>,
        node: String,
        value: bool,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let server = self.require_server()?;

        server
            .permission_manager
            .set_permission(player.gameprofile.id, node, value);

        Ok(())
    }

    fn unset_permission(&mut self, player: Resource<Player>, node: String) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let server = self.require_server()?;

        server
            .permission_manager
            .unset_permission(&player.gameprofile.id, &node);

        Ok(())
    }

    fn has_permission_set(
        &mut self,
        player: Resource<Player>,
        node: String,
    ) -> wasmtime::Result<Option<bool>> {
        let player = self.get(&player)?;
        let server = self.require_server()?;

        Ok(server
            .permission_manager
            .has_permission_set(&player.gameprofile.id, &node))
    }

    fn get_display_name(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Resource<pumpkin::plugin::text::TextComponent>> {
        let player = self.get(&player)?;
        let display_name = player.get_display_name();
        self.add(display_name)
            .map_err(|_| wasmtime::Error::msg("failed to add text-component resource"))
    }

    fn set_display_name(
        &mut self,
        player: Resource<Player>,
        display_name: Resource<pumpkin::plugin::text::TextComponent>,
    ) -> wasmtime::Result<()> {
        let display_name = self.take(display_name)?;
        let player = self.get(&player)?;
        player.set_display_name(Some(display_name));
        Ok(())
    }

    fn get_tab_list_name(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Option<Resource<pumpkin::plugin::text::TextComponent>>> {
        let player = self.get(&player)?;
        let tab_list_name = player.get_tab_list_name();
        tab_list_name.map_or_else(
            || Ok(None),
            |name| {
                self.add(name)
                    .map(Some)
                    .map_err(|_| wasmtime::Error::msg("failed to add text-component resource"))
            },
        )
    }

    fn set_tab_list_name(
        &mut self,
        player: Resource<Player>,
        name: Option<Resource<pumpkin::plugin::text::TextComponent>>,
    ) -> wasmtime::Result<()> {
        let name = name.map(|n| self.take(n)).transpose()?;
        let player = self.get(&player)?;
        player.set_tab_list_name(name);
        Ok(())
    }

    fn send_system_message(
        &mut self,
        player: Resource<Player>,
        text: Resource<pumpkin::plugin::text::TextComponent>,
        overlay: bool,
    ) -> wasmtime::Result<()> {
        let component = self.take(text)?;
        let player = self.get(&player)?;
        player.send_system_message_raw(&component, overlay);
        Ok(())
    }

    fn set_camera(
        &mut self,
        player: Resource<Player>,
        entity: Option<Resource<crate::pumpkin::plugin::world::Entity>>,
    ) -> wasmtime::Result<()> {
        let entity = entity.map(|r| self.take(r)).transpose()?;
        let player = self.get(&player)?;
        if let Some(target) = entity {
            player.set_camera_entity_id(target.get_entity().entity_id);
        } else {
            player.reset_camera();
        }
        Ok(())
    }

    fn set_camera_entity_id(
        &mut self,
        player: Resource<Player>,
        entity_id: u32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_camera_entity_id(entity_id as i32);
        Ok(())
    }

    fn get_camera_entity_id(&mut self, player: Resource<Player>) -> wasmtime::Result<u32> {
        let player = self.get(&player)?;
        Ok(player.get_camera_entity_id() as u32)
    }

    fn reset_camera(&mut self, player: Resource<Player>) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.reset_camera();
        Ok(())
    }

    fn play_sound(
        &mut self,
        player: Resource<Player>,
        sound: pumpkin::plugin::sounds::Sound,
        category: pumpkin::plugin::sounds::SoundCategory,
        volume: f32,
        pitch: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let sound_name = format!("{sound:?}").to_lowercase().replace('_', ".");
        let sound_data = pumpkin_data::sound::Sound::from_name(&sound_name)
            .ok_or_else(|| wasmtime::Error::msg(format!("Unknown sound: {sound_name}")))?;
        let internal_category = super::world::from_wit_sound_category(category);
        let pos = player.position();
        player.play_sound(
            sound_data as u16,
            internal_category,
            &pos,
            volume,
            pitch,
            rand::random::<i64>(),
        );
        Ok(())
    }

    fn play_sound_at(
        &mut self,
        player: Resource<Player>,
        pos: pumpkin::plugin::common::Position,
        sound: pumpkin::plugin::sounds::Sound,
        category: pumpkin::plugin::sounds::SoundCategory,
        volume: f32,
        pitch: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let sound_name = format!("{sound:?}").to_lowercase().replace('_', ".");
        let sound_data = pumpkin_data::sound::Sound::from_name(&sound_name)
            .ok_or_else(|| wasmtime::Error::msg(format!("Unknown sound: {sound_name}")))?;
        let internal_category = super::world::from_wit_sound_category(category);
        player.play_sound(
            sound_data as u16,
            internal_category,
            &pumpkin_util::math::vector3::Vector3::new(pos.0, pos.1, pos.2),
            volume,
            pitch,
            rand::random::<i64>(),
        );
        Ok(())
    }

    fn stop_sound(
        &mut self,
        player: Resource<Player>,
        sound: Option<pumpkin::plugin::sounds::Sound>,
        category: Option<pumpkin::plugin::sounds::SoundCategory>,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let sound_rl = sound.and_then(|s| {
            let sound_name = format!("{s:?}").to_lowercase().replace('_', ".");
            pumpkin_data::sound::Sound::from_name(&sound_name).map(|s| s.to_name().into())
        });
        let cat = category.map(super::world::from_wit_sound_category);
        player.stop_sound(sound_rl, cat);
        Ok(())
    }

    fn play_custom_sound(
        &mut self,
        player: Resource<Player>,
        sound_name: String,
        category: pumpkin::plugin::sounds::SoundCategory,
        volume: f32,
        pitch: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let internal_category = super::world::from_wit_sound_category(category);
        let pos = player.position();
        player.play_custom_sound(&sound_name, internal_category, &pos, volume, pitch);
        Ok(())
    }

    fn play_custom_sound_at(
        &mut self,
        player: Resource<Player>,
        pos: pumpkin::plugin::common::Position,
        sound_name: String,
        category: pumpkin::plugin::sounds::SoundCategory,
        volume: f32,
        pitch: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let internal_category = super::world::from_wit_sound_category(category);
        player.play_custom_sound(
            &sound_name,
            internal_category,
            &pumpkin_util::math::vector3::Vector3::new(pos.0, pos.1, pos.2),
            volume,
            pitch,
        );
        Ok(())
    }

    fn stop_custom_sound(
        &mut self,
        player: Resource<Player>,
        sound_name: Option<String>,
        category: Option<pumpkin::plugin::sounds::SoundCategory>,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let sound_rl = sound_name
            .as_deref()
            .map(pumpkin_util::resource_location::ResourceLocation::from);
        let cat = category.map(super::world::from_wit_sound_category);
        player.stop_sound(sound_rl, cat);
        Ok(())
    }

    fn spawn_particles(
        &mut self,
        player: Resource<Player>,
        particle: pumpkin::plugin::particles::Particle,
        pos: pumpkin::plugin::common::Position,
        count: u32,
        offset: pumpkin::plugin::common::Position,
        max_speed: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let particle_data =
            pumpkin_data::particle::Particle::from_id(particle as u16).ok_or_else(|| {
                wasmtime::Error::msg(format!("Unknown particle ID: {}", particle as u16))
            })?;
        player.spawn_particles(
            particle_data,
            pumpkin_util::math::vector3::Vector3::new(pos.0, pos.1, pos.2),
            count,
            pumpkin_util::math::vector3::Vector3::new(
                offset.0 as f32,
                offset.1 as f32,
                offset.2 as f32,
            ),
            max_speed,
        );
        Ok(())
    }

    fn send_block_change(
        &mut self,
        player: Resource<Player>,
        pos: pumpkin::plugin::common::BlockPos,
        block_id: u16,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.send_block_change(
            pumpkin_util::math::position::BlockPos(pumpkin_util::math::vector3::Vector3::new(
                pos.x, pos.y, pos.z,
            )),
            block_id,
        );
        Ok(())
    }

    fn reset_block_change(
        &mut self,
        player: Resource<Player>,
        pos: pumpkin::plugin::common::BlockPos,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.reset_block_change(pumpkin_util::math::position::BlockPos(
            pumpkin_util::math::vector3::Vector3::new(pos.x, pos.y, pos.z),
        ));
        Ok(())
    }

    fn send_hurt_animation(&mut self, player: Resource<Player>, yaw: f32) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.send_hurt_animation(yaw);
        Ok(())
    }

    fn send_block_damage(
        &mut self,
        player: Resource<Player>,
        pos: pumpkin::plugin::common::BlockPos,
        progress: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let stage = if (0.0..=1.0).contains(&progress) {
            (progress * 10.0).floor().clamp(0.0, 9.0) as i8
        } else {
            -1
        };
        let block_pos = pumpkin_util::math::position::BlockPos(
            pumpkin_util::math::vector3::Vector3::new(pos.x, pos.y, pos.z),
        );
        let packet = pumpkin_protocol::java::client::play::CSetBlockDestroyStage::new(
            player.entity_id().into(),
            block_pos,
            stage,
        );
        player.try_send_client_packet(&packet);
        Ok(())
    }

    fn look_at(
        &mut self,
        player: Resource<Player>,
        target: pumpkin::plugin::common::Position,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let target_vec = from_wasm_position(target);
        let packet = pumpkin_protocol::java::client::play::CPlayerLookAt::at_position(
            pumpkin_protocol::codec::var_int::VarInt(1),
            target_vec.x,
            target_vec.y,
            target_vec.z,
        );
        player.try_send_client_packet(&packet);
        Ok(())
    }

    fn add_custom_chat_completions(
        &mut self,
        player: Resource<Player>,
        completions: Vec<String>,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let str_refs: Vec<&str> = completions.iter().map(String::as_str).collect();
        let packet = pumpkin_protocol::java::client::play::CCustomChatCompletions::new(
            pumpkin_protocol::codec::var_int::VarInt(0),
            &str_refs,
        );
        player.try_send_client_packet(&packet);
        Ok(())
    }

    fn remove_custom_chat_completions(
        &mut self,
        player: Resource<Player>,
        completions: Vec<String>,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let str_refs: Vec<&str> = completions.iter().map(String::as_str).collect();
        let packet = pumpkin_protocol::java::client::play::CCustomChatCompletions::new(
            pumpkin_protocol::codec::var_int::VarInt(1),
            &str_refs,
        );
        player.try_send_client_packet(&packet);
        Ok(())
    }

    fn set_custom_chat_completions(
        &mut self,
        player: Resource<Player>,
        completions: Vec<String>,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let str_refs: Vec<&str> = completions.iter().map(String::as_str).collect();
        let packet = pumpkin_protocol::java::client::play::CCustomChatCompletions::new(
            pumpkin_protocol::codec::var_int::VarInt(2),
            &str_refs,
        );
        player.try_send_client_packet(&packet);
        Ok(())
    }

    fn open_book(
        &mut self,
        player: Resource<Player>,
        hand: pumpkin::plugin::common::Hand,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let hand = match hand {
            pumpkin::plugin::common::Hand::Right => pumpkin_util::Hand::Right,
            pumpkin::plugin::common::Hand::Left => pumpkin_util::Hand::Left,
        };
        player.open_book(hand);
        Ok(())
    }

    fn open_sign_editor(
        &mut self,
        player: Resource<Player>,
        pos: pumpkin::plugin::common::BlockPos,
        is_front_text: bool,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.open_sign_editor(
            pumpkin_util::math::position::BlockPos(pumpkin_util::math::vector3::Vector3::new(
                pos.x, pos.y, pos.z,
            )),
            is_front_text,
        );
        Ok(())
    }

    fn set_velocity(
        &mut self,
        player: Resource<Player>,
        velocity: pumpkin::plugin::common::Position,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_velocity(pumpkin_util::math::vector3::Vector3::new(
            velocity.0, velocity.1, velocity.2,
        ));
        Ok(())
    }

    fn apply_knockback(
        &mut self,
        player: Resource<Player>,
        strength: f64,
        x: f64,
        z: f64,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.apply_knockback(strength, x, z);
        Ok(())
    }

    fn is_sneaking(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.living_entity.entity.is_sneaking())
    }

    fn set_sneaking(&mut self, player: Resource<Player>, sneaking: bool) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.living_entity.entity.set_sneaking(sneaking);
        Ok(())
    }

    fn is_sprinting(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.living_entity.entity.is_sprinting())
    }

    fn set_sprinting(&mut self, player: Resource<Player>, sprinting: bool) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_sprinting(sprinting);
        Ok(())
    }

    fn is_on_ground(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player
            .living_entity
            .entity
            .on_ground
            .load(Ordering::Relaxed))
    }

    fn is_sleeping(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.is_sleeping())
    }

    fn is_sleeping_ignored(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.sleeping_ignored.load(Ordering::Relaxed))
    }

    fn set_sleeping_ignored(
        &mut self,
        player: Resource<Player>,
        ignored: bool,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.sleeping_ignored.store(ignored, Ordering::Relaxed);
        Ok(())
    }

    fn set_movement_locked(
        &mut self,
        player: Resource<Player>,
        locked: bool,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_movement_locked(locked);
        Ok(())
    }

    fn is_movement_locked(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.is_movement_locked())
    }

    fn set_freeze_ticks(&mut self, player: Resource<Player>, ticks: i32) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_freeze_ticks(ticks);
        Ok(())
    }

    fn get_freeze_ticks(&mut self, player: Resource<Player>) -> wasmtime::Result<i32> {
        let player = self.get(&player)?;
        Ok(player.get_freeze_ticks())
    }

    fn set_server_links(
        &mut self,
        player: Resource<Player>,
        links: Vec<pumpkin::plugin::player::ServerLink>,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?.clone();
        let mut converted = Vec::new();
        for link in links {
            converted.push(from_wit_server_link(self, link)?);
        }
        let protocol_links: Vec<pumpkin_protocol::Link<'_>> = converted
            .iter()
            .map(|(label, url)| pumpkin_protocol::Link::new(label.clone(), url))
            .collect();
        player.set_server_links(&protocol_links);
        Ok(())
    }

    fn remove_effect(
        &mut self,
        player: Resource<Player>,
        effect: pumpkin::plugin::status_effect::StatusEffectType,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let effect_type = super::status_effect::from_wasm_status_effect_type(effect);
        if let Some(status_effect) =
            pumpkin_data::effect::StatusEffect::from_name(effect_type.to_name())
        {
            player.remove_effect(status_effect);
        }
        Ok(())
    }

    fn clear_effects(&mut self, player: Resource<Player>) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.remove_all_effects();
        Ok(())
    }

    fn has_effect(
        &mut self,
        player: Resource<Player>,
        effect: pumpkin::plugin::status_effect::StatusEffectType,
    ) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        let effect_type = super::status_effect::from_wasm_status_effect_type(effect);
        Ok(
            pumpkin_data::effect::StatusEffect::from_name(effect_type.to_name())
                .is_some_and(|status_effect| player.has_effect(status_effect)),
        )
    }

    fn get_effect(
        &mut self,
        player: Resource<Player>,
        effect: pumpkin::plugin::status_effect::StatusEffectType,
    ) -> wasmtime::Result<Option<pumpkin::plugin::status_effect::StatusEffectInstance>> {
        let player = self.get(&player)?;
        let effect_type = super::status_effect::from_wasm_status_effect_type(effect);
        if let Some(status_effect) =
            pumpkin_data::effect::StatusEffect::from_name(effect_type.to_name())
            && let Some(eff) = player.get_effect(status_effect)
        {
            return Ok(super::status_effect::to_wasm_status_effect_instance(&eff));
        }
        Ok(None)
    }

    fn get_active_effects(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Vec<pumpkin::plugin::status_effect::StatusEffectInstance>> {
        let player = self.get(&player)?;
        let effects = player.get_active_effects();
        let mut list = Vec::with_capacity(effects.len());
        for eff in &effects {
            if let Some(instance) = super::status_effect::to_wasm_status_effect_instance(eff) {
                list.push(instance);
            }
        }
        Ok(list)
    }

    fn get_post_effects(&mut self, player: Resource<Player>) -> wasmtime::Result<Vec<String>> {
        let player = self.get(&player)?;
        Ok(player.get_post_effects())
    }

    fn has_post_effect(
        &mut self,
        player: Resource<Player>,
        effect: String,
    ) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.has_post_effect(&effect))
    }

    fn get_statistic(
        &mut self,
        player: Resource<Player>,
        category: WitStatisticCategory,
        stat_id: i32,
    ) -> wasmtime::Result<i32> {
        let player = self.get(&player)?;
        Ok(player.get_stat(from_wit_statistic_category(category), stat_id))
    }

    fn set_statistic(
        &mut self,
        player: Resource<Player>,
        category: WitStatisticCategory,
        stat_id: i32,
        value: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_stat(from_wit_statistic_category(category), stat_id, value);
        Ok(())
    }

    fn increment_statistic(
        &mut self,
        player: Resource<Player>,
        category: WitStatisticCategory,
        stat_id: i32,
        amount: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.increment_stat(from_wit_statistic_category(category), stat_id, amount);
        Ok(())
    }

    fn get_custom_statistic(
        &mut self,
        player: Resource<Player>,
        stat: WitCustomStatistic,
    ) -> wasmtime::Result<i32> {
        let player = self.get(&player)?;
        Ok(player.get_custom_stat(from_wit_custom_statistic(stat)))
    }

    fn set_custom_statistic(
        &mut self,
        player: Resource<Player>,
        stat: WitCustomStatistic,
        value: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_custom_stat(from_wit_custom_statistic(stat), value);
        Ok(())
    }

    fn increment_custom_statistic(
        &mut self,
        player: Resource<Player>,
        stat: WitCustomStatistic,
        amount: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.increment_custom_stat(from_wit_custom_statistic(stat), amount);
        Ok(())
    }

    fn send_stats(&mut self, player: Resource<Player>) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.send_stats();
        Ok(())
    }

    fn get_team(&mut self, player: Resource<Player>) -> wasmtime::Result<Option<String>> {
        let player = self.get(&player)?;
        let team = player.get_team();
        Ok(team.map(|t| t.name))
    }

    fn start_cooldown(
        &mut self,
        player: Resource<Player>,
        group: String,
        duration_ticks: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.start_cooldown(group, duration_ticks);
        Ok(())
    }

    fn get_cooldown(&mut self, player: Resource<Player>, group: String) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.get_cooldown(&group))
    }

    fn is_on_cooldown(
        &mut self,
        player: Resource<Player>,
        group: String,
    ) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.is_on_cooldown(&group))
    }

    fn set_allow_flight(
        &mut self,
        player: Resource<Player>,
        allowed: bool,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_allow_flight(allowed);
        Ok(())
    }

    fn set_fly_speed(&mut self, player: Resource<Player>, speed: f32) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_fly_speed(speed);
        Ok(())
    }

    fn set_walk_speed(&mut self, player: Resource<Player>, speed: f32) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_walk_speed(speed);
        Ok(())
    }

    fn set_invulnerable(
        &mut self,
        player: Resource<Player>,
        invulnerable: bool,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_invulnerable(invulnerable);
        Ok(())
    }

    fn set_player_time(
        &mut self,
        player: Resource<Player>,
        time: u64,
        relative: bool,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_player_time(time, relative);
        Ok(())
    }

    fn reset_player_time(&mut self, player: Resource<Player>) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.reset_player_time();
        Ok(())
    }

    fn get_player_time(&mut self, player: Resource<Player>) -> wasmtime::Result<Option<u64>> {
        let player = self.get(&player)?;
        Ok(player.get_player_time())
    }

    fn is_player_time_relative(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.is_player_time_relative())
    }

    fn set_player_weather(
        &mut self,
        player: Resource<Player>,
        weather: crate::pumpkin::plugin::player::PlayerWeather,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let w = match weather {
            crate::pumpkin::plugin::player::PlayerWeather::Clear => {
                pumpkin_core::entity::player::PlayerWeather::Clear
            }
            crate::pumpkin::plugin::player::PlayerWeather::Downfall => {
                pumpkin_core::entity::player::PlayerWeather::Downfall
            }
        };
        player.set_player_weather(w);
        Ok(())
    }

    fn reset_player_weather(&mut self, player: Resource<Player>) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.reset_player_weather();
        Ok(())
    }

    fn get_player_weather(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Option<crate::pumpkin::plugin::player::PlayerWeather>> {
        let player = self.get(&player)?;
        Ok(player.get_player_weather().map(|w| match w {
            pumpkin_core::entity::player::PlayerWeather::Clear => {
                crate::pumpkin::plugin::player::PlayerWeather::Clear
            }
            pumpkin_core::entity::player::PlayerWeather::Downfall => {
                crate::pumpkin::plugin::player::PlayerWeather::Downfall
            }
        }))
    }

    fn set_compass_target(
        &mut self,
        player: Resource<Player>,
        pos: crate::pumpkin::plugin::common::Position,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let pos_vec = from_wasm_position(pos);
        let block_pos = pumpkin_util::math::position::BlockPos::new(
            pos_vec.x as i32,
            pos_vec.y as i32,
            pos_vec.z as i32,
        );
        player.set_compass_target(block_pos);
        Ok(())
    }

    fn get_compass_target(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<crate::pumpkin::plugin::common::Position> {
        let player = self.get(&player)?;
        let target = player
            .get_compass_target()
            .unwrap_or(pumpkin_util::math::position::BlockPos::new(0, 0, 0));
        let vec3 = pumpkin_util::math::vector3::Vector3::new(
            f64::from(target.0.x),
            f64::from(target.0.y),
            f64::from(target.0.z),
        );
        Ok(to_wasm_position(vec3))
    }

    fn set_respawn_location(
        &mut self,
        player: Resource<Player>,
        pos: crate::pumpkin::plugin::common::Position,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let pos_vec = from_wasm_position(pos);
        let block_pos = pumpkin_util::math::position::BlockPos::new(
            pos_vec.x as i32,
            pos_vec.y as i32,
            pos_vec.z as i32,
        );
        player.set_respawn_location(block_pos);
        Ok(())
    }

    fn get_respawn_location(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Option<crate::pumpkin::plugin::common::Position>> {
        let player = self.get(&player)?;
        Ok(player.get_respawn_location().map(|p| {
            let vec3 = pumpkin_util::math::vector3::Vector3::new(
                f64::from(p.0.x),
                f64::from(p.0.y),
                f64::from(p.0.z),
            );
            to_wasm_position(vec3)
        }))
    }

    fn hide_player(
        &mut self,
        player: Resource<Player>,
        other: Resource<Player>,
    ) -> wasmtime::Result<()> {
        let other = self.take(other)?;
        let player = self.get(&player)?;
        player.hide_player(other.gameprofile.id);
        Ok(())
    }

    fn show_player(
        &mut self,
        player: Resource<Player>,
        other: Resource<Player>,
    ) -> wasmtime::Result<()> {
        let other = self.take(other)?;
        let player = self.get(&player)?;
        player.show_player(other.gameprofile.id);
        Ok(())
    }

    fn can_see(
        &mut self,
        player: Resource<Player>,
        other: Resource<Player>,
    ) -> wasmtime::Result<bool> {
        let other = self.take(other)?;
        let player = self.get(&player)?;
        Ok(player.can_see(&other.gameprofile.id))
    }

    fn can_see_player(
        &mut self,
        player: Resource<Player>,
        other: Resource<Player>,
    ) -> wasmtime::Result<bool> {
        let other = self.take(other)?;
        let player = self.get(&player)?;
        Ok(player.can_see(&other.gameprofile.id))
    }

    fn set_tab_list_ping(
        &mut self,
        player: Resource<Player>,
        latency_ms: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_tab_list_ping(latency_ms);
        Ok(())
    }

    fn set_item_cooldown(
        &mut self,
        player: Resource<Player>,
        item_id: String,
        ticks: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_item_cooldown(&item_id, ticks);
        Ok(())
    }

    fn get_item_cooldown(
        &mut self,
        player: Resource<Player>,
        item_id: String,
    ) -> wasmtime::Result<Option<i32>> {
        let player = self.get(&player)?;
        Ok(player.get_item_cooldown(&item_id))
    }

    fn has_item_cooldown(
        &mut self,
        player: Resource<Player>,
        item_id: String,
    ) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.has_item_cooldown(&item_id))
    }

    fn ray_trace_block(
        &mut self,
        player: Resource<Player>,
        max_distance: f64,
        include_fluids: bool,
    ) -> wasmtime::Result<Option<pumpkin::plugin::world::RayTraceBlockResult>> {
        let player = self.get(&player)?;
        let start = player.living_entity.entity.get_eye_pos();
        let direction = player.living_entity.entity.get_looking_vector();
        let end = start + direction * max_distance;
        let world = player.living_entity.entity.world.load_full();

        let hit = world.ray_trace_block(start, end, include_fluids);

        Ok(hit.map(
            |(pos, face, hit_pos)| pumpkin::plugin::world::RayTraceBlockResult {
                pos: pumpkin::plugin::world::BlockPos {
                    x: pos.0.x,
                    y: pos.0.y,
                    z: pos.0.z,
                },
                face: crate::world::to_wasm_block_direction(face),
                hit_pos: to_wasm_position(hit_pos),
            },
        ))
    }

    fn ray_trace_entity(
        &mut self,
        player: Resource<Player>,
        max_distance: f64,
    ) -> wasmtime::Result<Option<pumpkin::plugin::world::RayTraceEntityResult>> {
        let player = self.get(&player)?;
        let start = player.living_entity.entity.get_eye_pos();
        let direction = player.living_entity.entity.get_looking_vector();
        let end = start + direction * max_distance;
        let world = player.living_entity.entity.world.load_full();
        let self_id = player.living_entity.entity.entity_id;

        let hits = world.ray_trace_entities(start, end);
        for (hit_entity, hit_pos, distance) in hits {
            if hit_entity.get_entity().entity_id != self_id {
                let entity_res = self
                    .add(hit_entity)
                    .map_err(|_| wasmtime::Error::msg("failed to add entity resource"))?;
                return Ok(Some(pumpkin::plugin::world::RayTraceEntityResult {
                    entity: entity_res,
                    hit_pos: to_wasm_position(hit_pos),
                    distance,
                }));
            }
        }

        Ok(None)
    }

    fn get_target_entity(
        &mut self,
        player: Resource<Player>,
        max_distance: f64,
    ) -> wasmtime::Result<Option<Resource<pumpkin::plugin::world::Entity>>> {
        let res = self.ray_trace_entity(player, max_distance)?;
        Ok(res.map(|r| r.entity))
    }

    fn get_target_block(
        &mut self,
        player: Resource<Player>,
        max_distance: u32,
    ) -> wasmtime::Result<Option<crate::pumpkin::plugin::common::Position>> {
        let player = self.get(&player)?;
        let res = player.get_target_block(
            &player.living_entity.entity.world.load_full(),
            f64::from(max_distance),
        );
        Ok(res.map(|p| {
            let vec3 = pumpkin_util::math::vector3::Vector3::new(
                f64::from(p.0.x),
                f64::from(p.0.y),
                f64::from(p.0.z),
            );
            to_wasm_position(vec3)
        }))
    }

    fn get_target_block_exact(
        &mut self,
        player: Resource<Player>,
        max_distance: f64,
        include_fluids: bool,
    ) -> wasmtime::Result<Option<pumpkin::plugin::world::RayTraceBlockResult>> {
        self.ray_trace_block(player, max_distance, include_fluids)
    }

    fn launch_projectile(
        &mut self,
        _player: Resource<Player>,
        _type_: crate::pumpkin::plugin::player::ProjectileType,
    ) -> wasmtime::Result<Option<Resource<crate::pumpkin::plugin::world::Entity>>> {
        Ok(None)
    }

    fn set_tab_list_header_footer(
        &mut self,
        player: Resource<Player>,
        header: Resource<pumpkin::plugin::text::TextComponent>,
        footer: Resource<pumpkin::plugin::text::TextComponent>,
    ) -> wasmtime::Result<()> {
        let header = self.take(header)?;
        let footer = self.take(footer)?;
        let player = self.get(&player)?;
        player.set_tab_list_header_footer(&header, &footer);
        Ok(())
    }

    fn set_tab_list_order(&mut self, player: Resource<Player>, order: i32) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_tab_list_order(order);
        Ok(())
    }

    fn set_tab_list_latency(
        &mut self,
        player: Resource<Player>,
        latency: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_tab_list_latency(latency);
        Ok(())
    }

    fn set_tab_list_listed(
        &mut self,
        player: Resource<Player>,
        listed: bool,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_tab_list_listed(listed);
        Ok(())
    }

    fn show_title(
        &mut self,
        player: Resource<Player>,
        text: Resource<pumpkin::plugin::text::TextComponent>,
    ) -> wasmtime::Result<()> {
        let component = self.take(text)?;
        let player = self.get(&player)?;
        player.show_title(&component, &TitleMode::Title);
        Ok(())
    }

    fn show_subtitle(
        &mut self,
        player: Resource<Player>,
        text: Resource<pumpkin::plugin::text::TextComponent>,
    ) -> wasmtime::Result<()> {
        let component = self.take(text)?;
        let player = self.get(&player)?;
        player.show_title(&component, &TitleMode::SubTitle);
        Ok(())
    }

    fn show_actionbar(
        &mut self,
        player: Resource<Player>,
        text: Resource<pumpkin::plugin::text::TextComponent>,
    ) -> wasmtime::Result<()> {
        let component = self.take(text)?;
        let player = self.get(&player)?;
        player.show_title(&component, &TitleMode::ActionBar);
        Ok(())
    }

    fn send_title_animation(
        &mut self,
        player: Resource<Player>,
        fade_in: i32,
        stay: i32,
        fade_out: i32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.send_title_animation(fade_in, stay, fade_out);
        Ok(())
    }

    fn get_selected_slot(&mut self, player: Resource<Player>) -> wasmtime::Result<u8> {
        let player = self.get(&player)?;
        Ok(player.inventory.get_selected_slot())
    }

    fn has_open_inventory(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        let current = player.current_screen_handler.lock_ignore_poison().clone();
        let player_screen_handler: Arc<
            std::sync::Mutex<dyn pumpkin_inventory::screen_handler::ScreenHandler>,
        > = player.player_screen_handler.clone();
        Ok(!Arc::ptr_eq(&current, &player_screen_handler))
    }

    fn get_health(&mut self, player: Resource<Player>) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.living_entity.health.load())
    }

    fn set_health(&mut self, player: Resource<Player>, health: f32) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_health(health);
        Ok(())
    }

    fn get_max_health(&mut self, player: Resource<Player>) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.living_entity.get_max_health())
    }

    fn set_max_health(
        &mut self,
        player: Resource<Player>,
        max_health: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_max_health(max_health);
        Ok(())
    }

    fn get_food_level(&mut self, player: Resource<Player>) -> wasmtime::Result<u8> {
        let player = self.get(&player)?;
        Ok(player.hunger_manager.level.load())
    }

    fn get_saturation(&mut self, player: Resource<Player>) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.hunger_manager.saturation.load())
    }

    fn set_saturation(
        &mut self,
        player: Resource<Player>,
        saturation: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_saturation(saturation);
        Ok(())
    }

    fn get_exhaustion(&mut self, player: Resource<Player>) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.get_exhaustion())
    }

    fn set_exhaustion(
        &mut self,
        player: Resource<Player>,
        exhaustion: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_exhaustion(exhaustion);
        Ok(())
    }

    fn get_absorption(&mut self, player: Resource<Player>) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.get_absorption())
    }

    fn set_absorption(
        &mut self,
        player: Resource<Player>,
        absorption: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        player.set_absorption(absorption);
        Ok(())
    }

    fn get_experience_level(&mut self, player: Resource<Player>) -> wasmtime::Result<i32> {
        let player = self.get(&player)?;
        Ok(player.experience_level.load(Ordering::Relaxed))
    }

    fn get_experience_progress(&mut self, player: Resource<Player>) -> wasmtime::Result<f32> {
        let player = self.get(&player)?;
        Ok(player.experience_progress.load())
    }

    fn get_experience_points(&mut self, player: Resource<Player>) -> wasmtime::Result<i32> {
        let player = self.get(&player)?;
        Ok(player.experience_points.load(Ordering::Relaxed))
    }

    fn is_flying(&mut self, player: Resource<Player>) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        Ok(player.is_flying())
    }

    fn set_flying(&mut self, player: Resource<Player>, flying: bool) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        {
            let mut abilities = player.abilities.lock_ignore_poison();
            abilities.flying = flying;
        };
        player.send_abilities_update();
        Ok(())
    }

    fn get_abilities(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<pumpkin::plugin::player::PlayerAbilities> {
        let player = self.get(&player)?;
        let abilities = player.abilities.lock_ignore_poison();
        Ok(pumpkin::plugin::player::PlayerAbilities {
            invulnerable: abilities.invulnerable,
            flying: abilities.flying,
            allow_flying: abilities.allow_flying,
            creative: abilities.creative,
            allow_modify_world: abilities.allow_modify_world,
            fly_speed: abilities.fly_speed,
            walk_speed: abilities.walk_speed,
        })
    }

    fn set_abilities(
        &mut self,
        player: Resource<Player>,
        abilities: pumpkin::plugin::player::PlayerAbilities,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        {
            let mut a = player.abilities.lock_ignore_poison();
            a.invulnerable = abilities.invulnerable;
            a.flying = abilities.flying;
            a.allow_flying = abilities.allow_flying;
            a.creative = abilities.creative;
            a.allow_modify_world = abilities.allow_modify_world;
            a.fly_speed = abilities.fly_speed;
            a.walk_speed = abilities.walk_speed;
        };
        player.send_abilities_update();
        Ok(())
    }

    fn get_ip(&mut self, player: Resource<Player>) -> wasmtime::Result<String> {
        let player = self.get(&player)?;
        Ok(player.get_ip())
    }

    fn get_skin(&mut self, player: Resource<Player>) -> wasmtime::Result<Option<PlayerSkin>> {
        let player = self.get(&player)?;
        Ok(player
            .gameprofile
            .properties
            .load()
            .iter()
            .find(|p| p.name.as_ref() == "textures")
            .map(|p| PlayerSkin {
                value: p.value.to_string(),
                signature: p.signature.as_ref().map(std::string::ToString::to_string),
            }))
    }

    fn set_skin(&mut self, player: Resource<Player>, skin: PlayerSkin) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let mut properties = (**player.gameprofile.properties.load()).clone();

        properties.retain(|p| p.name.as_ref() != "textures");
        properties.push(Property {
            name: "textures".into(),
            value: skin.value.into(),
            signature: skin.signature.map(std::convert::Into::into),
        });

        player.gameprofile.properties.store(Arc::new(properties));

        Ok(())
    }

    fn get_skin_parts(&mut self, player: Resource<Player>) -> wasmtime::Result<SkinParts> {
        let player = self.get(&player)?;
        let mask = player.config.load().skin_parts;
        let mut parts = SkinParts::empty();
        if mask & 0x01 != 0 {
            parts |= SkinParts::CAPE;
        }
        if mask & 0x02 != 0 {
            parts |= SkinParts::JACKET;
        }
        if mask & 0x04 != 0 {
            parts |= SkinParts::LEFT_SLEEVE;
        }
        if mask & 0x08 != 0 {
            parts |= SkinParts::RIGHT_SLEEVE;
        }
        if mask & 0x10 != 0 {
            parts |= SkinParts::LEFT_PANTS_LEG;
        }
        if mask & 0x20 != 0 {
            parts |= SkinParts::RIGHT_PANTS_LEG;
        }
        if mask & 0x40 != 0 {
            parts |= SkinParts::HAT;
        }
        Ok(parts)
    }

    fn set_skin_parts(
        &mut self,
        player: Resource<Player>,
        parts: SkinParts,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let mut mask = 0u8;
        if parts.contains(SkinParts::CAPE) {
            mask |= 0x01;
        }
        if parts.contains(SkinParts::JACKET) {
            mask |= 0x02;
        }
        if parts.contains(SkinParts::LEFT_SLEEVE) {
            mask |= 0x04;
        }
        if parts.contains(SkinParts::RIGHT_SLEEVE) {
            mask |= 0x08;
        }
        if parts.contains(SkinParts::LEFT_PANTS_LEG) {
            mask |= 0x10;
        }
        if parts.contains(SkinParts::RIGHT_PANTS_LEG) {
            mask |= 0x20;
        }
        if parts.contains(SkinParts::HAT) {
            mask |= 0x40;
        }

        {
            let mut config = (**player.config.load()).clone();
            config.skin_parts = mask;
            player.config.store(Arc::new(config));
        };
        player.send_client_information();
        Ok(())
    }

    fn get_advancement_progress(
        &mut self,
        player: Resource<Player>,
        advancement_id: String,
    ) -> wasmtime::Result<Option<pumpkin::plugin::advancement::AdvancementProgress>> {
        let player = self.get(&player)?;
        let Some(advancement) = crate::advancement::find_advancement(&advancement_id) else {
            return Ok(None);
        };
        let guard = player.advancements.lock_ignore_poison();
        let progress = guard.progress.map.get(advancement).map_or_else(
            || pumpkin::plugin::advancement::AdvancementProgress {
                advancement_id: advancement.id.to_string(),
                done: false,
                awarded_criteria: Vec::new(),
                remaining_criteria: advancement
                    .criteria
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            },
            |progress| pumpkin::plugin::advancement::AdvancementProgress {
                advancement_id: advancement.id.to_string(),
                done: progress.is_done(),
                awarded_criteria: progress
                    .get_completed_criteria()
                    .map(|s| s.to_string())
                    .collect(),
                remaining_criteria: progress
                    .get_remaining_criteria()
                    .map(|s| s.to_string())
                    .collect(),
            },
        );
        Ok(Some(progress))
    }

    fn revoke_advancement_criterion(
        &mut self,
        player: Resource<Player>,
        advancement_id: String,
        criterion: String,
    ) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        let Some(advancement) = crate::advancement::find_advancement(&advancement_id) else {
            return Ok(false);
        };
        let mut guard = player.advancements.lock_ignore_poison();
        let revoked = guard.revoke(advancement, &criterion);
        if revoked {
            guard.flush_dirty(player, true);
        }
        Ok(revoked)
    }

    fn revoke_advancement(
        &mut self,
        player: Resource<Player>,
        advancement_id: String,
    ) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        let Some(advancement) = crate::advancement::find_advancement(&advancement_id) else {
            return Ok(false);
        };
        let mut guard = player.advancements.lock_ignore_poison();
        let progress = guard.progress.get_mut_or_start_progress(advancement);
        if !progress.has_progress() {
            return Ok(false);
        }
        let completed: Vec<Arc<str>> = progress.get_completed_criteria().collect();
        let mut any_revoked = false;
        for criterion in completed {
            if guard.revoke(advancement, &criterion) {
                any_revoked = true;
            }
        }
        if any_revoked {
            guard.flush_dirty(player, true);
        }
        Ok(any_revoked)
    }

    fn has_advancement(
        &mut self,
        player: Resource<Player>,
        advancement_id: String,
    ) -> wasmtime::Result<bool> {
        let player = self.get(&player)?;
        let Some(advancement) = crate::advancement::find_advancement(&advancement_id) else {
            return Ok(false);
        };
        let guard = player.advancements.lock_ignore_poison();
        let done =
            guard.progress.map.get(advancement).is_some_and(
                pumpkin_core::entity::player::advancement::AdvancementProgress::is_done,
            );
        Ok(done)
    }

    fn get_completed_advancements(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Vec<String>> {
        let player = self.get(&player)?;
        let guard = player.advancements.lock_ignore_poison();
        let list = guard
            .progress
            .map
            .iter()
            .filter(|(_, p)| p.is_done())
            .map(|(adv, _)| adv.id.to_string())
            .collect();
        Ok(list)
    }

    fn get_selected_advancement_tab(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Option<String>> {
        let player = self.get(&player)?;
        let guard = player.advancements.lock_ignore_poison();
        Ok(guard.last_selected_tab.map(|adv| adv.id.to_string()))
    }

    fn set_selected_advancement_tab(
        &mut self,
        player: Resource<Player>,
        tab_id: Option<String>,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?;
        let target_adv = tab_id
            .as_deref()
            .and_then(crate::advancement::find_advancement);
        let mut guard = player.advancements.lock_ignore_poison();
        guard.set_selected_tab(target_adv);
        Ok(())
    }

    fn as_java(
        &mut self,
        player: Resource<Player>,
    ) -> wasmtime::Result<Option<Resource<pumpkin::plugin::player::JavaPlayer>>> {
        let player = self.get(&player)?.clone();
        {
            let pumpkin_core::net::ClientPlatform::Java(_) = player.client.as_ref();
            Ok(Some(self.add(player)?))
        }
    }

    fn drop(&mut self, rep: Resource<Player>) -> wasmtime::Result<()> {
        self.drop(rep)
    }
}

impl pumpkin::plugin::player::HostPlayerWithStore<PluginHostState> for HasSelf<PluginHostState> {
    async fn perform_command(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        command: String,
    ) -> wasmtime::Result<bool> {
        let (player, server) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            Ok((
                state.get(&player)?.clone(),
                state.require_server().cloned()?,
            ))
        })?;

        let command_str = command.strip_prefix('/').unwrap_or(&command);
        let dispatcher = server.command_dispatcher.load();
        let source = player.get_command_source(&server);
        dispatcher.handle_command(&source, command_str);
        Ok(true)
    }

    async fn chat(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        message: String,
    ) -> wasmtime::Result<()> {
        let (player, server) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            Ok((
                state.get(&player)?.clone(),
                state.require_server().cloned()?,
            ))
        })?;

        if let Some(command) = message.strip_prefix('/') {
            let dispatcher = server.command_dispatcher.load();
            let source = player.get_command_source(&server);
            dispatcher.handle_command(&source, command);
        } else {
            let config = &server.advanced_config;
            let decorated = pumpkin_util::text::TextComponent::chat_decorated(
                &config.chat.format,
                &player.gameprofile.name,
                &message,
            );
            server.broadcast_message(
                &decorated,
                &pumpkin_util::text::TextComponent::text(player.gameprofile.name.clone()),
                0,
                None,
            );
        }
        Ok(())
    }

    async fn send_equipment_change(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        entity_id: u32,
        slot: pumpkin::plugin::world::EquipmentSlot,
        item: Option<Resource<WitHostItemStack>>,
    ) -> wasmtime::Result<()> {
        let stack = if let Some(stack_res) = item {
            accessor.take_res(stack_res)?.lock().await.clone()
        } else {
            pumpkin_data::item_stack::ItemStack::EMPTY.clone()
        };
        let player = accessor.get_res(&player)?;
        let slot = crate::living_entity::from_wit_equipment_slot(slot);
        let equipment = vec![(
            slot.discriminant(),
            pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer::from(stack),
        )];
        let packet = pumpkin_protocol::java::client::play::CSetEquipment::new(
            pumpkin_protocol::codec::var_int::VarInt(entity_id as i32),
            equipment,
        );
        player.try_send_client_packet(&packet);
        Ok(())
    }
    async fn set_item_in_hand(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        hand: pumpkin::plugin::common::Hand,
        stack: Option<Resource<WitHostItemStack>>,
    ) -> wasmtime::Result<()> {
        let stack = if let Some(stack_res) = stack {
            accessor.take_res(stack_res)?.lock().await.clone()
        } else {
            pumpkin_data::item_stack::ItemStack::EMPTY.clone()
        };
        let player = accessor.get_res(&player)?;

        let hand = from_wasm_hand(hand);
        let slot = match hand {
            pumpkin_util::Hand::Right => player.inventory().get_selected_slot() as usize,
            pumpkin_util::Hand::Left => PlayerInventory::OFF_HAND_SLOT,
        };

        player.inventory().set_stack(slot, stack.clone());
        player.sync_hand_slot(slot, stack);

        Ok(())
    }

    async fn set_inventory_item(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        slot: u8,
        stack: Option<Resource<WitHostItemStack>>,
    ) -> wasmtime::Result<()> {
        let stack = if let Some(stack_res) = stack {
            accessor.take_res(stack_res)?.lock().await.clone()
        } else {
            pumpkin_data::item_stack::ItemStack::EMPTY.clone()
        };
        let player = accessor.get_res(&player)?;

        player.inventory().set_stack(slot as usize, stack.clone());

        player.sync_hand_slot(slot as usize, stack);

        Ok(())
    }

    async fn set_ender_chest_item(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        slot: u8,
        stack: Option<Resource<WitHostItemStack>>,
    ) -> wasmtime::Result<()> {
        let stack = if let Some(stack_res) = stack {
            accessor.take_res(stack_res)?.lock().await.clone()
        } else {
            pumpkin_data::item_stack::ItemStack::EMPTY.clone()
        };
        let player = accessor.get_res(&player)?;

        let ec = player.ender_chest_inventory();
        ec.set_stack(slot as usize, stack.clone());

        // If the player currently has their ender chest screen open, sync slot
        let sync_id = {
            let screen_handler_arc = player.current_screen_handler.lock_ignore_poison().clone();
            let handler = screen_handler_arc.lock_ignore_poison();
            if let Some(generic) = handler
                .as_any()
                .downcast_ref::<GenericContainerScreenHandler>()
                && generic.inventory.as_any().is::<EnderChestInventory>()
            {
                Some(handler.sync_id())
            } else {
                None
            }
        };
        if let Some(sync_id) = sync_id {
            let stack_serializer = ItemStackSerializer::from(stack);
            let packet = CSetContainerSlot::new(sync_id as i8, 0, slot as i16, &stack_serializer);
            player.send_client_packet(&packet).await;
        }

        Ok(())
    }

    async fn clear_ender_chest(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        let ec = player.ender_chest_inventory();
        ec.clear();

        // If the player currently has their ender chest screen open, sync all slots
        let sync_id = {
            let screen_handler_arc = player.current_screen_handler.lock_ignore_poison().clone();
            let handler = screen_handler_arc.lock_ignore_poison();
            if let Some(generic) = handler
                .as_any()
                .downcast_ref::<GenericContainerScreenHandler>()
                && generic.inventory.as_any().is::<EnderChestInventory>()
            {
                Some(handler.sync_id())
            } else {
                None
            }
        };
        if let Some(sync_id) = sync_id {
            let empty_serializer =
                ItemStackSerializer::from(pumpkin_data::item_stack::ItemStack::EMPTY.clone());
            for slot in 0..27 {
                let packet =
                    CSetContainerSlot::new(sync_id as i8, 0, slot as i16, &empty_serializer);
                player.send_client_packet(&packet).await;
            }
        }

        Ok(())
    }

    async fn delete_message_by_signature(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        signature: Vec<u8>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        if let Some(client) = player.client.java() {
            let packet =
                pumpkin_protocol::java::client::play::CDeleteChat::from_signature(&signature);
            client.send_packet(&packet).await;
        }
        Ok(())
    }

    async fn delete_message_by_id(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        signature_id: i32,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        if let Some(client) = player.client.java() {
            let packet =
                pumpkin_protocol::java::client::play::CDeleteChat::from_cache_id(signature_id);
            client.send_packet(&packet).await;
        }
        Ok(())
    }

    async fn transfer(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        host: String,
        port: u16,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        {
            let pumpkin_core::net::ClientPlatform::Java(client) = player.client.as_ref();
            client
                .send_packet(&pumpkin_protocol::java::client::play::CTransfer::new(
                    &host,
                    pumpkin_protocol::codec::var_int::VarInt(i32::from(port)),
                ))
                .await;
        };
        Ok(())
    }

    async fn open_ender_chest(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || {
            player.open_ender_chest();
        })
        .await
    }

    async fn set_gamemode(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        mode: pumpkin::plugin::common::GameMode,
    ) -> wasmtime::Result<bool> {
        let player = accessor.get_res(&player)?;
        let mode = from_wasm_game_mode(mode);

        run_blocking(accessor, move || player.set_gamemode(mode)).await
    }

    async fn set_permission_level(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        level: pumpkin::plugin::permission::PermissionLevel,
    ) -> wasmtime::Result<()> {
        let (player, server) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            Ok((
                state.get(&player)?.clone(),
                state.require_server().cloned()?,
            ))
        })?;
        let level = from_wit_permission_level(level);

        run_blocking(accessor, move || {
            let command_dispatcher = server.command_dispatcher.load();
            player.set_permission_lvl(&server, level, &command_dispatcher);
        })
        .await
    }

    async fn has_permission(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        node: String,
    ) -> wasmtime::Result<bool> {
        let (player, server) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            Ok((
                state.get(&player)?.clone(),
                state.require_server().cloned()?,
            ))
        })?;

        run_blocking(accessor, move || player.has_permission(&server, &node)).await
    }

    async fn add_effect(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        effect: pumpkin::plugin::status_effect::StatusEffectInstance,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        let effect_type = super::status_effect::from_wasm_status_effect_type(effect.effect_type);
        let Some(status_effect) =
            pumpkin_data::effect::StatusEffect::from_name(effect_type.to_name())
        else {
            return Ok(());
        };
        let effect = pumpkin_data::potion::Effect {
            effect_type: status_effect,
            duration: effect.duration as i32,
            amplifier: effect.amplifier,
            ambient: effect.ambient,
            show_particles: effect.show_particles,
            show_icon: effect.show_icon,
            blend: false,
        };

        run_blocking(accessor, move || player.add_effect(effect)).await
    }

    async fn add_post_effect(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        effect: String,
    ) -> wasmtime::Result<bool> {
        let player = accessor.get_res(&player)?;
        Ok(player.add_post_effect(effect))
    }

    async fn remove_post_effect(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        effect: String,
    ) -> wasmtime::Result<bool> {
        let player = accessor.get_res(&player)?;
        Ok(player.remove_post_effect(&effect))
    }

    async fn clear_post_effects(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
    ) -> wasmtime::Result<u32> {
        let player = accessor.get_res(&player)?;
        Ok(player.clear_post_effects() as u32)
    }

    async fn send_post_effects(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        player.send_post_effects();
        Ok(())
    }

    async fn heal(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        amount: f32,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || player.heal(amount)).await
    }

    async fn damage(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        amount: f32,
        damage_type: WitDamageType,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        let damage_type = from_wit_damage_type(damage_type);

        run_blocking(accessor, move || {
            player.damage(&*player, amount, damage_type);
        })
        .await
    }

    async fn kill(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || player.kill()).await
    }

    async fn teleport(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        position: pumpkin::plugin::common::Position,
        yaw: Option<f32>,
        pitch: Option<f32>,
        world: Resource<World>,
    ) -> wasmtime::Result<()> {
        let (player, world) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            Ok((state.get(&player)?.clone(), state.take(world)?))
        })?;
        let position = from_wasm_position(position);

        run_blocking(accessor, move || {
            player.teleport(position, yaw, pitch, world);
        })
        .await
    }

    async fn teleport_world(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        world: Resource<World>,
        position: pumpkin::plugin::common::Position,
        yaw: Option<f32>,
        pitch: Option<f32>,
    ) -> wasmtime::Result<()> {
        let (player, world) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            Ok((state.get(&player)?.clone(), state.take(world)?))
        })?;
        let position = from_wasm_position(position);
        let runtime = tokio::runtime::Handle::current();

        run_blocking(accessor, move || {
            runtime.block_on(player.teleport_world(world, position, yaw, pitch));
        })
        .await
    }

    async fn respawn(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        let runtime = tokio::runtime::Handle::current();

        run_blocking(accessor, move || runtime.block_on(player.respawn())).await
    }

    async fn open_gui(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        gui: Resource<pumpkin::plugin::gui::Gui>,
    ) -> wasmtime::Result<()> {
        let (player, gui) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            let gui = state.take(gui)?;
            Ok((state.get(&player)?.clone(), gui))
        })?;

        let (window_type, inventory, allow_grab_items, allow_put_items, title) = (async move {
            let gui = gui.lock().await;
            (
                gui.window_type,
                gui.inventory.clone(),
                gui.allow_grab_items,
                gui.allow_put_items,
                gui.title.clone(),
            )
        })
        .await;

        run_blocking(accessor, move || {
            player.increment_screen_handler_sync_id();
            let sync_id = player.screen_handler_sync_id.load(Ordering::Relaxed);
            let screen_handler: Arc<
                std::sync::Mutex<dyn pumpkin_inventory::screen_handler::ScreenHandler>,
            > = Arc::new(std::sync::Mutex::new(PluginScreenHandler::new(
                sync_id,
                window_type,
                player.inventory(),
                &inventory,
                allow_grab_items,
                allow_put_items,
            )));
            player.open_handled_screen_direct(screen_handler, &title);
        })
        .await
    }

    async fn close_inventory(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        run_blocking(accessor, move || {
            player.close_handled_screen();
        })
        .await
    }

    async fn drop_held_item(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        drop_all: bool,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;
        run_blocking(accessor, move || {
            player.drop_held_item(drop_all);
        })
        .await
    }

    async fn drop_item(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        item: Resource<WitHostItemStack>,
    ) -> wasmtime::Result<()> {
        let stack = accessor.take_res(item)?.lock().await.clone();
        if stack.is_empty() {
            return Ok(());
        }
        let player = accessor.get_res(&player)?;
        run_blocking(accessor, move || {
            player.drop_item(stack);
        })
        .await
    }

    async fn ban(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        options: pumpkin::plugin::player::BanPlayerOptions,
    ) -> wasmtime::Result<()> {
        let pumpkin::plugin::player::BanPlayerOptions {
            reason,
            source,
            expires_at_utc,
            duration_seconds,
            kick_if_online,
            log_to_console,
        } = options;
        let (player, server, reason) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            Ok((
                state.get(&player)?.clone(),
                state.require_server().cloned()?,
                reason.map(|t| state.take(t)).transpose()?,
            ))
        })?;
        let expires = parse_ban_expiry(expires_at_utc, duration_seconds);

        run_blocking(accessor, move || {
            player.ban_explicit(
                &server,
                reason,
                source,
                expires,
                kick_if_online,
                log_to_console,
            );
        })
        .await
    }

    async fn ban_ip(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        options: pumpkin::plugin::player::BanIpOptions,
    ) -> wasmtime::Result<()> {
        let pumpkin::plugin::player::BanIpOptions {
            reason,
            source,
            expires_at_utc,
            duration_seconds,
            kick_matching_players,
            log_to_console,
        } = options;
        let (player, server, reason) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            Ok((
                state.get(&player)?.clone(),
                state.require_server().cloned()?,
                reason.map(|t| state.take(t)).transpose()?,
            ))
        })?;
        let expires = parse_ban_expiry(expires_at_utc, duration_seconds);

        run_blocking(accessor, move || {
            player.ban_ip_explicit(
                &server,
                reason,
                source,
                expires,
                kick_matching_players,
                log_to_console,
            );
        })
        .await
    }

    async fn set_food_level(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        level: u8,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || player.set_food_level(level)).await
    }

    async fn set_experience_level(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        level: i32,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || player.set_experience_level(level, true)).await
    }

    async fn set_experience_progress(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        progress: f32,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || {
            player.set_experience(
                player.experience_level.load(Ordering::Relaxed),
                progress,
                player.experience_points.load(Ordering::Relaxed),
            );
        })
        .await
    }

    async fn set_experience_points(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        points: i32,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || {
            player.set_experience(
                player.experience_level.load(Ordering::Relaxed),
                player.experience_progress.load(),
                points,
            );
        })
        .await
    }

    async fn add_experience_levels(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        levels: i32,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || player.add_experience_levels(levels)).await
    }

    async fn add_experience_points(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        points: i32,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?;

        run_blocking(accessor, move || player.add_experience_points(points)).await
    }

    async fn award_advancement_criterion(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        advancement_id: String,
        criterion: String,
    ) -> wasmtime::Result<bool> {
        let player = accessor.get_res(&player)?;
        let Some(advancement) = crate::advancement::find_advancement(&advancement_id) else {
            return Ok(false);
        };

        run_blocking(accessor, move || {
            let result = {
                let mut guard = player.advancements.lock_ignore_poison();
                guard.award(advancement, &criterion)
            };

            PlayerAdvancement::finish_award(&player, advancement, result);
            if result.awarded() {
                let mut guard = player.advancements.lock_ignore_poison();
                guard.flush_dirty(&player, true);
            }
            result.awarded()
        })
        .await
    }

    async fn award_advancement(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<Player>,
        advancement_id: String,
    ) -> wasmtime::Result<bool> {
        let player = accessor.get_res(&player)?;
        let Some(advancement) = crate::advancement::find_advancement(&advancement_id) else {
            return Ok(false);
        };

        run_blocking(accessor, move || {
            let result = {
                let mut guard = player.advancements.lock_ignore_poison();
                let progress = guard.progress.get_mut_or_start_progress(advancement);
                if progress.is_done() {
                    return false;
                }
                let remaining: Vec<Arc<str>> = progress.get_remaining_criteria().collect();
                let mut result = AdvancementAward::default();
                for criterion in remaining {
                    result = result.combine(guard.award(advancement, &criterion));
                }
                result
            };

            PlayerAdvancement::finish_award(&player, advancement, result);
            if result.awarded() {
                let mut guard = player.advancements.lock_ignore_poison();
                guard.flush_dirty(&player, true);
            }
            result.awarded()
        })
        .await
    }
}

impl pumpkin::plugin::player::HostJavaPlayer for PluginHostState {
    fn get_version(
        &mut self,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
    ) -> wasmtime::Result<pumpkin::plugin::player::JavaMinecraftVersion> {
        let player = self.get(&player)?.clone();

        let client = player
            .client
            .java()
            .ok_or_else(|| wasmtime::Error::msg("Not a java player"))?;
        Ok(to_wasm_java_version(client.version.load()))
    }

    fn get_brand(
        &mut self,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
    ) -> wasmtime::Result<String> {
        let player = self.get(&player)?.clone();

        let client = player
            .client
            .java()
            .ok_or_else(|| wasmtime::Error::msg("Not a java player"))?;
        Ok(client.brand.load().as_ref().clone().unwrap_or_default())
    }

    fn get_server_address(
        &mut self,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
    ) -> wasmtime::Result<String> {
        let player = self.get(&player)?.clone();

        let client = player
            .client
            .java()
            .ok_or_else(|| wasmtime::Error::msg("Not a java player"))?;
        Ok(client.server_address.clone())
    }

    fn get_settings(
        &mut self,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
    ) -> wasmtime::Result<pumpkin::plugin::player::JavaPlayerSettings> {
        let player = self.get(&player)?.clone();

        let config = player.config.load();
        let mask = config.skin_parts;
        let mut parts = SkinParts::empty();
        if mask & 0x01 != 0 {
            parts |= SkinParts::CAPE;
        }
        if mask & 0x02 != 0 {
            parts |= SkinParts::JACKET;
        }
        if mask & 0x04 != 0 {
            parts |= SkinParts::LEFT_SLEEVE;
        }
        if mask & 0x08 != 0 {
            parts |= SkinParts::RIGHT_SLEEVE;
        }
        if mask & 0x10 != 0 {
            parts |= SkinParts::LEFT_PANTS_LEG;
        }
        if mask & 0x20 != 0 {
            parts |= SkinParts::RIGHT_PANTS_LEG;
        }
        if mask & 0x40 != 0 {
            parts |= SkinParts::HAT;
        }

        Ok(pumpkin::plugin::player::JavaPlayerSettings {
            locale: config.locale.clone(),
            view_distance: config.view_distance.get(),
            chat_mode: to_wasm_chat_mode(&config.chat_mode),
            chat_colors: config.chat_colors,
            skin_parts: parts,
            main_hand: match config.main_hand {
                pumpkin_util::Hand::Left => pumpkin::plugin::common::Hand::Left,
                pumpkin_util::Hand::Right => pumpkin::plugin::common::Hand::Right,
            },
            text_filtering: config.text_filtering,
            server_listing: config.server_listing,
        })
    }

    fn get_scoreboard(
        &mut self,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
    ) -> wasmtime::Result<Resource<pumpkin::plugin::scoreboard::Scoreboard>> {
        let player = self.get(&player)?.clone();
        self.add(pumpkin_wasm_host_common::state::ScoreboardProvider::Player(
            player,
        ))
    }

    fn reset_scoreboard(
        &mut self,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?.clone();
        player.reset_scoreboard();
        Ok(())
    }

    fn send_game_event(
        &mut self,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
        event: pumpkin::plugin::player::ClientGameEvent,
        value: f32,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?.clone();

        let internal_event = from_wit_client_game_event(event);
        player.send_game_event(internal_event, value);
        Ok(())
    }

    fn send_entity_status(
        &mut self,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
        entity_id: i32,
        status: pumpkin::plugin::entity_statuses::EntityStatus,
    ) -> wasmtime::Result<()> {
        let player = self.get(&player)?.clone();

        // SAFETY: The WIT enum variants and discriminants are 1:1 generated from entity_statuses.json
        let internal_status: pumpkin_data::entity_status::EntityStatus =
            unsafe { std::mem::transmute(status as u8) };
        let packet = pumpkin_protocol::java::client::play::CEntityStatus::new(
            entity_id,
            internal_status as u8 as i8,
        );
        player.try_send_client_packet(&packet);
        Ok(())
    }

    fn drop(&mut self, rep: Resource<pumpkin::plugin::player::JavaPlayer>) -> wasmtime::Result<()> {
        self.drop(rep)
    }
}

impl pumpkin::plugin::player::HostJavaPlayerWithStore<PluginHostState>
    for HasSelf<PluginHostState>
{
    async fn send_packet(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
        packet: pumpkin::plugin::java_packets::ClientboundPacket,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?.clone();

        let client = player
            .client
            .java()
            .ok_or_else(|| wasmtime::Error::msg("Not a java player"))?;
        if let Some(bytes) = crate::generated_packets::serialize_java_packet(
            &packet,
            pumpkin_data::packet::CURRENT_MC_VERSION,
        ) {
            client.send_packet_now_data(bytes).await;
        }
        Ok(())
    }

    async fn send_custom_payload(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
        channel: String,
        data: Vec<u8>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?.clone();

        {
            let pumpkin_core::net::ClientPlatform::Java(_) = player.client.as_ref();
            player
                .send_client_packet(&pumpkin_protocol::java::client::play::CCustomPayload::new(
                    &channel, &data,
                ))
                .await;
        };
        Ok(())
    }

    async fn send_resource_pack(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
        pack: pumpkin::plugin::player::JavaResourcePack,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?.clone();

        let uuid = Uuid::from_wit(&pack.id);
        let prompt_message = pack
            .prompt_message
            .map(|t| accessor.take_res(t))
            .transpose()?;

        {
            let pumpkin_core::net::ClientPlatform::Java(client) = player.client.as_ref();
            client
                .send_packet(
                    &pumpkin_protocol::java::client::play::CAddResourcePack::new(
                        &uuid,
                        &pack.url,
                        &pack.hash,
                        pack.forced,
                        prompt_message,
                    ),
                )
                .await;
        };
        Ok(())
    }

    async fn remove_resource_pack(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
        id: pumpkin::plugin::uuid::Uuid,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?.clone();

        let uuid = Uuid::from_wit(&id);

        {
            let pumpkin_core::net::ClientPlatform::Java(client) = player.client.as_ref();
            client
                .send_packet(
                    &pumpkin_protocol::java::client::play::CRemoveResourcePack::new(Some(&uuid)),
                )
                .await;
        };
        Ok(())
    }

    async fn clear_resource_packs(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
    ) -> wasmtime::Result<()> {
        let player = accessor.get_res(&player)?.clone();

        {
            let pumpkin_core::net::ClientPlatform::Java(client) = player.client.as_ref();
            client
                .send_packet(&pumpkin_protocol::java::client::play::CRemoveResourcePack::new(None))
                .await;
        };
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn show_dialog(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
        dialog: Dialog,
    ) -> wasmtime::Result<()> {
        let (player, protocol_dialog) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            let player = state.get(&player)?.clone();
            let protocol_dialog = super::events::dialog::protocol_dialog_from_wasm(state, &dialog);
            Ok((player, protocol_dialog))
        })?;

        let operation = async move {
            if let Some(server) = player.world().server.upgrade() {
                let mut event =
                    pumpkin_core::plugin::api::events::dialog::dialog_show::DialogShowEvent::new(
                        player.clone(),
                        protocol_dialog.clone(),
                    );
                server.plugin_manager.fire(&server, &mut event).await;
                if event.cancelled {
                    return Ok(());
                }
            }

            {
                let pumpkin_core::net::ClientPlatform::Java(client) = player.client.as_ref();
                match client.connection_state.load() {
                    pumpkin_protocol::ConnectionState::Config => {
                        client
                            .send_packet(
                                &pumpkin_protocol::java::client::config::CConfigShowDialog::new(
                                    pumpkin_protocol::IdOr::Value(DialogNBT::from_dialog(
                                        &protocol_dialog,
                                    )),
                                ),
                            )
                            .await;
                    }
                    pumpkin_protocol::ConnectionState::Play => {
                        client
                            .send_packet(
                                &pumpkin_protocol::java::client::play::CPlayShowDialog::new(
                                    pumpkin_protocol::IdOr::Value(DialogNBT::from_dialog(
                                        &protocol_dialog,
                                    )),
                                ),
                            )
                            .await;
                    }
                    _ => {}
                }
            }

            Ok(())
        };

        in_active_context(accessor, operation).await?
    }

    async fn clear_dialog(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
    ) -> wasmtime::Result<()> {
        let player = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            let player = state.get(&player)?.clone();
            Ok(player)
        })?;

        let operation = async move {
            if let Some(server) = player.world().server.upgrade() {
                let mut event =
                    pumpkin_core::plugin::api::events::dialog::dialog_clear::DialogClearEvent::new(
                        player.clone(),
                    );
                server.plugin_manager.fire(&server, &mut event).await;
                if event.cancelled {
                    return Ok(());
                }
            }

            {
                let pumpkin_core::net::ClientPlatform::Java(client) = player.client.as_ref();
                match client.connection_state.load() {
                    pumpkin_protocol::ConnectionState::Config => {
                        client
                            .send_packet(
                                &pumpkin_protocol::java::client::config::CConfigClearDialog::new(),
                            )
                            .await;
                    }
                    pumpkin_protocol::ConnectionState::Play => {
                        client
                            .send_packet(
                                &pumpkin_protocol::java::client::play::CPlayClearDialog::new(),
                            )
                            .await;
                    }
                    _ => {}
                }
            }

            Ok(())
        };

        in_active_context(accessor, operation).await?
    }

    async fn kick(
        accessor: &Accessor<PluginHostState, Self>,
        player: Resource<pumpkin::plugin::player::JavaPlayer>,
        options: pumpkin::plugin::player::JavaKickOptions,
    ) -> wasmtime::Result<()> {
        let (player, reason) = accessor.with(|mut host| -> wasmtime::Result<_> {
            let state = host.get();
            let player = state.get(&player)?.clone();
            let reason = state.take(options.reason)?;
            Ok((player, reason))
        })?;

        if options.log_to_console {
            tracing::info!(
                "Kicking Java player {} ({}): {}",
                player.gameprofile.name,
                player.gameprofile.id,
                reason.clone().to_pretty_console()
            );
        }

        let operation = async move {
            if let Some(server) = player.world().server.upgrade()
                && let Some(player_arc) = player.world().get_player_by_uuid(player.gameprofile.id)
            {
                let mut event =
                    pumpkin_core::plugin::api::events::player::player_kick::PlayerKickEvent::new(
                        player_arc,
                        reason.clone().to_pretty_console(),
                    );
                server.plugin_manager.fire(&server, &mut event).await;
                if event.cancelled {
                    return Ok(());
                }
            }

            let send_packet = options.teardown_policy
                != pumpkin::plugin::player::SocketTeardownPolicy::DropConnection;
            {
                let pumpkin_core::net::ClientPlatform::Java(java) = player.client.as_ref();
                java.kick_explicit(&reason, send_packet).await;
            };

            Ok(())
        };

        in_active_context(accessor, operation).await?
    }
}

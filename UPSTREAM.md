# Upstream provenance and FerrumMC patches

Snapshot: 2026-10-29. This directory is a source copy, not a Git checkout. No `git init`, commits, remote writes or pull requests were performed. Reference repositories were read only. Generated build/runtime downloads live in ignored workspace caches and scratch directories.

## Compatibility tuple

- Pumpkin: https://github.com/Pumpkin-MC/Pumpkin, `f2cff8e6507a752e219a811d7aca58a8f005b1a2` (local reference HEAD, latest supplied snapshot). Workspace version `0.2.1+26.3-26.60`, internal Minecraft 26.3/protocol 777, native plugin API 3. The engine was not downgraded.
- PatchBukkit: https://github.com/Pumpkin-MC/PatchBukkit, `80a521938809f3054a042fc70885d249cde89f08`, vendored under `vendor/patchbukkit`, GPL-3.0. Sources retain Java/proto/Rust relative layout and upstream license.
- Horizon reference only: https://github.com/horizon-rust/horizon, `0ac347d5b0cc8c92f0be26a64a06197eeaae492b`, original code MIT. No implementation copied.
- Arclight reference only: https://github.com/IzzelAliz/Arclight, `f98b864928bd5f0b2331d41fe7748355eddfa6cc`, GPLv3. No implementation copied.
- Rust: stable 1.99.1, x86_64-pc-windows-msvc. `scripts/env.ps1` checks the exact installed compiler, while `rust-toolchain.toml` selects the installed stable channel. Build engine and native DLL together.
- JVM: Eclipse Temurin 25.0.2+10, `C:/Program Files/Eclipse Adoptium/jdk-25.0.2.10-hotspot`.
- Gradle: checked-in PatchBukkit wrapper 9.8.1. Paper compile bundle/API `26.3.build.205-stable`; runtime Paper 26.3 build 205, locally acquired and digest-verified by PatchBukkit.
- JNI 0.22.4; Rust dependency resolution is the root `Cargo.lock`. Inherited Gradle paperweight userdev uses `2.0.0-SNAPSHOT`, which is still a reproducibility limitation; source pins and Paper build pins do not make that plugin immutable.

## Workspace integration

The root workspace includes `vendor/patchbukkit/rust` and defaults to building both `pumpkin` (binary `ferrummc`) and `patchbukkit` (native library). All six PatchBukkit engine dependencies resolve to the local workspace: `pumpkin` aliases `pumpkin-core`; `pumpkin-data`, `pumpkin-protocol`, `pumpkin-util`, `pumpkin-config`, and `pumpkin-api-macros` use workspace paths. Shared Tokio, UUID, Serde, rand, bytes, TOML, ZIP, SHA-2 and tracing dependencies follow the engine workspace. No separate Git copy of `pumpkin-core` participates in the native ABI.

The vendored standalone Rust lock and toolchain files are intentionally omitted in favor of the root files. Build Java first, because the native build script embeds `java/patchbukkit/build/libs/patchbukkit.jar`. Upstream already supplies vendored protoc and Java-jar rebuild tracking; these were retained. No Mojang runtime jar is vendored as source or intended for distribution.

## Touched Pumpkin files

Compared with the supplied Pumpkin snapshot, excluding line-ending differences:

- `Cargo.toml`: PatchBukkit member/default member; debug iteration settings retained from the resumed workspace.
- `Cargo.lock`: added PatchBukkit and build dependencies into one locked graph.
- `crates/pumpkin/Cargo.toml`: executable name and Windows resource description/filename.
- `crates/pumpkin/src/main.rs`: FerrumMC display/debug/development branding, preserving Pumpkin credit.
- `crates/pumpkin-config/src/lib.rs`: default configuration filename `ferrummc.toml`.
- `crates/pumpkin-config/src/networking/java.rs`: default FerrumMC MOTD.
- `crates/pumpkin-core/src/net/java/mod.rs`: isolated FerrumMC minimum admission version 26.2; updated compatibility comment.
- `crates/pumpkin-core/src/net/java/handshake.rs`: admission uses that policy, retaining current-version upper bound and transfer behavior.
- `crates/pumpkin-core/src/server/connection_cache.rs`: FerrumMC wire brand/default MOTD and status admission range 26.2–26.3.
- `rust-toolchain.toml`: stable selection plus exact-version check documentation.
- `.gitignore`: local caches, scratch runtime and sample plugin sources/artifacts.
- `README.md`: FerrumMC scope, commands, credits and license links.
- `AGENTS.md`: FerrumMC architecture and docs pointers; retained Pumpkin parity, quality and no-agent-PR rules.

Extracted game-data JSON was not edited. Runtime registry hooks in generated block/item source are regenerated from the owned generator changes listed below. The old multi-version commit `05f3f8b4f7c8268ccc420e6f3167cb05e53345fb` exists in history, but this HEAD has current-only packet IDs and 26.3 registries. Admission changes are not an implementation of full client translation. See `docs/PHASE1.md` for measured differences and test limits.

## Touched PatchBukkit files

- `rust/Cargo.toml`: local/workspace engine and shared dependency alignment; inherited metadata/license retained.
- `java/settings.gradle.kts`: includes the owned `tests/plugins/hello-plugin` project.
- `java/patchbukkit/src/main/java/org/patchbukkit/bootstrap/HeadlessPaperServer.java`: default server-name option is FerrumMC.
- `.gitignore`: allows source trees whose directory names also match runtime ignore patterns.
- `rust/tests/common/harness.rs` and `rust/tests/bot_integration_test.rs`: adapt to Pumpkin's new `standalone` constructor argument; compile Unix-only cleanup declarations only on Unix.
- Removed standalone `rust/Cargo.lock` and `rust/rust-toolchain.toml` from the copy, as described above.

FerrumMC-owned additions include `scripts/*.ps1`, `tests/smoke/*`, `tests/plugins/hello-plugin/*`, this file, and Phase 1 documentation. Research and concurrent NeoForge artifacts are separate work; this implementation does not claim authorship of them.

## NeoForge Rust bootstrap additions

- `Cargo.toml` and `Cargo.lock`: added the owned `ferrum-neoforge` member using the same pinned JNI 0.22.4 as PatchBukkit.
- `crates/pumpkin/Cargo.toml` and `crates/pumpkin/src/main.rs`: optional `ferrum-neoforge.json` boot/shutdown hooks, with blocking FML work outside Tokio workers. An absent or disabled config preserves the no-mod path.
- Owned `crates/ferrum-neoforge`: embedded-JVM lifecycle, frozen-registry readiness checks, full Java exception diagnostics, validated names/property manifests and boundary tests.
- Owned `bridge/neoforge-runtime` entry/content classes: added `registryManifest()` without starting a Java server or world.
- Owned `scripts/test-neoforge-bridge.ps1`: scratch server placement, tick/NBT and restart acceptance runner. This runner must fail until runtime state handles, Rust chunk storage, world redirects and BE ticking are implemented. Exporting a manifest does not install modded IDs in Pumpkin.

## NeoForge runtime registry and block entity integration

- `tools/pumpkin-codegen/src/block.rs` and `tools/pumpkin-codegen/src/item.rs`: generated lookup fallbacks for frozen runtime additions; vanilla constants and numeric IDs retained.
- `crates/pumpkin-data/src/generated/block.rs` and `crates/pumpkin-data/src/generated/item.rs`: regenerated only those two outputs with `pumpkin-codegen -- block item`.
- `crates/pumpkin-data/src/lib.rs`, `crates/pumpkin-data/src/blocks.rs`, `crates/pumpkin-data/src/block_state.rs`, and new `crates/pumpkin-data/src/runtime.rs`: immutable append-only registry tables, names/property identities and runtime ID lookup; vanilla ID constructors remain const.
- `crates/pumpkin-world/src/chunk/format/mod.rs`: names/property NBT palettes for runtime worlds in both region backends; refuse unresolved mod states.
- `crates/pumpkin-world/src/generation/rule/block_match.rs`: runtime-capable lookup requires a normal function instead of const.
- `crates/pumpkin-world/tests/runtime_registry.rs`: round-trip custom property/name storage and reject unknown mod content.
- `crates/pumpkin-core/Cargo.toml`, `crates/pumpkin-core/src/lib.rs`, and new `crates/pumpkin-core/src/neoforge.rs`: synchronous JNI batch leases and Rust-owned block entity snapshots; defer callbacks until adjacent Rust chunks are loaded.
- `crates/pumpkin-data/src/item_stack/mod.rs`: preserve a runtime item's namespace in persisted stacks.
- `crates/pumpkin-core/src/block/registry.rs`: safely return no native behavior for appended Java block IDs.
- `crates/pumpkin-core/src/block/entities/mod.rs`: create and restore registered mod block entities through the existing factories.
- `crates/pumpkin-core/src/world/mod.rs`: serial deterministic mod ticker batch after native BE workers finish; expose the existing update helper within the crate.- `crates/pumpkin-core/src/command/commands/forceload.rs`: balance tickets when adding/removing forced chunks. Match vanilla `ChunkMap.FORCED_TICKET_LEVEL` at entity-ticking status relative to Pumpkin's full-chunk level, so neighbors needed by block entities are also loaded.
- Owned `scripts/test-neoforge-bridge.ps1`: explicitly force-load and wait for the acceptance chunk before placing/ticking machines.
- `crates/pumpkin-core/src/command/commands/mod.rs` and new `crates/pumpkin-core/src/command/commands/ferrum.rs`: permission-gated `/ferrum be` NBT inspection.
- `Cargo.lock`: local bridge dependency edges, using already locked dependencies.
- Owned `crates/ferrum-neoforge` and `bridge/neoforge-runtime`: JNI world redirect, registry installation, real BlockEntityTicker invocation, registry-aware Java NBT serialization and missing-mod startup gate. Rust-backed chunk-source/block-entity views support HBM's transformed `setChanged` path; capability listeners and HBM's injected field initializers are initialized on the leased view. Retain the loaded recipe/fuel tables for machine callbacks. No Java server/world constructor, world tick or save loop is run.

## NeoForge client configuration negotiation

- `crates/pumpkin-core/src/net/java/login/login_acknowledged.rs`: with the bridge enabled, 26.2 connections open a NeoForge configuration session instead of starting vanilla configuration; the former body is `start_vanilla_configuration`, run when NeoForge reaches the vanilla registry-sync task.
- `crates/pumpkin-core/src/net/java/login/known_packs.rs`, `login_acknowledged.rs` and `pending.rs`: with the bridge, the NeoForge JVM offers the known packs and sends the synchronized registries and tags (mod datapack entries included) from vanilla's `SynchronizeRegistriesTask`; the session then sends `FinishConfiguration` after the remaining modded tasks.
- `crates/pumpkin-core/src/net/java/pending.rs`: `neoforge` flag; configuration custom payloads and the negotiation pong (id 0) are forwarded to the session; the session closes on finish acknowledgement.
- `crates/pumpkin-core/src/net/java/mod.rs` and new `neoforge.rs`: session driver; Java-encoded 26.2 configuration packets are written as-is.
- `crates/pumpkin-core/src/net/java/compatibility.rs`: runtime (modded) block states, items, blocks and block entity types map to the NeoForge frozen-registry IDs the client received; the direct block palette width follows the synced state count; vanilla synchronized-registry entries (biome, damage type, enchantment, jukebox song, ...) map to the client IDs after mod entries sort in.
- `crates/pumpkin-data/src/runtime.rs`: `block_entity_type_id` lookup.
- Owned `crates/ferrum-neoforge` (`registry.rs`, `world.rs`) and `bridge/neoforge-runtime` (`ConfigBridge`, `BridgeContent`, `BridgeRuntime`): virtual server configuration listener running NeoForge's `NetworkRegistry` negotiation, early `SyncRegistries`, `RegisterConfigurationTasksEvent` tasks and codecs; SERVER mod configs load at boot for `SyncConfig`; manifest carries block and block entity type IDs. Tasks finishing via `tick()` are ticked immediately after each step (vanilla ticks them on the next server tick).
- Owned `scripts/test-neoforge-client.ps1` and the `clientProbe`/`ClientAcceptance` sources: headless NeoForge client acceptance using the pinned codecs.

## Update policy

Refresh Pumpkin and PatchBukkit as a tested tuple. Retain source hashes, licenses and a short patch inventory. Never fetch, build or patch inside reference repositories. Recheck native ABI alignment, clean boot, JVM/plugin readiness, protocol admission and real-client protection behavior on every update. Do not silently substitute a 26.2 engine or run Java world simulation to make a bridge appear compatible.

## Phase 2 client translation and gameplay smoke

The engine/data baseline remains 26.3. FerrumMC-owned `assets/compat/26_2` contains official 26.2 datagen reports and synchronized-registry JSON; its README pins the server jar digest and reproduction command. Runtime jars/decompiled source stay outside the repository. The dependency-free smoke captures wire traffic, and `tests/smoke/VanillaOracle.java` checks it with digest-pinned unmodified 26.2/26.3 codecs and registry loaders.

Additional Pumpkin changes for this work:

- `crates/pumpkin-core/src/net/java/compatibility.rs`: built-in 26.2 packet/registry/state/item translation at the wire boundary, keeping canonical simulation/storage unchanged; a regression covers the reordered entity movement ground flag.
- `crates/pumpkin-core/src/net/java/compatibility.rs` (again): entity data (items, block states, particles, variants), mob effects, damage events, chat (raw chat type becomes a system message), merchant offers, block entity data, jukebox song events and enchantment/potion item components map to the client IDs; `crates/pumpkin-core/src/neoforge.rs` reports block entities skipped for unloaded neighbours as retained.
- `crates/pumpkin-core/src/net/java/mod.rs`: apply built-in play translation when no translating packet plugin rewrites the ID/payload; passive PatchBukkit observers do not disable translation. Read older teleport/dig/swing/sign payloads with the negotiated format. Cancellation and actual packet-event translator overrides retain their contract.
- `crates/pumpkin-core/src/net/java/pending.rs`: equivalent configuration boundary translation and packet-plugin override handling.
- `crates/pumpkin-core/src/net/java/login/known_packs.rs` and `login_acknowledged.rs`: 26.2 known-pack version and compatible registry payloads, retaining current engine registries for 777 and external translating plugins.
- `crates/pumpkin-data/src/lib.rs`: export generated compatibility data.
- `tools/pumpkin-codegen/src/main.rs`, new `compatibility.rs`, and `registry.rs`: generate mappings from names/property identities and versioned synchronized network NBT.
- `crates/pumpkin-data/src/generated/compatibility.rs` (new) and `generated/registry.rs`: regenerated from those inputs; no hand edits.

Owned smoke changes are `scripts/smoke.ps1`, new `scripts/test-vanilla-codecs.ps1`, `tests/smoke/client.rs`, the isolated fixture config and vanilla oracle. The harness stages the executable as well as the DLL, avoiding a lock on the shared build executable while smoke runs. The client moves a creative acacia boat through a container click, requires a fresh full inventory snapshot, and compares all 46 slots, counts, component removals and cursor after reconnect without rewriting the reconnected inventory. The hello plugin's cancellation log now includes the player name so the harness can prove both protocols reached the Bukkit listener. `docs/PHASE1.md` and `docs/PHASE2.md` record commands, evidence, scope and real-client checks. Temporary gameplay tracing was removed; no simulation behavior or Bukkit cancellation timing was changed for this task. Concurrent NeoForge/runtime-registry changes belong to their separate implementation. No `pumpkin-protocol` source change was needed for the completion work.

The vanilla oracle applies each release's built-in data-component initializers against the received registry access before decoding non-empty stacks, and reports nested decoder causes. Its runner reuses extracted cached jars without requiring unused datagen reports.

## Java-only NeoForge admission (2026-10-30)

Java configuration requires completed NeoForge negotiation; status uses NeoForge 26.2's `isModded` extension. The executable requires an enabled bridge. Configuration disconnects encode text as NBT.

Touched upstream files: `crates/pumpkin-core/src/lib.rs`, `crates/pumpkin/src/main.rs`, `crates/pumpkin-core/src/net/java/status.rs`, `crates/pumpkin-core/src/net/java/pending.rs`, `crates/pumpkin-core/src/net/java/login/login_acknowledged.rs`, `crates/pumpkin-protocol/src/java/client/config/config_disconnect.rs`. Ferrum-owned negotiation, runtime, smoke probe and scripts changed alongside them.

## Bedrock removed

Owner decision: FerrumMC supports Java clients only, including its negotiated NeoForge clients. Removed the other edition's listeners, NetherNet signaling/ICE, status/authentication, packet handlers, player paths, forms, scoreboards, entity metadata, network NBT and Java conversion tables. Placement and block updates no longer call the vanilla-only block-state conversion at all. Rust remains the only world authority.

Legacy `networking.bedrock` tables are ignored by Serde and omitted from newly serialized configuration; a regression covers invalid legacy settings alongside a valid Java address. Java LAN announcements remain. References to the vanilla bedrock block, world-generation floor/roof rules, and Mojang's Java translation/data keys remain intentional; a raw case-insensitive search for `bedrock` therefore also finds game content.

The native plugin API is now version 4. Both WIT interfaces and SDK/host bindings intentionally remove the retired platform surface, so existing plugins using that surface must be rebuilt. Generated outputs come from `cargo run --locked -p pumpkin-codegen`; the generator now formats as Rust 2024, normalizes Windows template-pool paths, and emits the Java packet mappings for both host versions. Shared item-stack identities remain because Java item-merge validation uses them.

Removed upstream files:

- `assets/bedrock/LICENSE-GEYSER`
- `assets/bedrock/README.md`
- `assets/bedrock/biome_definitions.nbt`
- `assets/bedrock/biomes.json`
- `assets/bedrock/block_states.nbt`
- `assets/bedrock/blocks.nbt`
- `assets/bedrock/creative_items.json`
- `assets/bedrock/item_components.nbt`
- `assets/bedrock/item_data_overrides.json`
- `assets/bedrock/player_geometry.json`
- `assets/bedrock/runtime_item_states.json`
- `assets/en_us_bedrock.lang`
- `crates/pumpkin-auth/src/jwt/mod.rs`
- `crates/pumpkin-config/src/networking/bedrock.rs`
- `crates/pumpkin-core/src/block/entities/bedrock.rs`
- `crates/pumpkin-core/src/net/bedrock/level_chunk.rs`
- `crates/pumpkin-core/src/net/bedrock/login/login.rs`
- `crates/pumpkin-core/src/net/bedrock/login/mod.rs`
- `crates/pumpkin-core/src/net/bedrock/login/request_network_settings.rs`
- `crates/pumpkin-core/src/net/bedrock/login/resource_pack_response.rs`
- `crates/pumpkin-core/src/net/bedrock/mod.rs`
- `crates/pumpkin-core/src/net/bedrock/nethernet.rs`
- `crates/pumpkin-core/src/net/bedrock/nethernet/discovery.rs`
- `crates/pumpkin-core/src/net/bedrock/nethernet/endpoint.rs`
- `crates/pumpkin-core/src/net/bedrock/nethernet/ice_router.rs`
- `crates/pumpkin-core/src/net/bedrock/nethernet/identity.rs`
- `crates/pumpkin-core/src/net/bedrock/nethernet/peer.rs`
- `crates/pumpkin-core/src/net/bedrock/nethernet/session.rs`
- `crates/pumpkin-core/src/net/bedrock/nethernet/state.rs`
- `crates/pumpkin-core/src/net/bedrock/play/actor_event.rs`
- `crates/pumpkin-core/src/net/bedrock/play/animate.rs`
- `crates/pumpkin-core/src/net/bedrock/play/block_actor_data.rs`
- `crates/pumpkin-core/src/net/bedrock/play/block_pick_request.rs`
- `crates/pumpkin-core/src/net/bedrock/play/chat_command.rs`
- `crates/pumpkin-core/src/net/bedrock/play/chat_message.rs`
- `crates/pumpkin-core/src/net/bedrock/play/container_close.rs`
- `crates/pumpkin-core/src/net/bedrock/play/emote.rs`
- `crates/pumpkin-core/src/net/bedrock/play/emote_list.rs`
- `crates/pumpkin-core/src/net/bedrock/play/interaction.rs`
- `crates/pumpkin-core/src/net/bedrock/play/inventory_action.rs`
- `crates/pumpkin-core/src/net/bedrock/play/item_stack_request.rs`
- `crates/pumpkin-core/src/net/bedrock/play/mob_equipment.rs`
- `crates/pumpkin-core/src/net/bedrock/play/mod.rs`
- `crates/pumpkin-core/src/net/bedrock/play/modal_form_response.rs`
- `crates/pumpkin-core/src/net/bedrock/play/player_action.rs`
- `crates/pumpkin-core/src/net/bedrock/play/player_auth_input.rs`
- `crates/pumpkin-core/src/net/bedrock/play/player_block_action.rs`
- `crates/pumpkin-core/src/net/bedrock/play/player_hotbar.rs`
- `crates/pumpkin-core/src/net/bedrock/play/request_ability.rs`
- `crates/pumpkin-core/src/net/bedrock/play/request_chunk_radius.rs`
- `crates/pumpkin-core/src/net/bedrock/play/respawn.rs`
- `crates/pumpkin-core/src/net/bedrock/play/set_local_player_as_initialized.rs`
- `crates/pumpkin-core/src/net/bedrock/recipe.rs`
- `crates/pumpkin-core/src/net/bedrock/status.rs`
- `crates/pumpkin-core/src/plugin/api/events/player/bedrock_form_response.rs`
- `crates/pumpkin-data/src/generated/bedrock_biome.rs`
- `crates/pumpkin-data/src/generated/bedrock_creative.rs`
- `crates/pumpkin-nbt/fuzz/fuzz_targets/deserialize_bedrock.rs`
- `crates/pumpkin-plugin-api/src/events/player/bedrock_form_response.rs`
- `crates/pumpkin-plugin-api/src/forms.rs`
- `crates/pumpkin-plugin-wit/v0.1/bedrock-packets.wit`
- `crates/pumpkin-plugin-wit/v0.1/forms.wit`
- `crates/pumpkin-plugin-wit/v0.2/bedrock-packets.wit`
- `crates/pumpkin-plugin-wit/v0.2/forms.wit`
- `crates/pumpkin-protocol/fuzz/fuzz_targets/decoder_bedrock.rs`
- `crates/pumpkin-protocol/fuzz/fuzz_targets/encoder_bedrock.rs`
- `crates/pumpkin-protocol/src/bedrock/client/add_actor.rs`
- `crates/pumpkin-protocol/src/bedrock/client/add_item_actor.rs`
- `crates/pumpkin-protocol/src/bedrock/client/add_player.rs`
- `crates/pumpkin-protocol/src/bedrock/client/available_commands.rs`
- `crates/pumpkin-protocol/src/bedrock/client/biome_definition_list.rs`
- `crates/pumpkin-protocol/src/bedrock/client/block_actor_data.rs`
- `crates/pumpkin-protocol/src/bedrock/client/block_event.rs`
- `crates/pumpkin-protocol/src/bedrock/client/boss_event.rs`
- `crates/pumpkin-protocol/src/bedrock/client/change_dimension.rs`
- `crates/pumpkin-protocol/src/bedrock/client/chunk_radius_updated.rs`
- `crates/pumpkin-protocol/src/bedrock/client/client_cache_miss_response.rs`
- `crates/pumpkin-protocol/src/bedrock/client/common.rs`
- `crates/pumpkin-protocol/src/bedrock/client/container_open.rs`
- `crates/pumpkin-protocol/src/bedrock/client/correct_player_move_prediction.rs`
- `crates/pumpkin-protocol/src/bedrock/client/crafting_data.rs`
- `crates/pumpkin-protocol/src/bedrock/client/creative_content.rs`
- `crates/pumpkin-protocol/src/bedrock/client/disconnect.rs`
- `crates/pumpkin-protocol/src/bedrock/client/gamerules_changed.rs`
- `crates/pumpkin-protocol/src/bedrock/client/inventory_content.rs`
- `crates/pumpkin-protocol/src/bedrock/client/inventory_slot.rs`
- `crates/pumpkin-protocol/src/bedrock/client/item_registry.rs`
- `crates/pumpkin-protocol/src/bedrock/client/item_stack_response.rs`
- `crates/pumpkin-protocol/src/bedrock/client/jigsaw_structure_data.rs`
- `crates/pumpkin-protocol/src/bedrock/client/level_event.rs`
- `crates/pumpkin-protocol/src/bedrock/client/level_sound_event.rs`
- `crates/pumpkin-protocol/src/bedrock/client/mob_effect.rs`
- `crates/pumpkin-protocol/src/bedrock/client/mob_equipment.rs`
- `crates/pumpkin-protocol/src/bedrock/client/mod.rs`
- `crates/pumpkin-protocol/src/bedrock/client/modal_form_request.rs`
- `crates/pumpkin-protocol/src/bedrock/client/move_actor_absolute.rs`
- `crates/pumpkin-protocol/src/bedrock/client/move_actor_delta.rs`
- `crates/pumpkin-protocol/src/bedrock/client/move_player.rs`
- `crates/pumpkin-protocol/src/bedrock/client/network_chunk_publisher_update.rs`
- `crates/pumpkin-protocol/src/bedrock/client/network_settings.rs`
- `crates/pumpkin-protocol/src/bedrock/client/open_sign.rs`
- `crates/pumpkin-protocol/src/bedrock/client/play_status.rs`
- `crates/pumpkin-protocol/src/bedrock/client/player_hotbar.rs`
- `crates/pumpkin-protocol/src/bedrock/client/player_list.rs`
- `crates/pumpkin-protocol/src/bedrock/client/remove_actor.rs`
- `crates/pumpkin-protocol/src/bedrock/client/remove_objective.rs`
- `crates/pumpkin-protocol/src/bedrock/client/resource_pack_chunk_data.rs`
- `crates/pumpkin-protocol/src/bedrock/client/resource_pack_data_info.rs`
- `crates/pumpkin-protocol/src/bedrock/client/resource_pack_stack.rs`
- `crates/pumpkin-protocol/src/bedrock/client/resource_packs_info.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_actor_data.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_actor_link.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_actor_motion.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_difficulty.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_display_objective.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_health.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_player_gamemode.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_score.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_spawn_position.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_time.rs`
- `crates/pumpkin-protocol/src/bedrock/client/set_title.rs`
- `crates/pumpkin-protocol/src/bedrock/client/show_credits.rs`
- `crates/pumpkin-protocol/src/bedrock/client/start_game.rs`
- `crates/pumpkin-protocol/src/bedrock/client/take_item_actor.rs`
- `crates/pumpkin-protocol/src/bedrock/client/transfer.rs`
- `crates/pumpkin-protocol/src/bedrock/client/update_abilities.rs`
- `crates/pumpkin-protocol/src/bedrock/client/update_attributes.rs`
- `crates/pumpkin-protocol/src/bedrock/client/update_block.rs`
- `crates/pumpkin-protocol/src/bedrock/client/update_trade.rs`
- `crates/pumpkin-protocol/src/bedrock/client/voxel_shapes.rs`
- `crates/pumpkin-protocol/src/bedrock/enum_as_str.rs`
- `crates/pumpkin-protocol/src/bedrock/mod.rs`
- `crates/pumpkin-protocol/src/bedrock/network_item.rs`
- `crates/pumpkin-protocol/src/bedrock/packet_decoder.rs`
- `crates/pumpkin-protocol/src/bedrock/packet_encoder.rs`
- `crates/pumpkin-protocol/src/bedrock/server/actor_event.rs`
- `crates/pumpkin-protocol/src/bedrock/server/animate.rs`
- `crates/pumpkin-protocol/src/bedrock/server/block_actor_data.rs`
- `crates/pumpkin-protocol/src/bedrock/server/block_pick_request.rs`
- `crates/pumpkin-protocol/src/bedrock/server/client_cache_blob_status.rs`
- `crates/pumpkin-protocol/src/bedrock/server/client_cache_status.rs`
- `crates/pumpkin-protocol/src/bedrock/server/command_request.rs`
- `crates/pumpkin-protocol/src/bedrock/server/container_close.rs`
- `crates/pumpkin-protocol/src/bedrock/server/emote.rs`
- `crates/pumpkin-protocol/src/bedrock/server/emote_list.rs`
- `crates/pumpkin-protocol/src/bedrock/server/interact.rs`
- `crates/pumpkin-protocol/src/bedrock/server/inventory_transaction.rs`
- `crates/pumpkin-protocol/src/bedrock/server/item_stack_request.rs`
- `crates/pumpkin-protocol/src/bedrock/server/loading_screen.rs`
- `crates/pumpkin-protocol/src/bedrock/server/login.rs`
- `crates/pumpkin-protocol/src/bedrock/server/mob_equipment.rs`
- `crates/pumpkin-protocol/src/bedrock/server/mod.rs`
- `crates/pumpkin-protocol/src/bedrock/server/modal_form_response.rs`
- `crates/pumpkin-protocol/src/bedrock/server/packet_violation_warning.rs`
- `crates/pumpkin-protocol/src/bedrock/server/player_action.rs`
- `crates/pumpkin-protocol/src/bedrock/server/player_auth_input.rs`
- `crates/pumpkin-protocol/src/bedrock/server/player_hotbar.rs`
- `crates/pumpkin-protocol/src/bedrock/server/request_ability.rs`
- `crates/pumpkin-protocol/src/bedrock/server/request_chunk_radius.rs`
- `crates/pumpkin-protocol/src/bedrock/server/request_network_settings.rs`
- `crates/pumpkin-protocol/src/bedrock/server/resource_pack_chunk_request.rs`
- `crates/pumpkin-protocol/src/bedrock/server/resource_pack_client_response.rs`
- `crates/pumpkin-protocol/src/bedrock/server/respawn.rs`
- `crates/pumpkin-protocol/src/bedrock/server/set_local_player_as_initialized.rs`
- `crates/pumpkin-protocol/src/bedrock/server/set_player_inventory_options.rs`
- `crates/pumpkin-protocol/src/bedrock/server/text.rs`
- `crates/pumpkin-protocol/src/bedrock/status.rs`
- `crates/pumpkin-wasm-host-v0_1/src/forms.rs`
- `crates/pumpkin-wasm-host-v0_2/src/forms.rs`
- `tools/pumpkin-codegen/src/bedrock_biome.rs`
- `tools/pumpkin-codegen/src/bedrock_creative.rs`
- `tools/pumpkin-codegen/src/wit/v0_2/bedrock_packet.rs`

Updated upstream files (including the PatchBukkit kick caller):

- `Cargo.lock`
- `Cargo.toml`
- `crates/pumpkin-auth/Cargo.toml`
- `crates/pumpkin-auth/src/lib.rs`
- `crates/pumpkin-command/src/argument_types/attribute.rs`
- `crates/pumpkin-command/src/argument_types/block.rs`
- `crates/pumpkin-command/src/argument_types/block_predicate.rs`
- `crates/pumpkin-command/src/argument_types/component.rs`
- `crates/pumpkin-command/src/argument_types/coordinates/angle.rs`
- `crates/pumpkin-command/src/argument_types/coordinates/block_pos.rs`
- `crates/pumpkin-command/src/argument_types/coordinates/column_pos.rs`
- `crates/pumpkin-command/src/argument_types/coordinates/mod.rs`
- `crates/pumpkin-command/src/argument_types/coordinates/rotation.rs`
- `crates/pumpkin-command/src/argument_types/coordinates/swizzle.rs`
- `crates/pumpkin-command/src/argument_types/coordinates/vec2.rs`
- `crates/pumpkin-command/src/argument_types/coordinates/vec3.rs`
- `crates/pumpkin-command/src/argument_types/entity_anchor.rs`
- `crates/pumpkin-command/src/argument_types/gamemode.rs`
- `crates/pumpkin-command/src/argument_types/hex_color.rs`
- `crates/pumpkin-command/src/argument_types/item.rs`
- `crates/pumpkin-command/src/argument_types/item_predicate.rs`
- `crates/pumpkin-command/src/argument_types/mod.rs`
- `crates/pumpkin-command/src/argument_types/nbt.rs`
- `crates/pumpkin-command/src/argument_types/nbt_path.rs`
- `crates/pumpkin-command/src/argument_types/particle.rs`
- `crates/pumpkin-command/src/argument_types/resource.rs`
- `crates/pumpkin-command/src/argument_types/resource_or_tag.rs`
- `crates/pumpkin-command/src/argument_types/slot.rs`
- `crates/pumpkin-command/src/argument_types/sound_category.rs`
- `crates/pumpkin-command/src/argument_types/team_color.rs`
- `crates/pumpkin-command/src/argument_types/time.rs`
- `crates/pumpkin-command/src/argument_types/uuid.rs`
- `crates/pumpkin-command/src/errors/error_types.rs`
- `crates/pumpkin-command/src/node/dispatcher.rs`
- `crates/pumpkin-command/src/parser.rs`
- `crates/pumpkin-command/src/snbt/mod.rs`
- `crates/pumpkin-command/src/snbt/operations.rs`
- `crates/pumpkin-command/src/snbt/rules.rs`
- `crates/pumpkin-config/src/lib.rs`
- `crates/pumpkin-config/src/networking/mod.rs`
- `crates/pumpkin-config/src/resource_pack.rs`
- `crates/pumpkin-core/Cargo.toml`
- `crates/pumpkin-core/src/block/blocks/anvil.rs`
- `crates/pumpkin-core/src/block/blocks/barrel.rs`
- `crates/pumpkin-core/src/block/blocks/beacon.rs`
- `crates/pumpkin-core/src/block/blocks/bed.rs`
- `crates/pumpkin-core/src/block/blocks/blast_furnace.rs`
- `crates/pumpkin-core/src/block/blocks/brewing_stand.rs`
- `crates/pumpkin-core/src/block/blocks/chests.rs`
- `crates/pumpkin-core/src/block/blocks/crafting_table.rs`
- `crates/pumpkin-core/src/block/blocks/enchanting_table.rs`
- `crates/pumpkin-core/src/block/blocks/ender_chest.rs`
- `crates/pumpkin-core/src/block/blocks/furnace.rs`
- `crates/pumpkin-core/src/block/blocks/grindstone.rs`
- `crates/pumpkin-core/src/block/blocks/hopper.rs`
- `crates/pumpkin-core/src/block/blocks/lectern.rs`
- `crates/pumpkin-core/src/block/blocks/redstone/crafter.rs`
- `crates/pumpkin-core/src/block/blocks/redstone/dispenser.rs`
- `crates/pumpkin-core/src/block/blocks/redstone/dropper.rs`
- `crates/pumpkin-core/src/block/blocks/respawn_anchor.rs`
- `crates/pumpkin-core/src/block/blocks/shulker_box.rs`
- `crates/pumpkin-core/src/block/blocks/signs.rs`
- `crates/pumpkin-core/src/block/blocks/smoker.rs`
- `crates/pumpkin-core/src/block/blocks/stonecutter.rs`
- `crates/pumpkin-core/src/block/blocks/straw_bed.rs`
- `crates/pumpkin-core/src/block/entities/banner.rs`
- `crates/pumpkin-core/src/block/entities/barrel.rs`
- `crates/pumpkin-core/src/block/entities/beacon.rs`
- `crates/pumpkin-core/src/block/entities/bed.rs`
- `crates/pumpkin-core/src/block/entities/brewing_stand.rs`
- `crates/pumpkin-core/src/block/entities/campfire.rs`
- `crates/pumpkin-core/src/block/entities/chest_like_block_entity.rs`
- `crates/pumpkin-core/src/block/entities/command_block.rs`
- `crates/pumpkin-core/src/block/entities/decorated_pot.rs`
- `crates/pumpkin-core/src/block/entities/dispenser.rs`
- `crates/pumpkin-core/src/block/entities/dropper.rs`
- `crates/pumpkin-core/src/block/entities/furnace_like_block_entity.rs`
- `crates/pumpkin-core/src/block/entities/hanging_sign.rs`
- `crates/pumpkin-core/src/block/entities/hopper.rs`
- `crates/pumpkin-core/src/block/entities/jukebox.rs`
- `crates/pumpkin-core/src/block/entities/lectern.rs`
- `crates/pumpkin-core/src/block/entities/mod.rs`
- `crates/pumpkin-core/src/block/entities/shulker_box.rs`
- `crates/pumpkin-core/src/block/entities/sign.rs`
- `crates/pumpkin-core/src/block/registry.rs`
- `crates/pumpkin-core/src/command/argument_types/entity.rs`
- `crates/pumpkin-core/src/command/argument_types/entity_selector/option.rs`
- `crates/pumpkin-core/src/command/argument_types/entity_selector/parser.rs`
- `crates/pumpkin-core/src/command/argument_types/game_profile.rs`
- `crates/pumpkin-core/src/command/argument_types/objective.rs`
- `crates/pumpkin-core/src/command/argument_types/operation.rs`
- `crates/pumpkin-core/src/command/argument_types/resource_key.rs`
- `crates/pumpkin-core/src/command/argument_types/score_holder.rs`
- `crates/pumpkin-core/src/command/client_suggestions.rs`
- `crates/pumpkin-core/src/command/commands/advancement.rs`
- `crates/pumpkin-core/src/command/commands/attribute.rs`
- `crates/pumpkin-core/src/command/commands/ban.rs`
- `crates/pumpkin-core/src/command/commands/banip.rs`
- `crates/pumpkin-core/src/command/commands/banlist.rs`
- `crates/pumpkin-core/src/command/commands/bossbar.rs`
- `crates/pumpkin-core/src/command/commands/clear.rs`
- `crates/pumpkin-core/src/command/commands/clone.rs`
- `crates/pumpkin-core/src/command/commands/compute.rs`
- `crates/pumpkin-core/src/command/commands/damage.rs`
- `crates/pumpkin-core/src/command/commands/data.rs`
- `crates/pumpkin-core/src/command/commands/datapack.rs`
- `crates/pumpkin-core/src/command/commands/debug.rs`
- `crates/pumpkin-core/src/command/commands/defaultgamemode.rs`
- `crates/pumpkin-core/src/command/commands/deop.rs`
- `crates/pumpkin-core/src/command/commands/difficulty.rs`
- `crates/pumpkin-core/src/command/commands/effect.rs`
- `crates/pumpkin-core/src/command/commands/enchant.rs`
- `crates/pumpkin-core/src/command/commands/execute.rs`
- `crates/pumpkin-core/src/command/commands/experience.rs`
- `crates/pumpkin-core/src/command/commands/fetchprofile.rs`
- `crates/pumpkin-core/src/command/commands/fill.rs`
- `crates/pumpkin-core/src/command/commands/fillbiome.rs`
- `crates/pumpkin-core/src/command/commands/forceload.rs`
- `crates/pumpkin-core/src/command/commands/function.rs`
- `crates/pumpkin-core/src/command/commands/gamemode.rs`
- `crates/pumpkin-core/src/command/commands/gamerule.rs`
- `crates/pumpkin-core/src/command/commands/give.rs`
- `crates/pumpkin-core/src/command/commands/help.rs`
- `crates/pumpkin-core/src/command/commands/item.rs`
- `crates/pumpkin-core/src/command/commands/kick.rs`
- `crates/pumpkin-core/src/command/commands/kill.rs`
- `crates/pumpkin-core/src/command/commands/list.rs`
- `crates/pumpkin-core/src/command/commands/locate.rs`
- `crates/pumpkin-core/src/command/commands/loot.rs`
- `crates/pumpkin-core/src/command/commands/op.rs`
- `crates/pumpkin-core/src/command/commands/pardon.rs`
- `crates/pumpkin-core/src/command/commands/pardonip.rs`
- `crates/pumpkin-core/src/command/commands/particle.rs`
- `crates/pumpkin-core/src/command/commands/place.rs`
- `crates/pumpkin-core/src/command/commands/playsound.rs`
- `crates/pumpkin-core/src/command/commands/posteffect.rs`
- `crates/pumpkin-core/src/command/commands/random.rs`
- `crates/pumpkin-core/src/command/commands/recipe.rs`
- `crates/pumpkin-core/src/command/commands/reload.rs`
- `crates/pumpkin-core/src/command/commands/ride.rs`
- `crates/pumpkin-core/src/command/commands/rotate.rs`
- `crates/pumpkin-core/src/command/commands/saveall.rs`
- `crates/pumpkin-core/src/command/commands/saveoff.rs`
- `crates/pumpkin-core/src/command/commands/saveon.rs`
- `crates/pumpkin-core/src/command/commands/schedule.rs`
- `crates/pumpkin-core/src/command/commands/scoreboard.rs`
- `crates/pumpkin-core/src/command/commands/seed.rs`
- `crates/pumpkin-core/src/command/commands/setblock.rs`
- `crates/pumpkin-core/src/command/commands/setidletimeout.rs`
- `crates/pumpkin-core/src/command/commands/setworldspawn.rs`
- `crates/pumpkin-core/src/command/commands/spawnpoint.rs`
- `crates/pumpkin-core/src/command/commands/spectate.rs`
- `crates/pumpkin-core/src/command/commands/spreadplayers.rs`
- `crates/pumpkin-core/src/command/commands/stop.rs`
- `crates/pumpkin-core/src/command/commands/stopsound.rs`
- `crates/pumpkin-core/src/command/commands/stopwatch.rs`
- `crates/pumpkin-core/src/command/commands/summon.rs`
- `crates/pumpkin-core/src/command/commands/swing.rs`
- `crates/pumpkin-core/src/command/commands/tag.rs`
- `crates/pumpkin-core/src/command/commands/team.rs`
- `crates/pumpkin-core/src/command/commands/teammsg.rs`
- `crates/pumpkin-core/src/command/commands/teleport.rs`
- `crates/pumpkin-core/src/command/commands/test.rs`
- `crates/pumpkin-core/src/command/commands/tick.rs`
- `crates/pumpkin-core/src/command/commands/time.rs`
- `crates/pumpkin-core/src/command/commands/title.rs`
- `crates/pumpkin-core/src/command/commands/transfer.rs`
- `crates/pumpkin-core/src/command/commands/trigger.rs`
- `crates/pumpkin-core/src/command/commands/waypoint.rs`
- `crates/pumpkin-core/src/command/commands/weather.rs`
- `crates/pumpkin-core/src/command/commands/whitelist.rs`
- `crates/pumpkin-core/src/command/commands/worldborder.rs`
- `crates/pumpkin-core/src/command/context/command_source.rs`
- `crates/pumpkin-core/src/entity/ageable.rs`
- `crates/pumpkin-core/src/entity/ai/goal/breed.rs`
- `crates/pumpkin-core/src/entity/ai/goal/eat_grass.rs`
- `crates/pumpkin-core/src/entity/ai/goal/offer_flower.rs`
- `crates/pumpkin-core/src/entity/attributes.rs`
- `crates/pumpkin-core/src/entity/boss/wither.rs`
- `crates/pumpkin-core/src/entity/breath.rs`
- `crates/pumpkin-core/src/entity/combat.rs`
- `crates/pumpkin-core/src/entity/decoration/armor_stand.rs`
- `crates/pumpkin-core/src/entity/falling.rs`
- `crates/pumpkin-core/src/entity/item.rs`
- `crates/pumpkin-core/src/entity/living.rs`
- `crates/pumpkin-core/src/entity/marker.rs`
- `crates/pumpkin-core/src/entity/mob/creeper.rs`
- `crates/pumpkin-core/src/entity/mob/metadata.rs`
- `crates/pumpkin-core/src/entity/mob/mod.rs`
- `crates/pumpkin-core/src/entity/mob/slime.rs`
- `crates/pumpkin-core/src/entity/mob/spawn.rs`
- `crates/pumpkin-core/src/entity/mob/spider.rs`
- `crates/pumpkin-core/src/entity/mob/sun_burn.rs`
- `crates/pumpkin-core/src/entity/mob/zombie/zombie_villager.rs`
- `crates/pumpkin-core/src/entity/mod.rs`
- `crates/pumpkin-core/src/entity/passive/allay.rs`
- `crates/pumpkin-core/src/entity/passive/animal.rs`
- `crates/pumpkin-core/src/entity/passive/cat.rs`
- `crates/pumpkin-core/src/entity/passive/iron_golem.rs`
- `crates/pumpkin-core/src/entity/passive/nautilus.rs`
- `crates/pumpkin-core/src/entity/passive/ocelot.rs`
- `crates/pumpkin-core/src/entity/passive/sheep.rs`
- `crates/pumpkin-core/src/entity/passive/sniffer.rs`
- `crates/pumpkin-core/src/entity/passive/squid.rs`
- `crates/pumpkin-core/src/entity/passive/tamable.rs`
- `crates/pumpkin-core/src/entity/passive/villager/mod.rs`
- `crates/pumpkin-core/src/entity/passive/wandering_trader.rs`
- `crates/pumpkin-core/src/entity/passive/wolf.rs`
- `crates/pumpkin-core/src/entity/player.rs`
- `crates/pumpkin-core/src/entity/player/advancement.rs`
- `crates/pumpkin-core/src/entity/projectile/egg.rs`
- `crates/pumpkin-core/src/entity/projectile/ender_pearl.rs`
- `crates/pumpkin-core/src/entity/projectile/evoker_fangs.rs`
- `crates/pumpkin-core/src/entity/projectile/firework_rocket.rs`
- `crates/pumpkin-core/src/entity/projectile/fishing_bobber.rs`
- `crates/pumpkin-core/src/entity/projectile/lingering_potion.rs`
- `crates/pumpkin-core/src/entity/projectile/snowball.rs`
- `crates/pumpkin-core/src/entity/projectile/splash_potion.rs`
- `crates/pumpkin-core/src/entity/tnt.rs`
- `crates/pumpkin-core/src/entity/vehicle/boat.rs`
- `crates/pumpkin-core/src/entity/vehicle/minecart/chest.rs`
- `crates/pumpkin-core/src/entity/vehicle/minecart/hopper.rs`
- `crates/pumpkin-core/src/entity/vehicle/minecart/tnt.rs`
- `crates/pumpkin-core/src/entity/vehicle/vehicle.rs`
- `crates/pumpkin-core/src/item/items/spear.rs`
- `crates/pumpkin-core/src/lib.rs`
- `crates/pumpkin-core/src/net/chat/mod.rs`
- `crates/pumpkin-core/src/net/chunk_sender.rs`
- `crates/pumpkin-core/src/net/java/handshake.rs`
- `crates/pumpkin-core/src/net/java/login/encryption_response.rs`
- `crates/pumpkin-core/src/net/java/login/login_start.rs`
- `crates/pumpkin-core/src/net/java/mod.rs`
- `crates/pumpkin-core/src/net/java/play/attack.rs`
- `crates/pumpkin-core/src/net/java/play/change_game_mode.rs`
- `crates/pumpkin-core/src/net/java/play/chat_ack.rs`
- `crates/pumpkin-core/src/net/java/play/interact.rs`
- `crates/pumpkin-core/src/net/java/play/mod.rs`
- `crates/pumpkin-core/src/net/java/play/player_action.rs`
- `crates/pumpkin-core/src/net/java/play/player_position.rs`
- `crates/pumpkin-core/src/net/java/play/player_rotation.rs`
- `crates/pumpkin-core/src/net/java/play/use_item_on.rs`
- `crates/pumpkin-core/src/net/lan_broadcast.rs`
- `crates/pumpkin-core/src/net/management/handlers.rs`
- `crates/pumpkin-core/src/net/mod.rs`
- `crates/pumpkin-core/src/plugin/api/context.rs`
- `crates/pumpkin-core/src/plugin/api/events/player/mod.rs`
- `crates/pumpkin-core/src/plugin/mod.rs`
- `crates/pumpkin-core/src/server/mod.rs`
- `crates/pumpkin-core/src/server/tick_rate_manager.rs`
- `crates/pumpkin-core/src/world/bossbar.rs`
- `crates/pumpkin-core/src/world/chunker.rs`
- `crates/pumpkin-core/src/world/entity_tracker.rs`
- `crates/pumpkin-core/src/world/mod.rs`
- `crates/pumpkin-core/src/world/portal/mod.rs`
- `crates/pumpkin-core/src/world/scoreboard.rs`
- `crates/pumpkin-core/src/world/time.rs`
- `crates/pumpkin-data/Cargo.toml`
- `crates/pumpkin-data/src/item_stack/mod.rs`
- `crates/pumpkin-data/src/lib.rs`
- `crates/pumpkin-gametest/src/manager.rs`
- `crates/pumpkin-inventory/src/merchant/merchant_screen_handler.rs`
- `crates/pumpkin-macros/src/lib.rs`
- `crates/pumpkin-nbt/benches/nbt.rs`
- `crates/pumpkin-nbt/fuzz/Cargo.toml`
- `crates/pumpkin-nbt/src/deserializer.rs`
- `crates/pumpkin-nbt/src/lib.rs`
- `crates/pumpkin-nbt/src/serializer.rs`
- `crates/pumpkin-plugin-api/Cargo.toml`
- `crates/pumpkin-plugin-api/src/events/mod.rs`
- `crates/pumpkin-plugin-api/src/events/player/mod.rs`
- `crates/pumpkin-plugin-api/src/ext/player.rs`
- `crates/pumpkin-plugin-api/src/lib.rs`
- `crates/pumpkin-plugin-wit/v0.1/event.wit`
- `crates/pumpkin-plugin-wit/v0.1/player.wit`
- `crates/pumpkin-plugin-wit/v0.1/plugin.wit`
- `crates/pumpkin-plugin-wit/v0.1/scoreboard.wit`
- `crates/pumpkin-plugin-wit/v0.1/text.wit`
- `crates/pumpkin-plugin-wit/v0.2/event.wit`
- `crates/pumpkin-plugin-wit/v0.2/player.wit`
- `crates/pumpkin-plugin-wit/v0.2/plugin.wit`
- `crates/pumpkin-plugin-wit/v0.2/scoreboard.wit`
- `crates/pumpkin-plugin-wit/v0.2/text.wit`
- `crates/pumpkin-protocol/Cargo.toml`
- `crates/pumpkin-protocol/fuzz/Cargo.toml`
- `crates/pumpkin-protocol/src/codec/var_int.rs`
- `crates/pumpkin-protocol/src/lib.rs`
- `crates/pumpkin-protocol/src/packet.rs`
- `crates/pumpkin-protocol/src/serial/deserializer.rs`
- `crates/pumpkin-protocol/src/serial/serializer.rs`
- `crates/pumpkin-util/src/text/legacy.rs`
- `crates/pumpkin-util/src/text/mod.rs`
- `crates/pumpkin-util/src/translation.rs`
- `crates/pumpkin-util/src/version.rs`
- `crates/pumpkin-wasm-host-v0_1/src/bindings.rs`
- `crates/pumpkin-wasm-host-v0_1/src/context.rs`
- `crates/pumpkin-wasm-host-v0_1/src/events/cleanup.rs`
- `crates/pumpkin-wasm-host-v0_1/src/events/player.rs`
- `crates/pumpkin-wasm-host-v0_1/src/events/server.rs`
- `crates/pumpkin-wasm-host-v0_1/src/lib.rs`
- `crates/pumpkin-wasm-host-v0_1/src/player.rs`
- `crates/pumpkin-wasm-host-v0_1/src/scoreboard.rs`
- `crates/pumpkin-wasm-host-v0_1/src/server.rs`
- `crates/pumpkin-wasm-host-v0_1/src/text.rs`
- `crates/pumpkin-wasm-host-v0_2/src/bindings.rs`
- `crates/pumpkin-wasm-host-v0_2/src/context.rs`
- `crates/pumpkin-wasm-host-v0_2/src/events/cleanup.rs`
- `crates/pumpkin-wasm-host-v0_2/src/events/player.rs`
- `crates/pumpkin-wasm-host-v0_2/src/events/server.rs`
- `crates/pumpkin-wasm-host-v0_2/src/lib.rs`
- `crates/pumpkin-wasm-host-v0_2/src/player.rs`
- `crates/pumpkin-wasm-host-v0_2/src/scoreboard.rs`
- `crates/pumpkin-wasm-host-v0_2/src/server.rs`
- `crates/pumpkin-wasm-host-v0_2/src/text.rs`
- `crates/pumpkin-world/src/chunk/palette.rs`
- `crates/pumpkin-world/src/lib.rs`
- `crates/pumpkin/src/main.rs`
- `tools/pumpkin-codegen/src/advancement.rs`
- `tools/pumpkin-codegen/src/biome.rs`
- `tools/pumpkin-codegen/src/block.rs`
- `tools/pumpkin-codegen/src/effect.rs`
- `tools/pumpkin-codegen/src/enchantments.rs`
- `tools/pumpkin-codegen/src/item.rs`
- `tools/pumpkin-codegen/src/main.rs`
- `tools/pumpkin-codegen/src/template_pool.rs`
- `tools/pumpkin-codegen/src/translations.rs`
- `tools/pumpkin-codegen/src/wit/v0_2/mod.rs`
- `tools/pumpkin-codegen/src/wit/v0_2/packet_mapping.rs`
- `tools/pumpkin-tester/src/harness.rs`
- `vendor/patchbukkit/rust/src/java/native_callbacks/entity.rs`

Generator outputs regenerated or checked by the full generation run:

- `crates/pumpkin-data/src/generated/advancement.rs`
- `crates/pumpkin-data/src/generated/attributes.rs`
- `crates/pumpkin-data/src/generated/banner_pattern.rs`
- `crates/pumpkin-data/src/generated/biome.rs`
- `crates/pumpkin-data/src/generated/block.rs`
- `crates/pumpkin-data/src/generated/block_transformer.rs`
- `crates/pumpkin-data/src/generated/carver.rs`
- `crates/pumpkin-data/src/generated/cat_sound_variant.rs`
- `crates/pumpkin-data/src/generated/cat_variant.rs`
- `crates/pumpkin-data/src/generated/chat_type.rs`
- `crates/pumpkin-data/src/generated/chicken_sound_variant.rs`
- `crates/pumpkin-data/src/generated/chicken_variant.rs`
- `crates/pumpkin-data/src/generated/chunk_status.rs`
- `crates/pumpkin-data/src/generated/chunk_view_lut.rs`
- `crates/pumpkin-data/src/generated/compatibility.rs`
- `crates/pumpkin-data/src/generated/configured_feature.rs`
- `crates/pumpkin-data/src/generated/configured_features_generated.rs`
- `crates/pumpkin-data/src/generated/context_provider.rs`
- `crates/pumpkin-data/src/generated/cow_sound_variant.rs`
- `crates/pumpkin-data/src/generated/cow_variant.rs`
- `crates/pumpkin-data/src/generated/damage_type.rs`
- `crates/pumpkin-data/src/generated/data_component.rs`
- `crates/pumpkin-data/src/generated/decorated_pot_pattern.rs`
- `crates/pumpkin-data/src/generated/dimension.rs`
- `crates/pumpkin-data/src/generated/dye_color.rs`
- `crates/pumpkin-data/src/generated/effect.rs`
- `crates/pumpkin-data/src/generated/enchantment.rs`
- `crates/pumpkin-data/src/generated/enchantment_provider.rs`
- `crates/pumpkin-data/src/generated/enchantments.rs`
- `crates/pumpkin-data/src/generated/entity_pose.rs`
- `crates/pumpkin-data/src/generated/entity_status.rs`
- `crates/pumpkin-data/src/generated/entity_type.rs`
- `crates/pumpkin-data/src/generated/entity_variant.rs`
- `crates/pumpkin-data/src/generated/environment_attribute.rs`
- `crates/pumpkin-data/src/generated/flower_pot_transformations.rs`
- `crates/pumpkin-data/src/generated/fluid.rs`
- `crates/pumpkin-data/src/generated/frog_variant.rs`
- `crates/pumpkin-data/src/generated/game_event.rs`
- `crates/pumpkin-data/src/generated/game_rules.rs`
- `crates/pumpkin-data/src/generated/instrument.rs`
- `crates/pumpkin-data/src/generated/item.rs`
- `crates/pumpkin-data/src/generated/jukebox_song.rs`
- `crates/pumpkin-data/src/generated/loot_table.rs`
- `crates/pumpkin-data/src/generated/map_color.rs`
- `crates/pumpkin-data/src/generated/map_decoration.rs`
- `crates/pumpkin-data/src/generated/material_rule.rs`
- `crates/pumpkin-data/src/generated/message_type.rs`
- `crates/pumpkin-data/src/generated/meta_data_type.rs`
- `crates/pumpkin-data/src/generated/noise_parameter.rs`
- `crates/pumpkin-data/src/generated/noise_router.rs`
- `crates/pumpkin-data/src/generated/noise_settings.rs`
- `crates/pumpkin-data/src/generated/packet.rs`
- `crates/pumpkin-data/src/generated/painting_variant.rs`
- `crates/pumpkin-data/src/generated/particle.rs`
- `crates/pumpkin-data/src/generated/pig_sound_variant.rs`
- `crates/pumpkin-data/src/generated/pig_variant.rs`
- `crates/pumpkin-data/src/generated/placed_feature.rs`
- `crates/pumpkin-data/src/generated/placed_features_generated.rs`
- `crates/pumpkin-data/src/generated/potion.rs`
- `crates/pumpkin-data/src/generated/potion_brewing.rs`
- `crates/pumpkin-data/src/generated/processor_list.rs`
- `crates/pumpkin-data/src/generated/recipes.rs`
- `crates/pumpkin-data/src/generated/registry.rs`
- `crates/pumpkin-data/src/generated/scoreboard_slot.rs`
- `crates/pumpkin-data/src/generated/screen.rs`
- `crates/pumpkin-data/src/generated/slot_ranges.rs`
- `crates/pumpkin-data/src/generated/sound.rs`
- `crates/pumpkin-data/src/generated/sound_category.rs`
- `crates/pumpkin-data/src/generated/spawn_egg.rs`
- `crates/pumpkin-data/src/generated/statistic.rs`
- `crates/pumpkin-data/src/generated/status_effect.rs`
- `crates/pumpkin-data/src/generated/structure_metadata.rs`
- `crates/pumpkin-data/src/generated/structure_template.rs`
- `crates/pumpkin-data/src/generated/structures.rs`
- `crates/pumpkin-data/src/generated/tag.rs`
- `crates/pumpkin-data/src/generated/template_pool.rs`
- `crates/pumpkin-data/src/generated/test_instance.rs`
- `crates/pumpkin-data/src/generated/tracked_data.rs`
- `crates/pumpkin-data/src/generated/translation.rs`
- `crates/pumpkin-data/src/generated/trial_spawner.rs`
- `crates/pumpkin-data/src/generated/trim_material.rs`
- `crates/pumpkin-data/src/generated/trim_pattern.rs`
- `crates/pumpkin-data/src/generated/villager.rs`
- `crates/pumpkin-data/src/generated/wolf_sound_variant.rs`
- `crates/pumpkin-data/src/generated/wolf_variant.rs`
- `crates/pumpkin-data/src/generated/world_event.rs`
- `crates/pumpkin-data/src/generated/zombie_nautilus_variant.rs`
- `crates/pumpkin-plugin-api/src/generated/block.rs`
- `crates/pumpkin-plugin-api/src/generated/item.rs`
- `crates/pumpkin-plugin-api/src/generated/mod.rs`
- `crates/pumpkin-plugin-wit/v0.2/attributes.wit`
- `crates/pumpkin-plugin-wit/v0.2/biomes.wit`
- `crates/pumpkin-plugin-wit/v0.2/damage-types.wit`
- `crates/pumpkin-plugin-wit/v0.2/data-components.wit`
- `crates/pumpkin-plugin-wit/v0.2/enchantments.wit`
- `crates/pumpkin-plugin-wit/v0.2/entity-statuses.wit`
- `crates/pumpkin-plugin-wit/v0.2/entity-types.wit`
- `crates/pumpkin-plugin-wit/v0.2/game-events.wit`
- `crates/pumpkin-plugin-wit/v0.2/game-rules.wit`
- `crates/pumpkin-plugin-wit/v0.2/java-packets.wit`
- `crates/pumpkin-plugin-wit/v0.2/particles.wit`
- `crates/pumpkin-plugin-wit/v0.2/potions.wit`
- `crates/pumpkin-plugin-wit/v0.2/screens.wit`
- `crates/pumpkin-plugin-wit/v0.2/sounds.wit`
- `crates/pumpkin-plugin-wit/v0.2/statistics.wit`
- `crates/pumpkin-wasm-host-v0_1/src/generated_packets.rs`
- `crates/pumpkin-wasm-host-v0_2/src/generated_packets.rs`

## Join terrain loading and trial spawner sync (2026-10-30)

Player chunk tickets start at radius 4 and widen by 4 each time every chunk inside the current radius is loaded, so the nearest chunks reach the client first; before, a view distance 16 join delivered nothing for about 21 s in a debug build because the whole area finished at once. Chunk packets send trial spawner block entities as vanilla's update tag (`spawn_data`, `next_mob_spawns_at`), because the client has no `minecraft:trial_spawner` registry for the configs.

Touched upstream files: `crates/pumpkin-core/src/world/chunker.rs` (ticket update extracted into `apply_chunk_tickets`, new `ramp_chunk_tickets`), `crates/pumpkin-core/src/entity/player.rs` (`chunk_ticket_radius`, ramp call in tick, reset on world change), `crates/pumpkin-core/src/net/java/chunk_data/v1_18.rs`, `crates/pumpkin-core/src/block/entities/trial_spawner.rs` (`chunk_data_nbt`).
## Panic visibility
- `crates/pumpkin/src/main.rs`: the panic hook writes the message, location and backtrace to stderr and `panic.log` before anything else, because a non-unwinding panic aborts before the deferred crash report is flushed. The player-placement abort was `BlockState::to_be_network_id` asserting `id < STATE_COUNT` on a modded state (Bedrock sound/particle path); the Bedrock path is removed by the Java-only change.

## NeoForge mod payloads and bridged players (2026-10-30)

Serverbound non-`minecraft:` custom payloads, `use`/`useOn` of modded items and a per-tick inventory step for players holding modded items run in the bridge JVM against a `ServerPlayer` view (`RustPlayer`) of the Rust player: Rust sends a snapshot (position, rotation, game mode, selected slot, inventory), Java runs NeoForge handlers on the tick thread and returns clientbound payloads and the inventory stacks that changed. Mod-defined data components travel inside `custom_data` under `ferrum_mod_components`. Java-spawned entities are dropped with one warning.

Touched upstream files: `crates/pumpkin-core/src/entity/player.rs` (`tick_player` call after `process_inbound_packets`), `crates/pumpkin-core/src/server/mod.rs` (`player_left` in `remove_player`), `crates/pumpkin-core/src/net/java/mod.rs` (mod payload forward in the custom payload arm), `crates/pumpkin-core/src/net/java/play/use_item.rs`, `crates/pumpkin-core/src/net/java/play/use_item_on.rs` (modded item dispatch before vanilla handling).

## NeoForge mod item components, Java packets and mod entities (2026-10-30)

Mod item components (`hbm:gun_state`) stay NBT in the stack's custom data under `ferrum_mod_components`. The 26.2 translator turns them into the client's typed components (ids and stream codecs come from Java, encodings cached per component set) and parses them back from creative slots; container click hashes skip components the engine has no id for. Vanilla clientbound packets that Java produces (sounds, particles, level and entity events, direct sends to a bridged player) are encoded in Java with the 26.2 protocol, queued in an outbox with a target (player, area, tracking players of an entity) and forwarded by Rust without translation. `Level.addFreshEntity` of a mod entity type allocates the id in Rust, registers a `ModEntity` in the Rust world (runtime entity types installed from the manifest) and keeps the Java instance, which one serial pass per world tick (after Rust's own entities, before the tracker update) ticks in id order; position, motion and removal are mirrored, the existing entity tracker handles spawn/move/despawn and Java sends dirty synched data. Vanilla entity types spawned by Java are still dropped. Java level queries (`getEntities`, `getEntity`) merge Java's bridged entities with Rust's entities and players, and damage on those is applied by Rust.

Touched upstream files: `crates/pumpkin-core/src/net/java/compatibility.rs` (`item` splits mod components out of custom data, `creative_item` and `item_hash` handle ids the engine lacks, runtime entity type id), `crates/pumpkin-core/src/net/java/mod.rs` (`try_enqueue_native_packet`, byte reservation split out of `reserve_pending_bytes`), `crates/pumpkin-core/src/world/mod.rs` (`tick_entities` call), `crates/pumpkin-core/src/neoforge.rs` (owned), `crates/pumpkin-data/src/runtime.rs` (runtime entity types), `crates/ferrum-neoforge` (owned), `bridge/neoforge-runtime` (owned).

## NeoForge menus, mod block use, Java-spawned items and chunk watchers (2026-10-30)

Right-clicking a modded block runs its `useItemOn`/`useWithoutItem` in the bridge JVM (`PlayerBridge.useBlock`) before Rust's own handling. Menus Java opens (`ServerPlayer.openMenu`, including NeoForge's advanced open-screen payload) are owned by Java: `MenuBridge` decodes the client's container click/close/button packets exactly as sent, runs them against the player's inventory view that Rust refreshes before and reads back after every step, and broadcasts slot changes each player tick. Java-spawned item entities and experience orbs become real Rust entities (`spawn_item`, `spawn_experience`). Rust reports the chunks a player watches (`TrackingBridge`), and runs HBM's end-of-tick sync flush, so block entity sync payloads reach the watchers.

Touched upstream files: `crates/pumpkin-core/src/net/java/mod.rs` (container click, button click and close arms forward to Java while it owns a menu), `crates/pumpkin-core/src/net/java/play/use_item_on.rs` (modded block dispatch before the modded item dispatch), `crates/pumpkin-core/src/server/mod.rs` (`server_tick_end` after the world ticks), `crates/pumpkin-core/src/neoforge.rs` (owned), `bridge/neoforge-runtime` (owned), `scripts/test-neoforge-client.ps1` (owned).
